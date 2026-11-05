//! Object storage behind a trait (ADR 0009). Keys are content addresses built only from a tenant UUID and a
//! SHA-256 digest, so no user-controlled string ever reaches a filesystem path.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ObjectKey {
    pub tenant: Uuid,
    pub sha256: [u8; 32],
}

impl ObjectKey {
    pub fn as_string(&self) -> String {
        format!("{}/{}", self.tenant, hex::encode(self.sha256))
    }
}

/// An object written to the staging area, not yet visible under its key (see [`ObjectStore::stage`]).
#[derive(Debug)]
pub struct Staged {
    pub key: ObjectKey,
    path: PathBuf,
}

/// What a staging sweep did ([`FsStore::sweep_staging`]).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SweepCount {
    /// Staged copies whose database row exists: promoted (or dropped when the object was already stored).
    pub promoted: usize,
    /// Staged copies older than the cut-off without a database row: deleted (the upload never committed).
    pub deleted: usize,
}

pub trait ObjectStore: Send + Sync {
    /// Writes the object durably to a staging area: invisible to `get`/`exists` until [`ObjectStore::promote`].
    /// Upload order: stage, commit the database row, promote. A crash or database failure before the commit
    /// leaves only a staging copy, which the staging sweep removes; never a stored object without a row.
    fn stage(&self, key: &ObjectKey, data: &[u8]) -> io::Result<Staged>;
    /// Atomically moves a staged object to its key (rename + directory fsync). Idempotent for intact objects.
    fn promote(&self, staged: &Staged) -> io::Result<()>;
    /// Drops a staged object (the database transaction was rolled back for sure).
    fn discard(&self, staged: Staged);
    fn get(&self, key: &ObjectKey) -> io::Result<Vec<u8>>;
    fn exists(&self, key: &ObjectKey) -> io::Result<bool>;
    /// Keys of the staged copies present now (input of the staging sweep).
    fn staged_keys(&self) -> io::Result<Vec<ObjectKey>> {
        Ok(Vec::new())
    }
    /// Resolves staged copies given the keys that have a database row; see [`FsStore::sweep_staging`].
    fn sweep_staged(
        &self,
        _max_age: std::time::Duration,
        _with_row: &std::collections::HashSet<ObjectKey>,
    ) -> io::Result<SweepCount> {
        Ok(SweepCount::default())
    }
    /// stage + promote, for callers without a database row to order against.
    fn put(&self, key: &ObjectKey, data: &[u8]) -> io::Result<()> {
        let s = self.stage(key, data)?;
        let r = self.promote(&s);
        if r.is_err() {
            self.discard(s);
        }
        r
    }
}

/// Filesystem backend: `<root>/<tenant>/<hh>/<sha256hex>`, files 0600, dirs 0700; staging copies in
/// `<root>/.staging/<tenant>.<sha256hex>.<uuid>` (same filesystem, so promotion is an atomic rename).
pub struct FsStore {
    root: PathBuf,
}

fn sync_dir(dir: &Path) -> io::Result<()> {
    fs::File::open(dir)?.sync_all()
}

impl FsStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn path(&self, key: &ObjectKey) -> PathBuf {
        let h = hex::encode(key.sha256);
        self.root.join(key.tenant.to_string()).join(&h[..2]).join(h)
    }

    pub fn staging_dir(&self) -> PathBuf {
        self.root.join(".staging")
    }

    fn intact(&self, key: &ObjectKey) -> bool {
        fs::read(self.path(key))
            .map(|b| crate::crypto::sha256(&b) == key.sha256)
            .unwrap_or(false)
    }

    /// Staged copies currently present: (key, path, age).
    pub fn staged(&self) -> io::Result<Vec<(ObjectKey, PathBuf, std::time::Duration)>> {
        let dir = self.staging_dir();
        let rd = match fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut out = Vec::new();
        for e in rd {
            let e = e?;
            let name = e.file_name().to_string_lossy().into_owned();
            let mut it = name.split('.');
            let (Some(t), Some(h), Some(_), None) = (it.next(), it.next(), it.next(), it.next()) else {
                continue;
            };
            let (Ok(tenant), Ok(sha)) = (Uuid::parse_str(t), hex::decode(h)) else {
                continue;
            };
            let Ok(sha256) = <[u8; 32]>::try_from(sha.as_slice()) else {
                continue;
            };
            let age = e.metadata()?.modified()?.elapsed().unwrap_or_default();
            out.push((ObjectKey { tenant, sha256 }, e.path(), age));
        }
        Ok(out)
    }

    /// Resolves staging copies: `has_row(key)` says whether the database has an artifact for the key. With a row,
    /// the copy is promoted (the commit succeeded but the promotion did not happen, e.g. a crash right after it);
    /// without a row and older than `max_age`, it is deleted. Younger row-less copies may belong to an upload in
    /// flight and are left alone.
    pub fn sweep_staging(
        &self,
        max_age: std::time::Duration,
        mut has_row: impl FnMut(&ObjectKey) -> io::Result<bool>,
    ) -> io::Result<SweepCount> {
        let mut n = SweepCount::default();
        for (key, path, age) in self.staged()? {
            if has_row(&key)? {
                self.promote(&Staged { key, path })?;
                n.promoted += 1;
            } else if age >= max_age {
                fs::remove_file(&path)?;
                n.deleted += 1;
            }
        }
        if n.deleted > 0 {
            sync_dir(&self.staging_dir())?;
        }
        Ok(n)
    }
}

impl ObjectStore for FsStore {
    fn stage(&self, key: &ObjectKey, data: &[u8]) -> io::Result<Staged> {
        let dir = self.staging_dir();
        fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
        let path = dir.join(format!(
            "{}.{}.{}",
            key.tenant,
            hex::encode(key.sha256),
            Uuid::new_v4().simple()
        ));
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        let written = f.write_all(data).and_then(|()| f.sync_all());
        drop(f);
        // A failed write (e.g. ENOSPC) must not leave its partial copy behind (chaos C4).
        if let Err(e) = written {
            let _ = fs::remove_file(&path);
            return Err(e);
        }
        Ok(Staged { key: *key, path })
    }

    fn promote(&self, staged: &Staged) -> io::Result<()> {
        let path = self.path(&staged.key);
        if self.intact(&staged.key) {
            // Content-addressed: an intact copy is already stored.
            fs::remove_file(&staged.path)?;
            return Ok(());
        }
        // Missing or corrupted copy: replace atomically. Re-uploading the original restores a damaged object.
        let dir = path.parent().ok_or_else(|| io::Error::other("no parent"))?;
        fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        fs::rename(&staged.path, &path)?;
        // The rename is durable only once both directories are synced.
        sync_dir(dir)?;
        sync_dir(&self.staging_dir())
    }

    fn discard(&self, staged: Staged) {
        let _ = fs::remove_file(&staged.path);
    }

    fn staged_keys(&self) -> io::Result<Vec<ObjectKey>> {
        Ok(self.staged()?.into_iter().map(|s| s.0).collect())
    }

    fn sweep_staged(
        &self,
        max_age: std::time::Duration,
        with_row: &std::collections::HashSet<ObjectKey>,
    ) -> io::Result<SweepCount> {
        self.sweep_staging(max_age, |k| Ok(with_row.contains(k)))
    }

    fn get(&self, key: &ObjectKey) -> io::Result<Vec<u8>> {
        match fs::read(self.path(key)) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // Committed but not yet promoted (the instant after the commit, or a crash in between until the
                // staging sweep runs): serve the intact staged copy. Callers only ask for keys that have a row.
                for (k, path, _) in self.staged()? {
                    if k == *key
                        && let Ok(b) = fs::read(&path)
                        && crate::crypto::sha256(&b) == key.sha256
                    {
                        return Ok(b);
                    }
                }
                Err(e)
            }
            r => r,
        }
    }

    fn exists(&self, key: &ObjectKey) -> io::Result<bool> {
        Ok(self.path(key).exists())
    }
}

/// Staging sweep job: promotes committed staging copies and deletes row-less ones older than `max_age`; each tenant
/// with work done gets one audit event carrying only the counts.
pub async fn sweep_staging_job(
    pool: &sqlx::PgPool,
    store: std::sync::Arc<dyn ObjectStore>,
    max_age: std::time::Duration,
) -> Result<SweepCount, crate::error::ApiError> {
    use crate::error::ApiError;
    let s2 = std::sync::Arc::clone(&store);
    let keys = tokio::task::spawn_blocking(move || s2.staged_keys())
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::storage)?;
    let mut by_tenant: std::collections::BTreeMap<Uuid, Vec<Vec<u8>>> = std::collections::BTreeMap::new();
    for k in &keys {
        by_tenant.entry(k.tenant).or_default().push(k.sha256.to_vec());
    }
    let mut with_row = std::collections::HashSet::new();
    for (t, shas) in &by_tenant {
        let mut tx = crate::db::tenant_tx(pool, *t).await?;
        let found: Vec<Vec<u8>> = sqlx::query_scalar("SELECT sha256 FROM artifacts WHERE sha256 = ANY($1)")
            .bind(shas)
            .fetch_all(&mut *tx)
            .await?;
        tx.commit().await?;
        for f in found {
            if let Ok(sha256) = <[u8; 32]>::try_from(f.as_slice()) {
                with_row.insert(ObjectKey { tenant: *t, sha256 });
            }
        }
    }
    let set = with_row.clone();
    let s2 = std::sync::Arc::clone(&store);
    let n = tokio::task::spawn_blocking(move || s2.sweep_staged(max_age, &set))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::storage)?;
    if n != SweepCount::default() {
        // The sweep reports totals; attribute them per tenant from what is gone now (counts only, no names).
        let left: std::collections::HashSet<ObjectKey> = {
            let s2 = std::sync::Arc::clone(&store);
            tokio::task::spawn_blocking(move || s2.staged_keys())
                .await
                .map_err(ApiError::internal)?
                .map_err(ApiError::storage)?
                .into_iter()
                .collect()
        };
        for t in by_tenant.keys() {
            let gone = |row: bool| {
                keys.iter()
                    .filter(|k| k.tenant == *t && with_row.contains(k) == row && !left.contains(k))
                    .count()
            };
            let (promoted, deleted) = (gone(true), gone(false));
            if promoted + deleted == 0 {
                continue;
            }
            let mut tx = crate::db::tenant_tx(pool, *t).await?;
            crate::audit::append(
                &mut tx,
                *t,
                crate::audit::Record {
                    actor_user_id: None,
                    actor_kind: "system",
                    action: "storage.staging_swept",
                    target_type: "tenant",
                    target_id: Some(t.to_string()),
                    details: serde_json::json!({ "promoted": promoted, "deleted": deleted, "max_age_secs": max_age.as_secs() }),
                },
            )
            .await?;
            tx.commit().await?;
        }
    }
    Ok(n)
}

/// Runs [`sweep_staging_job`] every `every`.
pub fn spawn_staging_sweeper(
    pool: sqlx::PgPool,
    store: std::sync::Arc<dyn ObjectStore>,
    max_age: std::time::Duration,
    every: std::time::Duration,
) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(every);
        loop {
            tick.tick().await;
            match sweep_staging_job(&pool, std::sync::Arc::clone(&store), max_age).await {
                Ok(n) if n != SweepCount::default() => {
                    tracing::info!(promoted = n.promoted, deleted = n.deleted, "staging sweep");
                }
                Ok(_) => {}
                Err(e) => tracing::error!(error = ?e, "staging sweep failed"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_get_layout_and_permissions() {
        let root = std::env::temp_dir().join(format!("kz-store-{}", Uuid::new_v4()));
        let s = FsStore::new(&root);
        let key = ObjectKey {
            tenant: Uuid::from_u128(5),
            sha256: crate::crypto::sha256(b"hello"),
        };
        s.put(&key, b"hello").unwrap();
        s.put(&key, b"hello").unwrap();
        assert_eq!(s.get(&key).unwrap(), b"hello");
        let p = s.path(&key);
        assert!(p.starts_with(&root));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        fs::remove_dir_all(root).unwrap();
    }

    /// Chaos C4 (disk full): a write that fails part-way must not leave its temporary file behind, or a full disk
    /// stays full (every failed upload keeps its partial copy) and recovery needs manual cleanup.
    #[test]
    fn failed_put_removes_its_temp_file() {
        let root = std::env::temp_dir().join(format!("kz-store-{}", Uuid::new_v4()));
        let s = FsStore::new(&root);
        let key = ObjectKey {
            tenant: Uuid::from_u128(6),
            sha256: crate::crypto::sha256(b"blocked"),
        };
        // The final name is occupied by a non-empty directory: the rename fails after the data was written.
        let p = s.path(&key);
        fs::create_dir_all(p.join("x")).unwrap();
        assert!(s.put(&key, b"blocked").is_err());
        let dir = p.parent().unwrap();
        let tmp: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp-"))
            .collect();
        assert!(tmp.is_empty(), "temporary files left behind: {tmp:?}");
        assert!(s.staged().unwrap().is_empty(), "staging copy left behind");
        fs::remove_dir_all(root).unwrap();
    }

    fn key(t: u128, data: &[u8]) -> ObjectKey {
        ObjectKey {
            tenant: Uuid::from_u128(t),
            sha256: crate::crypto::sha256(data),
        }
    }

    /// A staged object is invisible until promoted; promotion is idempotent for intact objects.
    #[test]
    fn staged_objects_are_invisible_until_promoted() {
        let root = std::env::temp_dir().join(format!("kz-store-{}", Uuid::new_v4()));
        let s = FsStore::new(&root);
        let k = key(7, b"doc");
        let st = s.stage(&k, b"doc").unwrap();
        assert!(!s.exists(&k).unwrap());
        assert_eq!(
            s.get(&k).unwrap(),
            b"doc",
            "a committed-but-unpromoted object is still served"
        );
        s.promote(&st).unwrap();
        assert!(s.exists(&k).unwrap());
        assert!(s.staged().unwrap().is_empty());
        let again = s.stage(&k, b"doc").unwrap();
        s.promote(&again).unwrap();
        assert!(s.staged().unwrap().is_empty());
        s.discard(s.stage(&k, b"doc").unwrap());
        assert!(s.staged().unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    /// The sweep promotes staged copies that have a row, deletes old row-less ones and keeps young ones.
    #[test]
    fn sweep_promotes_committed_and_deletes_old_orphans() {
        let root = std::env::temp_dir().join(format!("kz-store-{}", Uuid::new_v4()));
        let s = FsStore::new(&root);
        let (committed, orphan) = (key(8, b"committed"), key(8, b"orphan"));
        s.stage(&committed, b"committed").unwrap();
        s.stage(&orphan, b"orphan").unwrap();
        let hour = std::time::Duration::from_secs(3600);
        let n = s.sweep_staging(hour, |k| Ok(*k == committed)).unwrap();
        assert_eq!(
            n,
            SweepCount {
                promoted: 1,
                deleted: 0
            },
            "young orphan kept (upload may be in flight)"
        );
        assert!(s.exists(&committed).unwrap());
        let n = s
            .sweep_staging(std::time::Duration::ZERO, |k| Ok(*k == committed))
            .unwrap();
        assert_eq!(
            n,
            SweepCount {
                promoted: 0,
                deleted: 1
            }
        );
        assert!(s.staged().unwrap().is_empty() && !s.exists(&orphan).unwrap());
        fs::remove_dir_all(root).unwrap();
    }
}
