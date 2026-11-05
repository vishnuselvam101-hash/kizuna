//! KIZUNA evidence pack (`kizuna-evidence-pack`, version 1): one ZIP that lets a customer or auditor check a decision
//! offline, with no database and no network.
//!
//! Contents: `manifest.json` (canonical JSON), `manifest.sig.json` (Ed25519 over `"KZEP1" || SHA-256(manifest)`), the
//! decision record, its derivation (claims → source locations, hashed into the audit chain at decision time), the
//! original documents (or only their hashes), and for every referenced audit event an RFC 6962 Merkle inclusion proof
//! into a signed KZCP3 checkpoint of the tenant's audit chain.
//!
//! What verification proves: files are byte-identical to what was captured, the capture / decision events are in the
//! audit chain up to the checkpoint, and the checkpoint was signed by key X at time T. What it does **not** prove: that
//! any document's content is true (directive §9). The report states both, in English and Japanese.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{Read, Seek, Write};

use audit_engine::{Event, canonical_json, chain_hash, checkpoint, merkle};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const FORMAT: &str = "kizuna-evidence-pack";
pub const VERSION: u32 = 1;
pub const MANIFEST: &str = "manifest.json";
pub const SIGNATURE: &str = "manifest.sig.json";
pub const DERIVATION: &str = "decision/derivation.json";
const EXPORT_DOMAIN: &[u8] = b"KZEP1";
const CAPTURE_ACTIONS: [&str; 2] = ["artifact.uploaded", "artifact.recaptured"];

/// Untrusted-input limits for the verifier (zip bombs, oversized manifests, absurd proofs).
pub const MAX_ENTRIES: usize = 10_000;
pub const MAX_JSON_BYTES: u64 = 64 << 20;
pub const MAX_FILE_BYTES: u64 = 4 << 30;
pub const MAX_EVENTS: usize = 100_000;
pub const MAX_PROOF_LEN: usize = 64;

// ---------------------------------------------------------------------------------------------------------------
// Manifest (canonical JSON, versioned)

/// 32 bytes as 64 lowercase hex characters.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct H32(pub [u8; 32]);
/// 64 bytes as 128 lowercase hex characters.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct H64(pub [u8; 64]);

impl fmt::Debug for H32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.0))
    }
}
impl fmt::Debug for H64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.0))
    }
}

fn strict_hex<const N: usize, E: serde::de::Error>(s: &str) -> Result<[u8; N], E> {
    if s.len() != 2 * N || !s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err(E::custom(format!("expected {} lowercase hex characters", 2 * N)));
    }
    let v = hex::decode(s).map_err(E::custom)?;
    v.try_into().map_err(|_| E::custom("length"))
}

impl Serialize for H32 {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(self.0))
    }
}
impl<'de> Deserialize<'de> for H32 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        strict_hex::<32, _>(&s).map(H32)
    }
}
impl Serialize for H64 {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(self.0))
    }
}
impl<'de> Deserialize<'de> for H64 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        strict_hex::<64, _>(&s).map(H64)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: String,
    pub version: u32,
    pub generated_at_us: i64,
    pub tenant_id: Uuid,
    pub subject: Subject,
    /// Every ZIP entry except the manifest and its signature, sorted by path.
    pub files: Vec<FileEntry>,
    pub artifacts: Vec<ArtifactEntry>,
    pub events: Vec<EventEntry>,
    pub checkpoint: CheckpointEntry,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Subject {
    /// `decision` (the verification report of the decision is included as `decision/report.json`).
    pub kind: String,
    pub id: Uuid,
    pub derivation_sha256: H32,
    /// Sequence number of the `decision.recorded` audit event.
    pub recorded_event_seq: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FileEntry {
    pub path: String,
    pub sha256: H32,
    pub size: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ArtifactEntry {
    pub id: Uuid,
    pub name: String,
    pub media_type: String,
    pub sha256: H32,
    pub size: u64,
    /// False when only the hash is exported (redaction, or a document that was never released from quarantine).
    pub included: bool,
    /// `requested` (hash-only on request) or `not_released`; null when included.
    pub excluded_reason: Option<String>,
    pub path: Option<String>,
    /// The `artifact.uploaded` / `artifact.recaptured` audit event that recorded this SHA-256.
    pub capture_event_seq: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EventEntry {
    pub seq: i64,
    pub occurred_at_us: i64,
    pub actor_user_id: Option<Uuid>,
    pub actor_kind: String,
    pub action: String,
    pub target_type: String,
    pub target_id: Option<String>,
    pub details: Value,
    pub prev_hash: H32,
    pub hash: H32,
    /// RFC 6962 audit path of leaf `seq - 1` in the checkpoint's Merkle tree.
    pub proof: Vec<H32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CheckpointEntry {
    /// Always `KZCP3` (Merkle root signed).
    pub message_version: String,
    pub seq: i64,
    pub head_hash: H32,
    pub signed_at_us: i64,
    pub prev_signature_sha256: H32,
    pub merkle_root: H32,
    pub key_id: String,
    pub public_key: H32,
    pub signature: H64,
    /// Audit path showing that the head hash is the last leaf (`seq - 1`) of the tree.
    pub head_proof: Vec<H32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExportSignature {
    pub algorithm: String,
    pub key_id: String,
    pub public_key: H32,
    pub signature: H64,
    pub signed: String,
}

impl EventEntry {
    pub fn event(&self, tenant_id: Uuid) -> Event {
        Event {
            tenant_id,
            seq: self.seq,
            occurred_at_us: self.occurred_at_us,
            actor_user_id: self.actor_user_id,
            actor_kind: self.actor_kind.clone(),
            action: self.action.clone(),
            target_type: self.target_type.clone(),
            target_id: self.target_id.clone(),
            details: self.details.clone(),
        }
    }
}

impl CheckpointEntry {
    pub fn message(&self, tenant: Uuid) -> Vec<u8> {
        checkpoint::message_v3(
            tenant,
            self.seq,
            &self.head_hash.0,
            self.signed_at_us,
            &self.prev_signature_sha256.0,
            &self.merkle_root.0,
        )
    }
}

/// Key id as used by the KIZUNA API: first 8 bytes of SHA-256(public key), hex.
pub fn key_id(public: &[u8; 32]) -> String {
    hex::encode(&Sha256::digest(public)[..8])
}

fn sha256(b: &[u8]) -> [u8; 32] {
    Sha256::digest(b).into()
}

fn export_message(manifest: &[u8]) -> Vec<u8> {
    [EXPORT_DOMAIN, &sha256(manifest)].concat()
}

/// Canonical JSON bytes of any serialisable value (sorted keys, no whitespace, integers only).
pub fn to_canonical<T: Serialize>(v: &T) -> Result<Vec<u8>, String> {
    let v = serde_json::to_value(v).map_err(|e| e.to_string())?;
    canonical_json(&v).map(String::into_bytes).map_err(|e| e.to_string())
}

/// Parses a manifest: must be valid, of a supported format/version and byte-for-byte canonical. Never panics.
pub fn parse_manifest(bytes: &[u8]) -> Result<Manifest, Msg> {
    let m: Manifest = serde_json::from_slice(bytes).map_err(|e| {
        Msg::new(
            format!("manifest.json is not a valid manifest: {e}"),
            format!("manifest.json がマニフェストとして不正です: {e}"),
        )
    })?;
    if m.format != FORMAT || m.version != VERSION {
        return Err(Msg::new(
            format!(
                "unsupported pack format {} version {} (this verifier reads {FORMAT} version {VERSION})",
                m.format, m.version
            ),
            format!(
                "未対応のパック形式です: {} バージョン {}（対応: {FORMAT} バージョン {VERSION}）",
                m.format, m.version
            ),
        ));
    }
    if m.events.len() > MAX_EVENTS
        || m.events.iter().any(|e| e.proof.len() > MAX_PROOF_LEN)
        || m.checkpoint.head_proof.len() > MAX_PROOF_LEN
        || m.files.len() > MAX_ENTRIES
        || m.artifacts.len() > MAX_ENTRIES
    {
        return Err(Msg::new(
            "manifest exceeds the verifier's size limits",
            "マニフェストが検証ツールの上限を超えています",
        ));
    }
    match to_canonical(&m) {
        Ok(c) if c == bytes => Ok(m),
        _ => Err(Msg::new(
            "manifest.json is not in canonical form (it was edited or re-serialised)",
            "manifest.json が正規形ではありません（編集または再シリアライズされています）",
        )),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Builder (used by the API)

/// Inputs for [`build`]. Artifact `path` fields are filled in by the builder.
pub struct BuildInput {
    pub generated_at_us: i64,
    pub tenant_id: Uuid,
    pub subject: Subject,
    /// Each artifact with its bytes, or `None` for hash-only.
    pub artifacts: Vec<(ArtifactEntry, Option<Vec<u8>>)>,
    pub events: Vec<EventEntry>,
    pub checkpoint: CheckpointEntry,
    /// Further documents, e.g. `decision/record.json`. Must include [`DERIVATION`] (canonical JSON).
    pub documents: Vec<(String, Vec<u8>)>,
}

/// Safe single path component from a display name (letters/digits of any script, `.`, `-`, `_`).
pub fn safe_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .take(100)
        .collect();
    let s = s.trim_start_matches('.').to_string();
    if s.is_empty() { "document".into() } else { s }
}

const README: &str = "KIZUNA evidence pack / KIZUNA 証拠パック\n\n\
Verify offline / オフライン検証:\n  kizuna-verify-pack <this.zip> [--trusted-key <public key hex or key id>] [--lang en|ja]\n\n\
Proven when verification passes: each included document is byte-identical to the document captured at the recorded\n\
time; the capture and decision events are in the tenant's audit chain up to a checkpoint signed by the key shown.\n\
NOT proven: that any document's content is true, accurate or authentic. A hash proves only that the bytes are\n\
unchanged since capture.\n\n\
検証で証明されること: 同梱文書が記録時刻に取得された文書とバイト単位で同一であること、取得・判断イベントが\n\
表示された鍵で署名されたチェックポイントまでの監査チェーンに含まれていること。\n\
証明されないこと: 文書の内容が真実・正確・真正であること。ハッシュが示すのは取得時から変更されていないことだけです。\n";

/// Writes the pack. `seed` is the Ed25519 signing key whose public key signed the checkpoint.
pub fn build(mut input: BuildInput, seed: &[u8; 32]) -> Result<Vec<u8>, String> {
    let public = checkpoint::public_key(seed);
    if public != input.checkpoint.public_key.0 {
        return Err("export key differs from the checkpoint key".into());
    }
    let mut entries: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for (path, bytes) in input.documents.drain(..) {
        entries.insert(path, bytes);
    }
    if !entries.contains_key(DERIVATION) {
        return Err("derivation document missing".into());
    }
    entries.insert("README.txt".into(), README.as_bytes().to_vec());
    let mut artifacts = Vec::new();
    for (mut a, bytes) in input.artifacts.drain(..) {
        match bytes {
            Some(b) => {
                if sha256(&b) != a.sha256.0 {
                    return Err(format!("artifact {} bytes do not match its hash", a.id));
                }
                let path = format!("artifacts/{}/{}", a.id, safe_name(&a.name));
                a.included = true;
                a.excluded_reason = None;
                a.path = Some(path.clone());
                entries.insert(path, b);
            }
            None => {
                a.included = false;
                a.path = None;
                if a.excluded_reason.is_none() {
                    a.excluded_reason = Some("requested".into());
                }
            }
        }
        artifacts.push(a);
    }
    artifacts.sort_by_key(|a| a.id);
    input.events.sort_by_key(|e| e.seq);
    let manifest = Manifest {
        format: FORMAT.into(),
        version: VERSION,
        generated_at_us: input.generated_at_us,
        tenant_id: input.tenant_id,
        subject: input.subject,
        files: entries
            .iter()
            .map(|(p, b)| FileEntry {
                path: p.clone(),
                sha256: H32(sha256(b)),
                size: b.len() as u64,
            })
            .collect(),
        artifacts,
        events: input.events,
        checkpoint: input.checkpoint,
    };
    let mbytes = to_canonical(&manifest)?;
    let sig = ExportSignature {
        algorithm: "ed25519".into(),
        key_id: key_id(&public),
        public_key: H32(public),
        signature: H64(checkpoint::sign(seed, &export_message(&mbytes))),
        signed: "\"KZEP1\" || SHA-256(manifest.json)".into(),
    };
    let sbytes = to_canonical(&sig)?;
    let mut zw = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .large_file(true);
    for (name, b) in [(MANIFEST.to_string(), &mbytes), (SIGNATURE.to_string(), &sbytes)]
        .into_iter()
        .chain(entries.iter().map(|(p, b)| (p.clone(), b)))
    {
        zw.start_file(name, opts).map_err(|e| e.to_string())?;
        zw.write_all(b).map_err(|e| e.to_string())?;
    }
    Ok(zw.finish().map_err(|e| e.to_string())?.into_inner())
}

// ---------------------------------------------------------------------------------------------------------------
// Verifier

/// A bilingual message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Msg {
    pub en: String,
    pub ja: String,
}

impl Msg {
    pub fn new(en: impl Into<String>, ja: impl Into<String>) -> Self {
        Msg {
            en: en.into(),
            ja: ja.into(),
        }
    }
}

/// One verification step. `code` is stable (tests and tooling match on it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    pub code: &'static str,
    pub ok: bool,
    pub msg: Msg,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Report {
    pub ok: bool,
    pub checks: Vec<Check>,
    pub proven: Vec<Msg>,
    pub not_proven: Vec<Msg>,
}

impl Report {
    fn pass(&mut self, code: &'static str, msg: Msg) {
        self.checks.push(Check { code, ok: true, msg });
    }
    fn fail(&mut self, code: &'static str, msg: Msg) {
        self.checks.push(Check { code, ok: false, msg });
    }
    fn check(&mut self, ok: bool, code: &'static str, msg: Msg) -> bool {
        if ok {
            self.pass(code, msg)
        } else {
            self.fail(code, msg)
        }
        ok
    }
    pub fn failures(&self) -> impl Iterator<Item = &Check> {
        self.checks.iter().filter(|c| !c.ok)
    }
    /// True when a failed check has this code.
    pub fn failed(&self, code: &str) -> bool {
        self.failures().any(|c| c.code == code)
    }
}

#[derive(Debug, Clone, Default)]
pub struct VerifyOptions {
    /// Expected signing key: a 64-hex public key or a 16-hex key id. Without it the key is reported, not trusted.
    pub trusted_key: Option<String>,
}

fn ts(us: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(us) * 1000)
        .ok()
        .and_then(|t| t.format(&time::format_description::well_known::Rfc3339).ok())
        .unwrap_or_else(|| format!("{us} µs"))
}

/// Strict entry path: relative, `/`-separated, no empty / `.` / `..` components, no backslash, colon or control chars.
fn path_ok(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 512
        && !p.starts_with('/')
        && !p.chars().any(|c| c.is_control() || c == '\\' || c == ':')
        && p.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
}

struct Entry {
    sha256: [u8; 32],
    size: u64,
    /// Kept only for the small JSON documents the verifier reads.
    bytes: Option<Vec<u8>>,
}

fn read_entries<R: Read + Seek>(r: R, rep: &mut Report) -> Option<BTreeMap<String, Entry>> {
    let mut za = match zip::ZipArchive::new(r) {
        Ok(z) => z,
        Err(e) => {
            rep.fail(
                "zip_unreadable",
                Msg::new(
                    format!("the file is not a readable ZIP archive: {e}"),
                    format!("ZIP として読み取れません: {e}"),
                ),
            );
            return None;
        }
    };
    if za.len() > MAX_ENTRIES {
        rep.fail(
            "zip_too_many_entries",
            Msg::new(
                format!("the pack has {} entries (limit {MAX_ENTRIES})", za.len()),
                format!("パックのエントリ数 {} が上限 {MAX_ENTRIES} を超えています", za.len()),
            ),
        );
        return None;
    }
    let mut out = BTreeMap::new();
    for i in 0..za.len() {
        let mut f = match za.by_index(i) {
            Ok(f) => f,
            Err(e) => {
                rep.fail(
                    "zip_unreadable",
                    Msg::new(
                        format!("ZIP entry {i} cannot be read: {e}"),
                        format!("ZIP エントリ {i} を読み取れません: {e}"),
                    ),
                );
                return None;
            }
        };
        let name = match f.name() {
            Ok(n) => n.into_owned(),
            Err(_) => {
                rep.fail(
                    "zip_bad_path",
                    Msg::new(
                        format!("ZIP entry {i} has an invalid name"),
                        format!("ZIP エントリ {i} の名前が不正です"),
                    ),
                );
                return None;
            }
        };
        if f.is_dir() {
            continue;
        }
        if !path_ok(&name) {
            rep.fail(
                "zip_bad_path",
                Msg::new(
                    format!("unsafe path in pack: {name:?}"),
                    format!("パック内に安全でないパスがあります: {name:?}"),
                ),
            );
            return None;
        }
        let small = name.ends_with(".json");
        let limit = if small { MAX_JSON_BYTES } else { MAX_FILE_BYTES };
        if f.size() > limit {
            rep.fail(
                "zip_entry_too_large",
                Msg::new(
                    format!("{name} declares {} bytes (limit {limit})", f.size()),
                    format!("{name} のサイズ {} が上限 {limit} を超えています", f.size()),
                ),
            );
            return None;
        }
        // Never trust the declared size: read at most limit + 1 bytes.
        let mut h = Sha256::new();
        let mut keep = small.then(Vec::new);
        let mut buf = vec![0u8; 64 * 1024];
        let mut size = 0u64;
        let mut lim = (&mut f).take(limit + 1);
        loop {
            match lim.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    h.update(&buf[..n]);
                    if let Some(k) = keep.as_mut() {
                        k.extend_from_slice(&buf[..n]);
                    }
                    size += n as u64;
                }
                Err(e) => {
                    rep.fail(
                        "zip_unreadable",
                        Msg::new(
                            format!("{name} cannot be decompressed: {e}"),
                            format!("{name} を展開できません: {e}"),
                        ),
                    );
                    return None;
                }
            }
        }
        if size > limit {
            rep.fail(
                "zip_entry_too_large",
                Msg::new(
                    format!("{name} expands beyond {limit} bytes"),
                    format!("{name} の展開後サイズが上限 {limit} を超えています"),
                ),
            );
            return None;
        }
        let e = Entry {
            sha256: h.finalize().into(),
            size,
            bytes: keep,
        };
        if out.insert(name.clone(), e).is_some() {
            rep.fail(
                "zip_duplicate_entry",
                Msg::new(
                    format!("{name} appears more than once in the pack"),
                    format!("{name} がパック内に重複しています"),
                ),
            );
            return None;
        }
    }
    Some(out)
}

/// Verifies a pack from any seekable reader (a file or an in-memory buffer). Every check runs and is reported.
pub fn verify<R: Read + Seek>(r: R, opts: &VerifyOptions) -> Report {
    let mut rep = Report::default();
    run(r, opts, &mut rep);
    rep.ok = !rep.checks.is_empty() && rep.checks.iter().all(|c| c.ok);
    rep.not_proven.insert(
        0,
        Msg::new(
            "That any document's content is true, accurate, complete or authentic. A hash proves only that the bytes are unchanged since capture (directive §9).",
            "文書の内容が真実・正確・完全・真正であること。ハッシュが示すのは、取得時からバイト列が変更されていないことだけです（指令 §9）。",
        ),
    );
    rep.not_proven.insert(
        1,
        Msg::new(
            "Who wrote or issued a document, or that it had not already been altered before it was captured.",
            "文書の作成者・発行者、および取得前に改変されていなかったこと。",
        ),
    );
    if !rep.ok {
        rep.proven.clear();
    }
    rep
}

fn run<R: Read + Seek>(r: R, opts: &VerifyOptions, rep: &mut Report) {
    let Some(entries) = read_entries(r, rep) else { return };
    let Some(mbytes) = entries.get(MANIFEST).and_then(|e| e.bytes.clone()) else {
        rep.fail(
            "manifest_missing",
            Msg::new("manifest.json is missing", "manifest.json がありません"),
        );
        return;
    };
    let m = match parse_manifest(&mbytes) {
        Ok(m) => m,
        Err(msg) => {
            let code = if msg.en.contains("canonical") {
                "manifest_not_canonical"
            } else if msg.en.contains("unsupported") {
                "manifest_unsupported"
            } else {
                "manifest_invalid"
            };
            rep.fail(code, msg);
            return;
        }
    };
    rep.pass(
        "manifest",
        Msg::new(
            format!("manifest.json is a canonical {FORMAT} v{VERSION} manifest"),
            format!("manifest.json は正規形の {FORMAT} v{VERSION} マニフェストです"),
        ),
    );
    let cp = &m.checkpoint;
    let kid_short = &cp.key_id;

    // Signing key: key id, optional pin.
    let kid_ok = key_id(&cp.public_key.0) == cp.key_id;
    rep.check(
        kid_ok,
        "key_id_mismatch",
        if kid_ok {
            Msg::new(
                format!("checkpoint key id {} matches its public key", cp.key_id),
                format!("チェックポイントの鍵ID {} は公開鍵と一致します", cp.key_id),
            )
        } else {
            Msg::new(
                format!(
                    "checkpoint key id {} does not belong to public key {}",
                    cp.key_id,
                    hex::encode(cp.public_key.0)
                ),
                format!(
                    "チェックポイントの鍵ID {} は公開鍵 {} のものではありません",
                    cp.key_id,
                    hex::encode(cp.public_key.0)
                ),
            )
        },
    );
    let pinned = match opts.trusted_key.as_deref().map(str::to_ascii_lowercase) {
        None => false,
        Some(t) => {
            let ok = t == hex::encode(cp.public_key.0) || t == key_id(&cp.public_key.0);
            rep.check(
                ok,
                "trusted_key_mismatch",
                if ok {
                    Msg::new(
                        format!("signing key {kid_short} is the trusted key"),
                        format!("署名鍵 {kid_short} は信頼済みの鍵です"),
                    )
                } else {
                    Msg::new(
                        format!(
                            "pack is signed by key {} ({}), not by the trusted key {t}",
                            kid_short,
                            hex::encode(cp.public_key.0)
                        ),
                        format!(
                            "パックの署名鍵は {}（{}）で、信頼済みの鍵 {t} ではありません",
                            kid_short,
                            hex::encode(cp.public_key.0)
                        ),
                    )
                },
            );
            ok
        }
    };

    // Export signature over the manifest.
    match entries
        .get(SIGNATURE)
        .and_then(|e| e.bytes.as_deref())
        .map(serde_json::from_slice::<ExportSignature>)
    {
        None => rep.fail(
            "export_signature_missing",
            Msg::new("manifest.sig.json is missing", "manifest.sig.json がありません"),
        ),
        Some(Err(e)) => rep.fail(
            "export_signature_invalid",
            Msg::new(
                format!("manifest.sig.json is malformed: {e}"),
                format!("manifest.sig.json が不正です: {e}"),
            ),
        ),
        Some(Ok(s)) => {
            if s.public_key != cp.public_key {
                rep.fail(
                    "export_key_mismatch",
                    Msg::new(
                        format!(
                            "manifest was signed by key {} but the checkpoint by key {}",
                            key_id(&s.public_key.0),
                            kid_short
                        ),
                        format!(
                            "マニフェストの署名鍵 {} がチェックポイントの署名鍵 {} と異なります",
                            key_id(&s.public_key.0),
                            kid_short
                        ),
                    ),
                );
            } else {
                let ok = checkpoint::verify(&s.public_key.0, &export_message(&mbytes), &s.signature.0);
                rep.check(
                    ok,
                    "export_signature_invalid",
                    if ok {
                        Msg::new(format!("manifest is signed by key {kid_short}"), format!("マニフェストは鍵 {kid_short} で署名されています"))
                    } else {
                        Msg::new(
                            "manifest signature is invalid: the manifest was changed after export or signed with another key",
                            "マニフェスト署名が無効です：エクスポート後にマニフェストが変更されたか、別の鍵で署名されています",
                        )
                    },
                );
            }
        }
    }

    // Checkpoint signature (KZCP3) and head-in-tree.
    let size = u64::try_from(cp.seq).unwrap_or(0);
    let cp_sig_ok = cp.message_version == "KZCP3"
        && size > 0
        && checkpoint::verify(&cp.public_key.0, &cp.message(m.tenant_id), &cp.signature.0);
    rep.check(
        cp_sig_ok,
        "checkpoint_signature_invalid",
        if cp_sig_ok {
            Msg::new(
                format!("checkpoint #{} signed by key {kid_short} at {}", cp.seq, ts(cp.signed_at_us)),
                format!("チェックポイント #{} は鍵 {kid_short} により {} に署名されています", cp.seq, ts(cp.signed_at_us)),
            )
        } else {
            Msg::new(
                format!("checkpoint #{} signature is invalid (checkpoint fields, Merkle root or key were changed)", cp.seq),
                format!("チェックポイント #{} の署名が無効です（チェックポイントの項目・Merkle ルート・鍵が変更されています）", cp.seq),
            )
        },
    );
    let head_ok = size > 0
        && merkle::verify(
            &cp.merkle_root.0,
            size,
            size - 1,
            &cp.head_hash.0,
            &h32s(&cp.head_proof),
        )
        .is_ok();
    rep.check(
        head_ok,
        "checkpoint_head_not_in_tree",
        if head_ok {
            Msg::new(
                "checkpoint head hash is the last leaf of its Merkle tree",
                "チェックポイントの先頭ハッシュは Merkle 木の最後の葉です",
            )
        } else {
            Msg::new(
                "checkpoint head hash is not the last leaf of the signed Merkle tree",
                "チェックポイントの先頭ハッシュが署名済み Merkle 木の最後の葉ではありません",
            )
        },
    );

    // Files: every listed file present with matching hash and size; nothing unlisted.
    let mut files_ok = BTreeMap::new();
    for f in &m.files {
        let ok = match entries.get(&f.path) {
            None => {
                rep.fail(
                    "file_missing",
                    Msg::new(
                        format!("{} is listed in the manifest but missing from the pack", f.path),
                        format!("{} はマニフェストにありますがパックにありません", f.path),
                    ),
                );
                false
            }
            Some(e) if e.sha256 != f.sha256.0 => {
                rep.fail(
                    "file_hash_mismatch",
                    Msg::new(
                        format!(
                            "{} was modified: SHA-256 {} differs from the manifest's {}",
                            f.path,
                            hex::encode(e.sha256),
                            hex::encode(f.sha256.0)
                        ),
                        format!(
                            "{} が改変されています：SHA-256 {} がマニフェストの {} と異なります",
                            f.path,
                            hex::encode(e.sha256),
                            hex::encode(f.sha256.0)
                        ),
                    ),
                );
                false
            }
            Some(e) if e.size != f.size => {
                rep.fail(
                    "file_size_mismatch",
                    Msg::new(
                        format!("{} has {} bytes, the manifest says {}", f.path, e.size, f.size),
                        format!(
                            "{} のサイズ {} がマニフェストの {} と異なります",
                            f.path, e.size, f.size
                        ),
                    ),
                );
                false
            }
            Some(_) => true,
        };
        files_ok.insert(f.path.as_str(), ok);
    }
    let listed: BTreeSet<&str> = m.files.iter().map(|f| f.path.as_str()).collect();
    for name in entries.keys() {
        if name != MANIFEST && name != SIGNATURE && !listed.contains(name.as_str()) {
            rep.fail(
                "zip_unlisted_entry",
                Msg::new(
                    format!("{name} is in the pack but not in the manifest"),
                    format!("{name} はパックにありますがマニフェストにありません"),
                ),
            );
        }
    }
    if files_ok.values().all(|ok| *ok) {
        rep.pass(
            "files",
            Msg::new(
                format!("all {} listed files match the manifest", m.files.len()),
                format!(
                    "記載された {} 件のファイルはすべてマニフェストと一致します",
                    m.files.len()
                ),
            ),
        );
    }

    // Audit events: content hash, chain link to the checkpoint's Merkle tree.
    let mut good_events: BTreeMap<i64, &EventEntry> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for e in &m.events {
        if !seen.insert(e.seq) {
            rep.fail(
                "event_duplicate",
                Msg::new(
                    format!("audit event #{} appears twice", e.seq),
                    format!("監査イベント #{} が重複しています", e.seq),
                ),
            );
            continue;
        }
        let hash_ok = chain_hash(&e.event(m.tenant_id), &e.prev_hash.0).is_ok_and(|h| h == e.hash.0);
        if !hash_ok {
            rep.fail(
                "event_hash_mismatch",
                Msg::new(
                    format!(
                        "audit event #{} ({}) was changed: its content does not match its chain hash",
                        e.seq, e.action
                    ),
                    format!(
                        "監査イベント #{}（{}）が改変されています：内容がチェーンハッシュと一致しません",
                        e.seq, e.action
                    ),
                ),
            );
            continue;
        }
        if e.seq < 1 || e.seq > cp.seq {
            rep.fail(
                "event_after_checkpoint",
                Msg::new(
                    format!(
                        "audit event #{} is outside the checkpointed chain (1..={})",
                        e.seq, cp.seq
                    ),
                    format!(
                        "監査イベント #{} はチェックポイント範囲（1..={}）の外です",
                        e.seq, cp.seq
                    ),
                ),
            );
            continue;
        }
        let idx = u64::try_from(e.seq - 1).unwrap_or(u64::MAX);
        match merkle::verify(&cp.merkle_root.0, size, idx, &e.hash.0, &h32s(&e.proof)) {
            Ok(()) => {
                good_events.insert(e.seq, e);
            }
            Err(merkle::ProofError::WrongLength) => rep.fail("event_proof_truncated", Msg::new(format!("Merkle proof of audit event #{} has {} nodes, a tree of {} events needs a different number (proof truncated or padded)", e.seq, e.proof.len(), cp.seq), format!("監査イベント #{} の Merkle 証明のノード数 {} が {} 件の木に合いません（切り詰めまたは追加）", e.seq, e.proof.len(), cp.seq))),
            Err(_) => rep.fail("event_proof_invalid", Msg::new(format!("Merkle proof of audit event #{} does not lead to the signed checkpoint root", e.seq), format!("監査イベント #{} の Merkle 証明が署名済みチェックポイントのルートに到達しません", e.seq))),
        }
    }
    if !m.events.is_empty() && good_events.len() == m.events.len() {
        rep.pass(
            "events",
            Msg::new(
                format!(
                    "all {} audit events are unchanged and included under checkpoint #{}",
                    m.events.len(),
                    cp.seq
                ),
                format!(
                    "{} 件の監査イベントはすべて未改変で、チェックポイント #{} に含まれています",
                    m.events.len(),
                    cp.seq
                ),
            ),
        );
    }

    // Decision: derivation document ↔ hash ↔ decision.recorded event.
    let s = &m.subject;
    let derivation_ok = match entries.get(DERIVATION).and_then(|e| e.bytes.as_deref()) {
        None => {
            rep.fail(
                "derivation_missing",
                Msg::new(
                    "decision/derivation.json is missing",
                    "decision/derivation.json がありません",
                ),
            );
            false
        }
        Some(b) => {
            let ok = sha256(b) == s.derivation_sha256.0;
            rep.check(
                ok,
                "derivation_hash_mismatch",
                if ok {
                    Msg::new(
                        format!(
                            "decision derivation hashes to the recorded {}",
                            hex::encode(s.derivation_sha256.0)
                        ),
                        format!(
                            "判断の導出内容は記録済みハッシュ {} と一致します",
                            hex::encode(s.derivation_sha256.0)
                        ),
                    )
                } else {
                    Msg::new(
                        "decision/derivation.json does not match the derivation hash recorded at decision time",
                        "decision/derivation.json が判断時に記録された導出ハッシュと一致しません",
                    )
                },
            )
        }
    };
    let rec = good_events.get(&s.recorded_event_seq);
    let rec_ok = rec.is_some_and(|e| {
        e.action == "decision.recorded"
            && e.target_type == "decision"
            && e.target_id.as_deref() == Some(s.id.to_string().as_str())
            && e.details["derivation_sha256"].as_str() == Some(hex::encode(s.derivation_sha256.0).as_str())
    });
    if rec.is_none() {
        rep.fail(
            "decision_event_missing",
            Msg::new(
                format!(
                    "the decision.recorded audit event #{} is not in the pack (or failed verification)",
                    s.recorded_event_seq
                ),
                format!(
                    "判断記録の監査イベント #{} がパックにありません（または検証に失敗）",
                    s.recorded_event_seq
                ),
            ),
        );
    } else {
        rep.check(
            rec_ok,
            "decision_event_mismatch",
            if rec_ok {
                Msg::new(
                    format!(
                        "decision {} was recorded in audit event #{}",
                        s.id, s.recorded_event_seq
                    ),
                    format!(
                        "判断 {} は監査イベント #{} に記録されています",
                        s.id, s.recorded_event_seq
                    ),
                )
            } else {
                Msg::new(
                    format!(
                        "audit event #{} does not record decision {} with this derivation hash",
                        s.recorded_event_seq, s.id
                    ),
                    format!(
                        "監査イベント #{} は判断 {} をこの導出ハッシュで記録していません",
                        s.recorded_event_seq, s.id
                    ),
                )
            },
        );
    }

    // Artifacts: capture event ↔ hash ↔ included bytes.
    let derivation: Value = entries
        .get(DERIVATION)
        .and_then(|e| e.bytes.as_deref())
        .and_then(|b| serde_json::from_slice(b).ok())
        .unwrap_or(Value::Null);
    let by_id: BTreeMap<String, &ArtifactEntry> = m.artifacts.iter().map(|a| (a.id.to_string(), a)).collect();
    let mut art_proven = Vec::new();
    for a in &m.artifacts {
        let sha = hex::encode(a.sha256.0);
        let Some(ev) = good_events.get(&a.capture_event_seq) else {
            rep.fail(
                "artifact_capture_event_missing",
                Msg::new(
                    format!(
                        "document {:?}: its capture audit event #{} is not in the pack (or failed verification)",
                        a.name, a.capture_event_seq
                    ),
                    format!(
                        "文書 {:?}：取得時の監査イベント #{} がパックにありません（または検証に失敗）",
                        a.name, a.capture_event_seq
                    ),
                ),
            );
            continue;
        };
        let ev_ok = CAPTURE_ACTIONS.contains(&ev.action.as_str())
            && ev.target_id.as_deref() == Some(a.id.to_string().as_str())
            && ev.details["sha256"].as_str() == Some(sha.as_str());
        if !ev_ok {
            rep.fail(
                "artifact_capture_event_mismatch",
                Msg::new(
                    format!(
                        "document {:?}: audit event #{} did not capture SHA-256 {sha} for artifact {}",
                        a.name, a.capture_event_seq, a.id
                    ),
                    format!(
                        "文書 {:?}：監査イベント #{} は成果物 {} の SHA-256 {sha} を記録していません",
                        a.name, a.capture_event_seq, a.id
                    ),
                ),
            );
            continue;
        }
        let file_ok = match (a.included, a.path.as_deref()) {
            (true, Some(p)) => match entries.get(p) {
                Some(e) if e.sha256 == a.sha256.0 && listed.contains(p) => true,
                Some(e) => {
                    rep.fail(
                        "artifact_file_mismatch",
                        Msg::new(
                            format!(
                                "document {:?} ({p}) was modified: SHA-256 {} differs from the captured {sha}",
                                a.name,
                                hex::encode(e.sha256)
                            ),
                            format!(
                                "文書 {:?}（{p}）が改変されています：SHA-256 {} が取得時の {sha} と異なります",
                                a.name,
                                hex::encode(e.sha256)
                            ),
                        ),
                    );
                    false
                }
                None => {
                    rep.fail(
                        "artifact_file_mismatch",
                        Msg::new(
                            format!("document {:?} is marked included but {p} is missing", a.name),
                            format!("文書 {:?} は同梱扱いですが {p} がありません", a.name),
                        ),
                    );
                    false
                }
            },
            (false, None) => true,
            _ => {
                rep.fail(
                    "artifact_path_inconsistent",
                    Msg::new(
                        format!("document {:?}: 'included' and 'path' disagree", a.name),
                        format!("文書 {:?}：included と path が矛盾しています", a.name),
                    ),
                );
                false
            }
        };
        if file_ok {
            art_proven.push(if a.included {
                Msg::new(
                    format!("Document {:?} ({}) is byte-identical to the document captured at {} (audit event #{}, SHA-256 {sha}).", a.name, a.path.as_deref().unwrap_or(""), ts(ev.occurred_at_us), ev.seq),
                    format!("文書 {:?}（{}）は {} に取得された文書とバイト単位で同一です（監査イベント #{}、SHA-256 {sha}）。", a.name, a.path.as_deref().unwrap_or(""), ts(ev.occurred_at_us), ev.seq),
                )
            } else {
                Msg::new(
                    format!("Document {:?}: SHA-256 {sha} was recorded in the audit chain at {} (audit event #{}). The file is not in this pack (hash only).", a.name, ts(ev.occurred_at_us), ev.seq),
                    format!("文書 {:?}：SHA-256 {sha} は {} に監査チェーンへ記録されています（監査イベント #{}）。ファイルは同梱されていません（ハッシュのみ）。", a.name, ts(ev.occurred_at_us), ev.seq),
                )
            });
        }
    }
    // Trace: every document the derivation cites must be in the pack with the same hash.
    let mut trace_lines = 0usize;
    for ev in derivation["evidence"].as_array().into_iter().flatten() {
        trace_lines += 1;
        let Some(aid) = ev["artifact_id"].as_str() else {
            continue;
        };
        match by_id.get(aid) {
            Some(a)
                if ev["artifact_sha256"]
                    .as_str()
                    .is_none_or(|h| h == hex::encode(a.sha256.0)) => {}
            Some(_) => rep.fail(
                "trace_artifact_mismatch",
                Msg::new(
                    format!("derivation cites artifact {aid} with a different hash than the pack"),
                    format!("導出内容が引用する成果物 {aid} のハッシュがパックと異なります"),
                ),
            ),
            None => rep.fail(
                "trace_artifact_missing",
                Msg::new(
                    format!("derivation cites artifact {aid}, which is not in the pack"),
                    format!("導出内容が引用する成果物 {aid} がパックにありません"),
                ),
            ),
        }
    }

    // Proven / not proven.
    if cp_sig_ok {
        rep.proven.push(Msg::new(
            format!("Checkpoint #{} of tenant {}'s audit chain was signed with Ed25519 key {kid_short} at {}{}.", cp.seq, m.tenant_id, ts(cp.signed_at_us), if pinned { " (the key you trust)" } else { "" }),
            format!("テナント {} の監査チェーンのチェックポイント #{} は、Ed25519 鍵 {kid_short} により {} に署名されています{}。", m.tenant_id, cp.seq, ts(cp.signed_at_us), if pinned { "（信頼済みの鍵）" } else { "" }),
        ));
    }
    for e in good_events.values() {
        rep.proven.push(Msg::new(
            format!(
                "Audit event #{} ({}, {}) is unchanged and included in the audit chain up to checkpoint #{}.",
                e.seq,
                e.action,
                ts(e.occurred_at_us),
                cp.seq
            ),
            format!(
                "監査イベント #{}（{}、{}）は未改変で、チェックポイント #{} までの監査チェーンに含まれています。",
                e.seq,
                e.action,
                ts(e.occurred_at_us),
                cp.seq
            ),
        ));
    }
    if rec_ok && derivation_ok {
        rep.proven.push(Msg::new(
            format!("The decision's derivation (claims → source locations, {trace_lines} items) is exactly the one recorded at decision time."),
            format!("判断の導出内容（主張 → 出典箇所、{trace_lines} 件）は判断時に記録されたものと完全に同一です。"),
        ));
    }
    rep.proven.extend(art_proven);
    if !pinned {
        rep.not_proven.push(Msg::new(
            format!("That key {kid_short} ({}) belongs to the tenant: confirm the key id through a separate channel, or rerun with --trusted-key.", hex::encode(cp.public_key.0)),
            format!("鍵 {kid_short}（{}）がテナントの鍵であること：別経路で鍵IDを確認するか、--trusted-key を指定して再実行してください。", hex::encode(cp.public_key.0)),
        ));
    }
    if m.artifacts.iter().any(|a| !a.included) {
        rep.not_proven.push(Msg::new(
            "The content of hash-only documents: their files are not in this pack, so only the recorded hash can be shown.",
            "ハッシュのみの文書の内容：ファイルが同梱されていないため、記録されたハッシュしか示せません。",
        ));
    }
    rep.not_proven.push(Msg::new(
        "decision/record.json, decision/trace.json and decision/report.json describe the state at export time; they are covered by the export signature only, not by the audit chain.",
        "decision/record.json・trace.json・report.json はエクスポート時点の状態です。エクスポート署名の対象ですが、監査チェーンには含まれません。",
    ));
    rep.not_proven.push(Msg::new(
        format!("Anything recorded after the checkpoint ({}).", ts(cp.signed_at_us)),
        format!("チェックポイント（{}）以降に記録された事項。", ts(cp.signed_at_us)),
    ));
}

fn h32s(v: &[H32]) -> Vec<[u8; 32]> {
    v.iter().map(|h| h.0).collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Ja,
    Both,
}

/// Human-readable report.
pub fn render(r: &Report, lang: Lang) -> String {
    let pick = |m: &Msg| -> String {
        match lang {
            Lang::En => m.en.clone(),
            Lang::Ja => m.ja.clone(),
            Lang::Both => format!("{}\n    {}", m.en, m.ja),
        }
    };
    let head = |en: &str, ja: &str| -> String {
        match lang {
            Lang::En => en.into(),
            Lang::Ja => ja.into(),
            Lang::Both => format!("{en} / {ja}"),
        }
    };
    let mut out = String::new();
    out += &head(
        if r.ok {
            "RESULT: VERIFIED"
        } else {
            "RESULT: VERIFICATION FAILED"
        },
        if r.ok {
            "結果：検証成功"
        } else {
            "結果：検証失敗"
        },
    );
    out += "\n\n";
    if !r.ok {
        out += &head("Failures", "失敗した検査");
        out += ":\n";
        for c in r.failures() {
            out += &format!("  ✗ [{}] {}\n", c.code, pick(&c.msg));
        }
        out += "\n";
    }
    out += &head("Checks", "検査");
    out += ":\n";
    for c in r.checks.iter().filter(|c| c.ok) {
        out += &format!("  ✓ {}\n", pick(&c.msg));
    }
    if r.ok {
        out += "\n";
        out += &head("What this pack PROVES", "このパックが証明すること");
        out += ":\n";
        for m in &r.proven {
            out += &format!("  • {}\n", pick(m));
        }
    }
    out += "\n";
    out += &head("What this pack does NOT prove", "このパックが証明しないこと");
    out += ":\n";
    for m in &r.not_proven {
        out += &format!("  • {}\n", pick(m));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        for ok in ["a", "artifacts/x/y.pdf", "decision/record.json"] {
            assert!(path_ok(ok), "{ok}");
        }
        for bad in [
            "",
            "/etc/passwd",
            "../x",
            "a/../b",
            "a//b",
            "a\\b",
            "C:x",
            "./a",
            "a/\u{0}",
        ] {
            assert!(!path_ok(bad), "{bad:?}");
        }
        assert_eq!(safe_name("../見積書 v2.pdf"), "_見積書_v2.pdf");
        assert_eq!(safe_name("..."), "document");
    }

    #[test]
    fn strict_hex_rejects_uppercase_and_wrong_length() {
        assert!(serde_json::from_str::<H32>(&format!("\"{}\"", "ab".repeat(32))).is_ok());
        assert!(serde_json::from_str::<H32>(&format!("\"{}\"", "AB".repeat(32))).is_err());
        assert!(serde_json::from_str::<H32>(&format!("\"{}\"", "ab".repeat(31))).is_err());
    }
}
