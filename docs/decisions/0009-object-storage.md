# ADR 0009: Object storage behind a trait; filesystem backend first; MinIO not adopted

Status: accepted (2026-11-05)

## Decision
- `ObjectStore` trait (put/get/exists/delete by content-addressed key `tenant/<uuid>/sha256/<hex>`). M2 implements a
  filesystem backend (atomic write via temp file + rename, path built only from validated UUID and hex, no user path
  components). S3-compatible backend later behind the same trait.
- MinIO server is AGPL-3.0; running it as a separate service is possible, but it is not needed for tests and its
  licence implications for a commercial distribution must be reviewed first. Not adopted.
