//! Sealed delivery receipt (`kizuna-delivery-receipt`, version 1): what KIZUNA gives a supplier when it receives a
//! document from it (portal upload, or staff uploading on the supplier's behalf). One canonical JSON file, checked
//! offline by the same verifier as evidence packs (native CLI `kizuna-verify-pack receipt`; the library has no I/O so a
//! browser build is possible, but none is shipped yet).
//!
//! The receipt carries one audit event of the tenant's hash chain (`delivery_receipt.issued`, or
//! `delivery_receipt.claims_committed` for commitments to extracted values added later), and, once a signed KZCP3
//! checkpoint covers that event, its RFC 6962 inclusion proof and the checkpoint. Before that the receipt is
//! **pending**: internally consistent, but nothing is proven by a KIZUNA signature yet. With the checkpoint it is
//! **final**.
//!
//! Salted commitments: `SHA-256("KZRC1" || salt || len(label) || label || len(value) || value)` with a fresh random
//! 32-byte salt per commitment. Only the commitment is in the audit chain; the salts (openings) are given to the
//! supplier alone, who can disclose a value to a third party by handing over its opening.
//!
//! Proves: KIZUNA recorded receiving exactly the file with this SHA-256 (and size) at the stated time, from the stated
//! supplier channel, in its append-only audit chain; with a final receipt, that chain state was signed with key X at T.
//! Does not prove: anything about the file's content or correctness, that KIZUNA accepted or reviewed it, who at the
//! supplier pressed the button beyond the account recorded, an independent time source, or key ownership unless the key
//! is pinned.

use std::collections::{BTreeMap, BTreeSet};

use audit_engine::{chain_hash, merkle};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    CheckpointEntry, EventEntry, H32, MAX_PROOF_LEN, Msg, Report, VerifyOptions, h32s, key_id, to_canonical, ts,
};

pub const RECEIPT_FORMAT: &str = "kizuna-delivery-receipt";
pub const RECEIPT_VERSION: u32 = 1;
/// Audit action of a delivery receipt (written in the same transaction as the capture of the file).
pub const ISSUED: &str = "delivery_receipt.issued";
/// Audit action of a later receipt that commits to selected extracted values of an earlier delivery.
pub const CLAIMS_COMMITTED: &str = "delivery_receipt.claims_committed";
pub const TARGET_TYPE: &str = "delivery_receipt";
const COMMIT_DOMAIN: &[u8] = b"KZRC1";
/// Untrusted-input limits.
pub const MAX_RECEIPT_BYTES: usize = 1 << 20;
pub const MAX_COMMITMENTS: usize = 200;
pub const MAX_LABEL: usize = 300;
pub const MAX_VALUE: usize = 2_100;

/// The downloadable receipt (canonical JSON, byte-for-byte).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub format: String,
    pub version: u32,
    pub tenant_id: Uuid,
    pub receipt_id: Uuid,
    /// The receipt's audit event; `proof` is empty while pending.
    pub event: EventEntry,
    /// The signed checkpoint that covers the event; null while pending.
    pub checkpoint: Option<CheckpointEntry>,
    /// Openings of the salted commitments (supplier copy only). May be a subset when disclosing selectively.
    pub openings: Vec<Opening>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Opening {
    pub label: String,
    pub value: String,
    pub salt: H32,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Commitment {
    pub label: String,
    pub commitment: H32,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct UploaderRef {
    /// `supplier_user` (portal) or `tenant_user` (staff uploading on the supplier's behalf).
    pub kind: String,
    pub id: Uuid,
}

/// `details` of the receipt's audit event. Optional fields: present on `ISSUED` only (`request_id` only for the
/// `supplier_portal` channel), except `delivery_receipt_id` (present on `CLAIMS_COMMITTED` only).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReceiptDetails {
    pub receipt_id: Uuid,
    pub supplier_id: Uuid,
    pub artifact_id: Uuid,
    pub sha256: H32,
    pub commitments: Vec<Commitment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uploader: Option<UploaderRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_event_seq: Option<i64>,
    /// The evidence request the supplier answered (required for `supplier_portal` deliveries).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_receipt_id: Option<Uuid>,
}

/// Salted commitment to one labelled value (length-prefixed, domain-separated).
pub fn commitment(label: &str, value: &str, salt: &[u8; 32]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(COMMIT_DOMAIN);
    h.update(salt);
    for part in [label.as_bytes(), value.as_bytes()] {
        h.update(u32::try_from(part.len()).unwrap_or(u32::MAX).to_be_bytes());
        h.update(part);
    }
    h.finalize().into()
}

/// The file the receipt is checked against (computed by the caller: streamed natively, in memory in WASM).
#[derive(Debug, Clone, Copy)]
pub struct FileDigest {
    pub sha256: [u8; 32],
    pub size: u64,
}

impl FileDigest {
    pub fn of(bytes: &[u8]) -> Self {
        FileDigest {
            sha256: Sha256::digest(bytes).into(),
            size: bytes.len() as u64,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub receipt_id: Uuid,
    pub action: String,
    pub supplier_id: Uuid,
    pub artifact_id: Uuid,
    pub sha256: String,
    pub size: Option<u64>,
    pub filename: Option<String>,
    pub channel: Option<String>,
    pub uploader: Option<UploaderRef>,
    pub request_id: Option<Uuid>,
    pub received_at: String,
    pub event_seq: i64,
    pub checkpoint_seq: Option<i64>,
    pub checkpoint_signed_at: Option<String>,
    pub key_id: Option<String>,
    pub commitments: usize,
    pub opened: Vec<Opening>,
}

/// Result of [`verify_receipt`]: `status` is `final` (all checks passed and a signed checkpoint covers the event),
/// `pending` (all checks passed, no checkpoint yet) or `failed`.
#[derive(Debug, Clone, Serialize)]
pub struct ReceiptReport {
    pub status: &'static str,
    pub summary: Option<Summary>,
    #[serde(flatten)]
    pub report: Report,
}

/// Serialises a receipt in its canonical form (the bytes the verifier requires).
pub fn build_receipt(r: &Receipt) -> Result<Vec<u8>, String> {
    to_canonical(r)
}

/// Parses a receipt: valid, supported format, within limits and byte-for-byte canonical. Never panics.
pub fn parse_receipt(bytes: &[u8]) -> Result<Receipt, Msg> {
    if bytes.len() > MAX_RECEIPT_BYTES {
        return Err(Msg::new(
            "receipt is larger than 1 MiB",
            "受領証が 1 MiB を超えています",
        ));
    }
    let r: Receipt = serde_json::from_slice(bytes).map_err(|e| {
        Msg::new(
            format!("not a valid receipt: {e}"),
            format!("受領証として不正です: {e}"),
        )
    })?;
    if r.format != RECEIPT_FORMAT || r.version != RECEIPT_VERSION {
        return Err(Msg::new(
            format!(
                "unsupported receipt format {} version {} (this verifier reads {RECEIPT_FORMAT} version {RECEIPT_VERSION})",
                r.format, r.version
            ),
            format!(
                "未対応の受領証形式です: {} バージョン {}（対応: {RECEIPT_FORMAT} バージョン {RECEIPT_VERSION}）",
                r.format, r.version
            ),
        ));
    }
    if r.event.proof.len() > MAX_PROOF_LEN
        || r.checkpoint
            .as_ref()
            .is_some_and(|c| c.head_proof.len() > MAX_PROOF_LEN)
        || r.openings.len() > MAX_COMMITMENTS
    {
        return Err(Msg::new(
            "receipt exceeds the verifier's size limits",
            "受領証が検証ツールの上限を超えています",
        ));
    }
    match to_canonical(&r) {
        Ok(c) if c == bytes => Ok(r),
        _ => Err(Msg::new(
            "receipt is not in canonical form (it was edited or re-serialised)",
            "受領証が正規形ではありません（編集または再シリアライズされています）",
        )),
    }
}

/// Verifies a receipt offline. Every check runs and has a stable code.
pub fn verify_receipt(bytes: &[u8], file: Option<FileDigest>, opts: &VerifyOptions) -> ReceiptReport {
    let mut rep = Report::default();
    let summary = run(bytes, file, opts, &mut rep);
    rep.ok = rep.failures().next().is_none() && summary.is_some();
    let status = match &summary {
        _ if !rep.ok => "failed",
        Some(s) if s.checkpoint_seq.is_some() => "final",
        _ => "pending",
    };
    ReceiptReport {
        status,
        summary,
        report: rep,
    }
}

fn run(bytes: &[u8], file: Option<FileDigest>, opts: &VerifyOptions, rep: &mut Report) -> Option<Summary> {
    let r = match parse_receipt(bytes) {
        Ok(r) => r,
        Err(m) => {
            rep.fail("receipt_format", m);
            not_proven_always(rep, None);
            return None;
        }
    };
    rep.pass(
        "receipt_format",
        Msg::new(
            format!("canonical {RECEIPT_FORMAT} v{RECEIPT_VERSION} receipt"),
            format!("正規形の {RECEIPT_FORMAT} v{RECEIPT_VERSION} 受領証です"),
        ),
    );
    let e = &r.event;

    // The event is a receipt event about this receipt, with well-formed details.
    let details: Option<ReceiptDetails> = serde_json::from_value(e.details.clone()).ok();
    let kind_ok = details.as_ref().is_some_and(|d| {
        let shape = match e.action.as_str() {
            ISSUED => {
                d.delivery_receipt_id.is_none()
                    && match d.channel.as_deref() {
                        Some("supplier_portal") => d.request_id.is_some(),
                        Some("staff_on_behalf") => true,
                        _ => false,
                    }
                    && d.uploader.is_some()
                    && d.size.is_some()
                    && d.filename.is_some()
                    && d.capture_event_seq.is_some()
            }
            CLAIMS_COMMITTED => d.delivery_receipt_id.is_some() && !d.commitments.is_empty() && d.request_id.is_none(),
            _ => false,
        };
        shape
            && e.target_type == TARGET_TYPE
            && e.target_id.as_deref() == Some(r.receipt_id.to_string().as_str())
            && d.receipt_id == r.receipt_id
            && d.commitments.len() <= MAX_COMMITMENTS
    });
    if !rep.check(
        kind_ok,
        "receipt_event_invalid",
        if kind_ok {
            Msg::new(
                format!("audit event #{} ({}) is this receipt's event", e.seq, e.action),
                format!("監査イベント #{}（{}）はこの受領証のイベントです", e.seq, e.action),
            )
        } else {
            Msg::new(
                "the audit event in the receipt is not a delivery receipt event for this receipt id",
                "受領証内の監査イベントが、この受領証IDの受領イベントではありません",
            )
        },
    ) {
        not_proven_always(rep, None);
        return None;
    }
    let d = details?;

    // The event is unchanged (its content matches its chain hash).
    let hash_ok = chain_hash(&e.event(r.tenant_id), &e.prev_hash.0).is_ok_and(|h| h == e.hash.0);
    rep.check(
        hash_ok,
        "event_hash_mismatch",
        if hash_ok {
            Msg::new(
                format!("audit event #{} content matches its chain hash", e.seq),
                format!("監査イベント #{} の内容はチェーンハッシュと一致します", e.seq),
            )
        } else {
            Msg::new(
                format!(
                    "audit event #{} was changed: its content does not match its chain hash",
                    e.seq
                ),
                format!(
                    "監査イベント #{} が改変されています：内容がチェーンハッシュと一致しません",
                    e.seq
                ),
            )
        },
    );

    // Commitments and openings.
    let mut by_label = BTreeMap::new();
    let mut labels_ok = true;
    for c in &d.commitments {
        labels_ok &= c.label.len() <= MAX_LABEL && by_label.insert(c.label.as_str(), c.commitment).is_none();
    }
    let mut opened = BTreeSet::new();
    let mut openings_ok = labels_ok;
    for o in &r.openings {
        let fresh = opened.insert(o.label.as_str());
        let matches = by_label
            .get(o.label.as_str())
            .is_some_and(|c| o.value.len() <= MAX_VALUE && commitment(&o.label, &o.value, &o.salt.0) == c.0);
        if !(fresh && matches) {
            openings_ok = false;
            rep.fail(
                "opening_mismatch",
                Msg::new(
                    format!(
                        "the opening for \"{}\" does not match a commitment in the receipt event",
                        o.label
                    ),
                    format!("「{}」の開示値が受領イベント内のコミットメントと一致しません", o.label),
                ),
            );
        }
    }
    if openings_ok {
        rep.pass(
            "openings",
            Msg::new(
                format!(
                    "{} of {} committed values opened and matching",
                    r.openings.len(),
                    d.commitments.len()
                ),
                format!(
                    "{} 件中 {} 件のコミット値が開示され、一致しています",
                    d.commitments.len(),
                    r.openings.len()
                ),
            ),
        );
    } else if !labels_ok {
        rep.fail(
            "commitments_invalid",
            Msg::new(
                "commitment labels are duplicated or too long",
                "コミットメントのラベルが重複しているか長すぎます",
            ),
        );
    }

    // The file the supplier holds, when given.
    if let Some(f) = file {
        let ok = f.sha256 == d.sha256.0 && d.size.is_none_or(|s| s == f.size);
        rep.check(
            ok,
            "file_mismatch",
            if ok {
                Msg::new(
                    format!(
                        "the given file has SHA-256 {} and {} bytes, as receipted",
                        hex::encode(f.sha256),
                        f.size
                    ),
                    format!(
                        "指定されたファイルは SHA-256 {}・{} バイトで、受領内容と一致します",
                        hex::encode(f.sha256),
                        f.size
                    ),
                )
            } else {
                Msg::new(
                    format!(
                        "the given file (SHA-256 {}, {} bytes) is not the receipted file",
                        hex::encode(f.sha256),
                        f.size
                    ),
                    format!(
                        "指定されたファイル（SHA-256 {}・{} バイト）は受領されたファイルではありません",
                        hex::encode(f.sha256),
                        f.size
                    ),
                )
            },
        );
    }

    // Checkpoint (final receipts).
    let mut pinned = false;
    let mut cp_proven = false;
    match &r.checkpoint {
        None => {
            let ok = e.proof.is_empty();
            rep.check(
                ok,
                "pending_with_proof",
                if ok {
                    Msg::new(
                        "no signed checkpoint covers this event yet: the receipt is PENDING",
                        "このイベントを含む署名済みチェックポイントはまだありません：受領証は「保留」です",
                    )
                } else {
                    Msg::new(
                        "a pending receipt must not carry a proof",
                        "保留中の受領証に証明が含まれています",
                    )
                },
            );
        }
        Some(cp) => {
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
                        format!("checkpoint key id {} does not belong to its public key", cp.key_id),
                        format!("チェックポイントの鍵ID {} は公開鍵のものではありません", cp.key_id),
                    )
                },
            );
            if let Some(t) = opts.trusted_key.as_deref().map(str::to_ascii_lowercase) {
                pinned = t == hex::encode(cp.public_key.0) || t == key_id(&cp.public_key.0);
                rep.check(
                    pinned,
                    "trusted_key_mismatch",
                    if pinned {
                        Msg::new(
                            format!("signing key {} is the trusted key", cp.key_id),
                            format!("署名鍵 {} は信頼済みの鍵です", cp.key_id),
                        )
                    } else {
                        Msg::new(
                            format!("signed by key {}, not by the trusted key {t}", cp.key_id),
                            format!("署名鍵は {} で、信頼済みの鍵 {t} ではありません", cp.key_id),
                        )
                    },
                );
            }
            let size = u64::try_from(cp.seq).unwrap_or(0);
            let sig_ok = cp.message_version == "KZCP3"
                && size > 0
                && audit_engine::checkpoint::verify(&cp.public_key.0, &cp.message(r.tenant_id), &cp.signature.0);
            rep.check(
                sig_ok,
                "checkpoint_signature_invalid",
                if sig_ok {
                    Msg::new(
                        format!(
                            "checkpoint #{} signed by key {} at {}",
                            cp.seq,
                            cp.key_id,
                            ts(cp.signed_at_us)
                        ),
                        format!(
                            "チェックポイント #{} は鍵 {} により {} に署名されています",
                            cp.seq,
                            cp.key_id,
                            ts(cp.signed_at_us)
                        ),
                    )
                } else {
                    Msg::new(
                        format!("checkpoint #{} signature is invalid", cp.seq),
                        format!("チェックポイント #{} の署名が無効です", cp.seq),
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
            let in_range = e.seq >= 1 && e.seq <= cp.seq;
            let proof_ok = in_range
                && merkle::verify(
                    &cp.merkle_root.0,
                    size,
                    u64::try_from(e.seq - 1).unwrap_or(u64::MAX),
                    &e.hash.0,
                    &h32s(&e.proof),
                )
                .is_ok();
            rep.check(
                proof_ok,
                "event_proof_invalid",
                if proof_ok {
                    Msg::new(
                        format!("audit event #{} is included under checkpoint #{}", e.seq, cp.seq),
                        format!(
                            "監査イベント #{} はチェックポイント #{} に含まれています",
                            e.seq, cp.seq
                        ),
                    )
                } else {
                    Msg::new(
                        format!(
                            "Merkle proof of audit event #{} does not lead to the signed checkpoint root",
                            e.seq
                        ),
                        format!(
                            "監査イベント #{} の Merkle 証明が署名済みチェックポイントのルートに到達しません",
                            e.seq
                        ),
                    )
                },
            );
            cp_proven = sig_ok && head_ok && proof_ok && hash_ok;
        }
    }

    // Proven / not proven.
    let when = ts(e.occurred_at_us);
    let sha = hex::encode(d.sha256.0);
    if let Some(cp) = r.checkpoint.as_ref().filter(|_| cp_proven) {
        let pin = |en: bool| match (pinned, en) {
            (true, true) => " (the key you trust)",
            (true, false) => "（信頼済みの鍵）",
            _ => "",
        };
        match e.action.as_str() {
            ISSUED => rep.proven.push(Msg::new(
                format!(
                    "KIZUNA recorded receiving exactly the file with SHA-256 {sha} ({} bytes, \"{}\") from supplier {} via {}{} at {when} (KIZUNA server clock).",
                    d.size.unwrap_or(0), d.filename.as_deref().unwrap_or(""), d.supplier_id, d.channel.as_deref().unwrap_or(""),
                    d.request_id.map(|r| format!(" against request {r}")).unwrap_or_default()
                ),
                format!(
                    "KIZUNA は SHA-256 {sha}（{} バイト、「{}」）のファイルを、取引先 {} から {} 経由{}で {when}（KIZUNA サーバー時刻）に受領したことを記録しています。",
                    d.size.unwrap_or(0), d.filename.as_deref().unwrap_or(""), d.supplier_id, d.channel.as_deref().unwrap_or(""),
                    d.request_id.map(|r| format!("（依頼 {r} への回答）")).unwrap_or_default()
                ),
            )),
            _ => rep.proven.push(Msg::new(
                format!("KIZUNA committed at {when} to {} value(s) extracted from the file with SHA-256 {sha}.", d.commitments.len()),
                format!("KIZUNA は {when} に、SHA-256 {sha} のファイルから抽出した {} 件の値にコミットしました。", d.commitments.len()),
            )),
        }
        rep.proven.push(Msg::new(
            format!(
                "That record (audit event #{}) is in tenant {}'s append-only audit chain, whose state up to event #{} was signed with Ed25519 key {} at {}{}: it cannot be altered or removed afterwards without breaking that signature.",
                e.seq, r.tenant_id, cp.seq, cp.key_id, ts(cp.signed_at_us), pin(true)
            ),
            format!(
                "この記録（監査イベント #{}）はテナント {} の追記専用監査チェーンにあり、イベント #{} までの状態は Ed25519 鍵 {} により {} に署名されています{}。署名を壊さずに後から改変・削除することはできません。",
                e.seq, r.tenant_id, cp.seq, cp.key_id, ts(cp.signed_at_us), pin(false)
            ),
        ));
        if !r.openings.is_empty() && openings_ok {
            rep.proven.push(Msg::new(
                format!(
                    "The {} opened value(s) are exactly the values KIZUNA committed to.",
                    r.openings.len()
                ),
                format!(
                    "開示された {} 件の値は、KIZUNA がコミットした値と完全に同一です。",
                    r.openings.len()
                ),
            ));
        }
        if !pinned {
            rep.not_proven.push(Msg::new(
                format!("That key {} belongs to the customer's KIZUNA tenant: confirm the key id through a separate channel, or rerun with --trusted-key.", cp.key_id),
                format!("鍵 {} が取引先の KIZUNA テナントの鍵であること：別経路で鍵IDを確認するか、--trusted-key を指定して再実行してください。", cp.key_id),
            ));
        }
    }
    not_proven_always(rep, Some(r.checkpoint.is_none()));

    Some(Summary {
        receipt_id: r.receipt_id,
        action: e.action.clone(),
        supplier_id: d.supplier_id,
        artifact_id: d.artifact_id,
        sha256: sha,
        size: d.size,
        filename: d.filename.clone(),
        channel: d.channel.clone(),
        uploader: d.uploader.clone(),
        request_id: d.request_id,
        received_at: when,
        event_seq: e.seq,
        checkpoint_seq: r.checkpoint.as_ref().filter(|_| cp_proven).map(|c| c.seq),
        checkpoint_signed_at: r.checkpoint.as_ref().filter(|_| cp_proven).map(|c| ts(c.signed_at_us)),
        key_id: r.checkpoint.as_ref().map(|c| c.key_id.clone()),
        commitments: d.commitments.len(),
        opened: if openings_ok { r.openings.clone() } else { Vec::new() },
    })
}

fn not_proven_always(rep: &mut Report, pending: Option<bool>) {
    if pending == Some(true) {
        rep.not_proven.push(Msg::new(
            "PENDING: no signed checkpoint covers this receipt yet, so nothing in it is proven by a KIZUNA signature. Download it again after the next checkpoint to get the final receipt.",
            "保留中：この受領証を含む署名済みチェックポイントがまだないため、KIZUNA の署名による証明はありません。次のチェックポイント後に再ダウンロードすると確定版になります。",
        ));
    }
    rep.not_proven.push(Msg::new(
        "Anything about the file's content: that it is correct, genuine, complete or meets a specification. A receipt is not acceptance, review or approval.",
        "ファイル内容に関する事項：正しさ・真正性・完全性・仕様適合。受領証は受入・審査・承認ではありません。",
    ));
    rep.not_proven.push(Msg::new(
        "The time comes from KIZUNA's server clock, not an independent timestamp authority; the checkpoint signature time is the latest the record can have been made.",
        "時刻は KIZUNA サーバーの時計によるもので、独立した時刻認証局ではありません。チェックポイントの署名時刻が記録の最遅時刻です。",
    ));
    rep.not_proven.push(Msg::new(
        "Which person at the supplier sent the file beyond the account recorded, and, for staff uploads on the supplier's behalf, that the supplier actually sent it to that staff member.",
        "記録されたアカウント以上に、取引先のどの人物が送付したか。また、担当者による代理アップロードの場合、取引先が実際にその担当者へ送付したこと。",
    ));
    rep.not_proven.push(Msg::new(
        "Committed values are what KIZUNA extracted from the file; a matching opening does not make the extraction correct. Unopened commitments reveal nothing.",
        "コミット値は KIZUNA がファイルから抽出した値です。開示値が一致しても抽出が正しいことにはなりません。未開示のコミットメントからは何も分かりません。",
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commitment_is_length_prefixed_and_salted() {
        let s = [1u8; 32];
        assert_ne!(commitment("ab", "c", &s), commitment("a", "bc", &s));
        assert_ne!(commitment("a", "b", &s), commitment("a", "b", &[2u8; 32]));
        assert_eq!(commitment("a", "b", &s), commitment("a", "b", &s));
    }

    #[test]
    fn garbage_never_panics() {
        for b in [&b""[..], b"{}", b"null", b"[1,2]", &[0xff; 40]] {
            let r = verify_receipt(b, None, &VerifyOptions::default());
            assert_eq!(r.status, "failed");
        }
    }
}
