#![allow(dead_code, clippy::unwrap_used)]
//! Integration-test harness: real PostgreSQL (runtime role for the app, owner role for setup/tamper).
//! Fails loudly when the test database is not configured.

use std::net::SocketAddr;

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use base64::Engine;
use http_body_util::BodyExt;
use kizuna_api::config::Config;
use kizuna_api::{AppState, db, router};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::sync::OnceCell;
use tower::ServiceExt;
use uuid::Uuid;

pub const PW: &str = "correct horse battery staple";

fn env(k: &str) -> String {
    if let Ok(v) = std::env::var(k) {
        return v;
    }
    let f = concat!(env!("CARGO_MANIFEST_DIR"), "/../../.env.local");
    std::fs::read_to_string(f)
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix(&format!("{k}=")).map(str::to_owned))
        })
        .unwrap_or_else(|| panic!("{k} not set; run scripts/db-setup.sh or export it"))
}

static MIGRATED: OnceCell<()> = OnceCell::const_new();

pub async fn owner_pool() -> PgPool {
    db::connect(&env("KIZUNA_TEST_OWNER_DATABASE_URL"), 2)
        .await
        .unwrap()
}

pub async fn runtime_pool() -> PgPool {
    db::connect(&env("KIZUNA_TEST_DATABASE_URL"), 3)
        .await
        .unwrap()
}

pub fn test_config() -> Config {
    let key = base64::engine::general_purpose::STANDARD.encode(rand_bytes());
    let url = env("KIZUNA_TEST_DATABASE_URL");
    let storage = std::env::temp_dir().join(format!("kizuna-test-{}", Uuid::now_v7()));
    Config::from_lookup(|k| match k {
        "KIZUNA_DATABASE_URL" => Some(url.clone()),
        "KIZUNA_DATA_KEY" => Some(key.clone()),
        "KIZUNA_STORAGE_DIR" => Some(storage.to_string_lossy().into_owned()),
        "KIZUNA_EXTRACTION_DIR" => {
            Some(concat!(env!("CARGO_MANIFEST_DIR"), "/../../services/extraction/src").into())
        }
        _ => None,
    })
    .unwrap()
}

fn rand_bytes() -> [u8; 32] {
    kizuna_api::crypto::random_bytes::<32>()
}

pub struct Ctx {
    pub app: Router,
    pub st: AppState,
    pub owner: PgPool,
    pub ip: SocketAddr,
}

pub struct Resp {
    pub status: StatusCode,
    pub body: Value,
    pub headers: HeaderMap,
}

pub async fn ctx() -> Ctx {
    ctx_with(|_| {}).await
}

pub async fn ctx_with(f: impl FnOnce(&mut Config)) -> Ctx {
    MIGRATED
        .get_or_init(|| async {
            let p = owner_pool().await;
            db::MIGRATOR.run(&p).await.unwrap();
            p.close().await;
        })
        .await;
    let mut cfg = test_config();
    f(&mut cfg);
    let st = AppState::new(runtime_pool().await, cfg);
    let b = Uuid::new_v4().as_bytes()[0..4].to_vec();
    Ctx {
        app: router(st.clone()),
        st,
        owner: owner_pool().await,
        ip: SocketAddr::from(([10, b[1], b[2], b[3].max(1)], 40000)),
    }
}

impl Ctx {
    pub async fn raw(&self, req: Request<Body>) -> Resp {
        let mut req = req;
        req.extensions_mut().insert(ConnectInfo(self.ip));
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let body = serde_json::from_slice(&bytes)
            .unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into()));
        Resp {
            status,
            body,
            headers,
        }
    }

    pub async fn call(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Resp {
        let mut b = Request::builder().method(method).uri(path);
        if let Some(t) = token {
            b = b.header("authorization", format!("Bearer {t}"));
        }
        let req = match body {
            Some(v) => b
                .header("content-type", "application/json")
                .body(Body::from(v.to_string()))
                .unwrap(),
            None => b.body(Body::empty()).unwrap(),
        };
        self.raw(req).await
    }

    pub async fn get(&self, path: &str, token: &str) -> Resp {
        self.call(Method::GET, path, Some(token), None).await
    }

    pub async fn post(&self, path: &str, token: Option<&str>, body: Value) -> Resp {
        self.call(Method::POST, path, token, Some(body)).await
    }

    /// Creates a tenant directly through the owner role, returns (id, slug).
    pub async fn tenant(&self) -> (Uuid, String) {
        let id = Uuid::now_v7();
        let slug = format!("t-{}", &id.simple().to_string()[20..]);
        sqlx::query("INSERT INTO tenants (id, slug, name) VALUES ($1,$2,'Test')")
            .bind(id)
            .bind(&slug)
            .execute(&self.owner)
            .await
            .unwrap();
        (id, slug)
    }

    pub async fn user(&self, tenant: Uuid, role: &str) -> (Uuid, String) {
        let email = format!("{}@example.test", Uuid::new_v4().simple());
        let mut tx = db::tenant_tx(&self.owner, tenant).await.unwrap();
        let id = kizuna_api::users::insert_user(
            &mut tx,
            tenant,
            &email,
            "Test User",
            policy_engine::Role::parse(role).unwrap(),
            PW,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        (id, email)
    }

    pub async fn login(&self, slug: &str, email: &str, pw: &str, totp: Option<String>) -> Resp {
        self.post(
            "/api/v1/auth/login",
            None,
            json!({ "tenant": slug, "email": email, "password": pw, "totp_code": totp }),
        )
        .await
    }

    pub async fn token(&self, slug: &str, email: &str) -> String {
        let r = self.login(slug, email, PW, None).await;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
        r.body["access_token"].as_str().unwrap().to_owned()
    }

    /// Enrols TOTP for the session's user; returns the raw secret.
    pub async fn enroll_totp(&self, token: &str) -> Vec<u8> {
        let r = self
            .post("/api/v1/auth/mfa/totp/setup", Some(token), json!({}))
            .await;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
        let secret = base32_decode(r.body["secret_base32"].as_str().unwrap());
        let code = kizuna_api::crypto::totp_now(&secret, now());
        let r = self
            .post(
                "/api/v1/auth/mfa/totp/confirm",
                Some(token),
                json!({ "code": code }),
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
        secret
    }

    /// A tenant with an MFA-verified admin session: (tenant, slug, admin id, token).
    pub async fn admin(&self) -> (Uuid, String, Uuid, String) {
        let (t, slug) = self.tenant().await;
        let (uid, email) = self.user(t, "tenant_admin").await;
        let tok = self.token(&slug, &email).await;
        self.enroll_totp(&tok).await;
        (t, slug, uid, tok)
    }

    pub async fn security_kinds(&self, tenant: Uuid) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT kind FROM security_events WHERE tenant_id = $1 ORDER BY occurred_at",
        )
        .bind(tenant)
        .fetch_all(&self.owner)
        .await
        .unwrap()
    }
}

pub fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

pub fn base32_decode(s: &str) -> Vec<u8> {
    let (mut buf, mut bits, mut out) = (0u32, 0u32, Vec::new());
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'2'..=b'7' => c - b'2' + 26,
            _ => panic!("bad base32"),
        } as u32;
        buf = (buf << 5) | v;
        bits += 5;
        if bits >= 8 {
            out.push((buf >> (bits - 8)) as u8);
            bits -= 8;
        }
    }
    out
}
