#![allow(clippy::unwrap_used)]
//! Regressions found by the bug hunt on de315db.
mod common;

use std::pin::Pin;
use std::task::{Context, Poll};

use axum::body::{Body, Bytes};
use axum::http::{Method, Request, StatusCode};
use common::*;
use http_body::Frame;
use serde_json::json;
use tokio::sync::oneshot;

/// Request body that sends `head`, then waits for `gate` before sending `tail` (a slow client mid-upload).
struct GatedBody {
    head: Option<Bytes>,
    gate: Option<oneshot::Receiver<()>>,
    tail: Option<Bytes>,
}

impl http_body::Body for GatedBody {
    type Data = Bytes;
    type Error = std::convert::Infallible;
    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        if let Some(h) = self.head.take() {
            return Poll::Ready(Some(Ok(Frame::data(h))));
        }
        if let Some(g) = self.gate.as_mut() {
            if Pin::new(g).poll(cx).is_pending() {
                return Poll::Pending;
            }
            self.gate = None;
        }
        Poll::Ready(self.tail.take().map(|t| Ok(Frame::data(t))))
    }
}

use std::future::Future;

#[tokio::test]
async fn upload_is_refused_when_the_session_is_revoked_mid_upload() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (_, email) = c.user(t, "analyst").await;
    let tok = c.token(&slug, &email).await;
    let boundary = "kizunagatedboundary";
    let head = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"m.csv\"\r\nContent-Type: text/csv\r\n\r\npart number,material\r\n"
    );
    let tail = format!("REVOKED-MID-UPLOAD,SUS304\r\n\r\n--{boundary}--\r\n");
    let (open, gate) = oneshot::channel();
    let body = GatedBody {
        head: Some(Bytes::from(head)),
        gate: Some(gate),
        tail: Some(Bytes::from(tail)),
    };
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/artifacts")
        .header("authorization", format!("Bearer {tok}"))
        .header("content-type", format!("multipart/form-data; boundary={boundary}"))
        .body(Body::new(body))
        .unwrap();
    let app = c.app.clone();
    let ip = c.ip;
    let upload = tokio::spawn(async move {
        use tower::ServiceExt;
        let mut req = req;
        req.extensions_mut().insert(axum::extract::ConnectInfo(ip));
        app.oneshot(req).await.unwrap().status()
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    // The user logs out (or an admin revokes the session) while the body is still streaming.
    assert_eq!(
        c.post("/api/v1/auth/logout", Some(&tok), json!({})).await.status,
        StatusCode::OK
    );
    open.send(()).unwrap();
    assert_eq!(upload.await.unwrap(), StatusCode::UNAUTHORIZED);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM artifacts WHERE tenant_id = $1")
        .bind(t)
        .fetch_one(&c.owner)
        .await
        .unwrap();
    assert_eq!(n, 0, "a revoked session must not be able to finish storing an artifact");
}
