//! Object storage behind a trait (ADR 0009). Keys are content addresses built only from a tenant UUID and a
//! SHA-256 digest, so no user-controlled string ever reaches a filesystem path.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::PathBuf;

use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectKey {
    pub tenant: Uuid,
    pub sha256: [u8; 32],
}

impl ObjectKey {
    pub fn as_string(&self) -> String {
        format!("{}/{}", self.tenant, hex::encode(self.sha256))
    }
}

pub trait ObjectStore: Send + Sync {
    fn put(&self, key: &ObjectKey, data: &[u8]) -> io::Result<()>;
    fn get(&self, key: &ObjectKey) -> io::Result<Vec<u8>>;
    fn exists(&self, key: &ObjectKey) -> io::Result<bool>;
}

/// Filesystem backend: `<root>/<tenant>/<hh>/<sha256hex>`, files 0600, dirs 0700, atomic write via rename.
pub struct FsStore {
    root: PathBuf,
}

impl FsStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn path(&self, key: &ObjectKey) -> PathBuf {
        let h = hex::encode(key.sha256);
        self.root.join(key.tenant.to_string()).join(&h[..2]).join(h)
    }
}

impl ObjectStore for FsStore {
    fn put(&self, key: &ObjectKey, data: &[u8]) -> io::Result<()> {
        let path = self.path(key);
        if path.exists() {
            return Ok(()); // content-addressed: same key, same bytes
        }
        let dir = path.parent().ok_or_else(|| io::Error::other("no parent"))?;
        fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        let tmp = dir.join(format!(".tmp-{}", Uuid::new_v4()));
        let mut f = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, &path)
    }

    fn get(&self, key: &ObjectKey) -> io::Result<Vec<u8>> {
        fs::read(self.path(key))
    }

    fn exists(&self, key: &ObjectKey) -> io::Result<bool> {
        Ok(self.path(key).exists())
    }
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
}
