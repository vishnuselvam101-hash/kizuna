#!/usr/bin/env python3
"""Drives kizuna-server through the synthetic manufacturing workflow; scores findings vs ground truth.

Usage: driver.py --base http://127.0.0.1:PORT --data DIR --creds creds.json --out result.json

creds.json: {"tenant": slug, "other_tenant": slug, "password": pw,
             "users": {"analyst": email, "reviewer": email, "reviewer2": email, "viewer": email,
                       "auditor": email, "outsider": email}}
Stdlib only. Exit code 0 = every workflow check passed (precision/recall are reported, not gated, unless
--min-recall/--min-precision are given).
"""

from __future__ import annotations

import argparse
import json
import time
import urllib.error
import urllib.request
import uuid
from pathlib import Path


class Api:
    def __init__(self, base: str):
        if not base.startswith(("http://", "https://")):
            raise SystemExit("--base must be an http(s) URL")
        self.base = base.rstrip("/")

    def req(
        self,
        method: str,
        path: str,
        token: str | None = None,
        body=None,
        raw: bytes | None = None,
        ctype=None,
    ):
        data = raw if raw is not None else (json.dumps(body).encode() if body is not None else None)
        r = urllib.request.Request(self.base + path, data=data, method=method)  # noqa: S310 (scheme checked)
        if token:
            r.add_header("Authorization", f"Bearer {token}")
        if data is not None:
            r.add_header("Content-Type", ctype or "application/json")
        try:
            with urllib.request.urlopen(r, timeout=90) as resp:  # noqa: S310 (scheme checked)
                status, text = resp.status, resp.read()
        except urllib.error.HTTPError as e:
            status, text = e.code, e.read()
        try:
            return status, json.loads(text)
        except ValueError:
            return status, text.decode("utf-8", "replace")

    def upload(self, token: str | None, name: str, data: bytes, kind: str | None):
        b = "kizunae2e" + uuid.uuid4().hex
        parts = b""
        if kind:
            parts += f'--{b}\r\nContent-Disposition: form-data; name="kind"\r\n\r\n{kind}\r\n'.encode()
        parts += (
            f'--{b}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\n'
            "Content-Type: application/octet-stream\r\n\r\n"
        ).encode()
        parts += data + f"\r\n--{b}--\r\n".encode()
        return self.req(
            "POST", "/api/v1/artifacts", token, raw=parts, ctype=f"multipart/form-data; boundary={b}"
        )


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", required=True)
    ap.add_argument("--data", required=True)
    ap.add_argument("--creds", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--min-recall", type=float, default=None)
    ap.add_argument("--min-precision", type=float, default=None)
    a = ap.parse_args()
    api = Api(a.base)
    data = Path(a.data)
    manifest = json.loads((data / "manifest.json").read_text())
    truth = json.loads((data / "ground_truth.json").read_text())
    creds = json.loads(Path(a.creds).read_text())
    checks: list[dict] = []

    def check(name: str, ok: bool, detail=None) -> bool:
        checks.append({"check": name, "ok": bool(ok), "detail": detail})
        print(f"  [{'PASS' if ok else 'FAIL'}] {name}" + (f" — {detail}" if detail and not ok else ""))
        return ok

    tok: dict[str, str] = {}
    for role, email in creds["users"].items():
        tenant = creds["other_tenant"] if role == "outsider" else creds["tenant"]
        s, b = api.req(
            "POST",
            "/api/v1/auth/login",
            body={"tenant": tenant, "email": email, "password": creds["password"]},
        )
        if not check(f"login {role}", s == 200, b):
            return 2
        tok[role] = b["access_token"]

    # 1. Ingest: upload + sandboxed extraction for every file.
    t0 = time.perf_counter()
    ids: dict[str, str] = {}
    upload_ok = extract_ok = 0
    mal_results = []
    for m in manifest:
        blob = (data / "files" / m["file"]).read_bytes()
        s, b = api.upload(tok["analyst"], Path(m["file"]).name, blob, m["kind"])
        if m["expect"] != "accepted":
            got = (
                "rejected_415"
                if s == 415
                else (b.get("artifact", {}).get("status") if isinstance(b, dict) else str(s))
            )
            mal_results.append({"file": m["file"], "expect": m["expect"], "got": got, "http": s})
            if s == 200:
                ids[m["file"]] = b["artifact"]["id"]
            continue
        if s != 200 or b["artifact"]["status"] != "accepted":
            check(f"upload {m['file']}", False, b)
            continue
        upload_ok += 1
        aid = b["artifact"]["id"]
        ids[m["file"]] = aid
        s, x = api.req("POST", f"/api/v1/artifacts/{aid}/extract", tok["analyst"], body={})
        if s == 200 and x.get("claims_created", 0) > 0:
            extract_ok += 1
        else:
            check(f"extract {m['file']}", False, x)
    ingest_s = time.perf_counter() - t0
    n_clean = sum(1 for m in manifest if m["expect"] == "accepted")
    check("all clean documents uploaded and accepted", upload_ok == n_clean, f"{upload_ok}/{n_clean}")
    check("all clean documents produced claims", extract_ok == n_clean, f"{extract_ok}/{n_clean}")

    # 2. Detection.
    t1 = time.perf_counter()
    s, run = api.req("POST", "/api/v1/verification/run", tok["analyst"], body={})
    detect_s = time.perf_counter() - t1
    check("verification run", s == 200, run)
    s, rerun = api.req("POST", "/api/v1/verification/run", tok["analyst"], body={})
    check("verification run is idempotent", s == 200 and rerun.get("findings_new") == 0, rerun)
    s, conflicts = api.req("GET", "/api/v1/conflicts?status=open", tok["viewer"])
    check("viewer can read conflicts", s == 200, s)
    conflicts = conflicts if isinstance(conflicts, list) else []
    s, cand = api.req("GET", "/api/v1/entities/candidates?subject_type=part", tok["viewer"])
    candidates = cand.get("candidates", []) if isinstance(cand, dict) else []

    # 3. Score document-level faults: a finding is a TP when its part carries an injected fault whose
    #    accepted kinds include the finding's kind; every other finding is a FP. A fault counts as
    #    detected when at least one TP finding hits it.
    by_part = {p["part"]: p for p in truth["parts"]}
    tp = fp = 0
    fp_list, hit = [], set()
    for c in conflicts:
        p = by_part.get(c["subject_key"])
        if p and p["fault"] and c["kind"] in p["accepted_kinds"]:
            tp += 1
            hit.add(p["part"])
        else:
            fp += 1
            fp_list.append({"kind": c["kind"], "subject_key": c["subject_key"]})
    rule_faults = [p for p in truth["parts"] if p["fault"] and p["accepted_kinds"]]
    fn_list = [{"part": p["part"], "fault": p["fault"]} for p in rule_faults if p["part"] not in hit]
    per_fault: dict[str, dict] = {}
    for p in rule_faults:
        d = per_fault.setdefault(p["fault"], {"injected": 0, "detected": 0})
        d["injected"] += 1
        d["detected"] += p["part"] in hit

    # Duplicate component ids: entity-resolution candidates (never auto-merged).
    want_pairs = {tuple(x) for x in truth["duplicate_pairs"]}
    got_pairs = {tuple(sorted([c["left"], c["right"]])) for c in candidates}
    er_tp = len(want_pairs & got_pairs)
    per_fault["duplicate_component_id"] = {"injected": len(want_pairs), "detected": er_tp}
    er_fp = sorted(got_pairs - want_pairs)

    # Malicious uploads.
    mal_ok = sum(1 for r in mal_results if r["expect"] == r["got"])
    per_fault["malicious_upload"] = {"injected": len(mal_results), "detected": mal_ok}

    # 4. Decision workflow on a clean part: decision, trace, report, selective invalidation, audit chain.
    clean = next(p for p in truth["parts"] if not p["fault"])
    s, claims = api.req(
        "GET", f"/api/v1/claims?subject_type=part&subject_key={clean['part']}", tok["reviewer"]
    )
    ev = [
        {"claim_id": c["id"]}
        for c in claims
        if c["predicate"] in ("mfg.dimension.length", "mfg.approval.status")
    ][:2]
    length_claim = next((c for c in claims if c["predicate"] == "mfg.dimension.length"), None)
    if ev and length_claim:
        ev = [
            {"claim_id": length_claim["id"], "materiality": {"type": "numeric_abs", "threshold": "0.05"}}
            if e["claim_id"] == length_claim["id"]
            else e
            for e in ev
        ]
    s, dec = api.req(
        "POST",
        "/api/v1/decisions",
        tok["reviewer"],
        body={
            "kind": "supplier_part_release",
            "title": f"Release {clean['part']} rev {clean['revision']}",
            "subject_type": "part",
            "subject_key": clean["part"],
            "outcome": "approved",
            "rationale": "Synthetic E2E: spec, drawing, PO and inspection agree",
            "evidence": ev,
            "depends_on": [],
        },
    )
    if check("reviewer records a decision on clean evidence", s == 200, dec):
        did = dec["id"]
        s, tr = api.req("GET", f"/api/v1/decisions/{did}/trace", tok["viewer"])
        check("decision trace reconstructs", s == 200 and isinstance(tr, dict), s)
        s, rep = api.req("GET", f"/api/v1/decisions/{did}/report", tok["auditor"])
        check("verification report has a sha256", s == 200 and len(rep.get("report_sha256", "")) == 64, s)
        if length_claim:
            body = {
                k: length_claim[k]
                for k in ("subject_type", "subject_key", "predicate", "value_unit", "revision", "scope")
                if length_claim.get(k) is not None
            }
            body["value_num"] = str(float(length_claim["value_num"]) + 1)
            body["reason"] = "supplier corrected drawing"
            s, sup = api.req(
                "POST", f"/api/v1/claims/{length_claim['id']}/supersede", tok["reviewer"], body=body
            )
            s2, d2 = api.req("GET", f"/api/v1/decisions/{did}", tok["viewer"])
            check(
                "material correction invalidates the decision",
                s == 200 and d2.get("decision", {}).get("status") == "potentially_affected",
                {
                    "supersede": s,
                    "status": d2.get("decision", {}).get("status") if isinstance(d2, dict) else d2,
                },
            )
    s, av = api.req("GET", "/api/v1/audit/verify", tok["auditor"])
    check("audit chain intact", s == 200 and av.get("intact") is True, av)

    # 5. Unauthorised requests: each must be refused with the expected status.
    sample_conflict = conflicts[0]["id"] if conflicts else str(uuid.uuid4())
    quarantined = next((ids[r["file"]] for r in mal_results if r["got"] == "quarantined"), str(uuid.uuid4()))
    sample_artifact = next(iter(v for k, v in ids.items() if not k.startswith("malicious/")))
    unauth = []
    for u in truth["unauthorised_requests"]:
        path = u["path"].format(conflict=sample_conflict, quarantined=quarantined, artifact=sample_artifact)
        t = {"forged": "kz_" + "A" * 43, None: None}.get(u["as"], tok.get(u["as"] or ""))
        if u["method"] == "UPLOAD":
            s, _ = api.upload(t, "x.csv", b"PO Number,Part No\nPO-1,X-1\n", "purchase_order")
        elif u["method"] == "POST":
            body = {}
            if "review" in path:
                body = {"decision": "approve", "comment": "x"}
            elif path.endswith(
                "/decisions"
            ):  # well-formed, so the refusal is authorisation and not validation
                body = {
                    "kind": "supplier_part_release",
                    "title": "t",
                    "subject_type": "part",
                    "subject_key": "X-1",
                    "outcome": "approved",
                    "rationale": "r",
                    "evidence": [],
                    "depends_on": [],
                }
            s, _ = api.req("POST", path, t, body=body)
        else:
            s, _ = api.req("GET", path, t)
        unauth.append({"id": u["id"], "expect": u["expect"], "got": s})
    blocked = sum(1 for u in unauth if u["got"] == u["expect"])
    per_fault["unauthorised_request"] = {"injected": len(unauth), "detected": blocked}
    s, sec = api.req("GET", "/api/v1/security-events", tok["auditor"])
    sec_kinds = sorted({e.get("kind") for e in sec}) if isinstance(sec, list) else []

    precision = tp / (tp + fp) if tp + fp else 0.0
    recall = len(hit) / len(rule_faults) if rule_faults else 0.0
    injected = sum(v["injected"] for v in per_fault.values())
    detected = sum(v["detected"] for v in per_fault.values())
    result = {
        "seed": truth["seed"],
        "documents": n_clean,
        "malicious_files": len(mal_results),
        "timings_s": {"ingest_and_extract": round(ingest_s, 3), "verification_run": round(detect_s, 3)},
        "verification_run": run,
        "findings_total": len(conflicts),
        "findings_by_kind": {
            k: sum(1 for c in conflicts if c["kind"] == k) for k in sorted({c["kind"] for c in conflicts})
        },
        "rule_findings": {
            "precision": round(precision, 4),
            "recall_faults": round(recall, 4),
            "tp_findings": tp,
            "fp_findings": fp,
            "false_positives": fp_list,
            "missed_faults": fn_list,
        },
        "entity_resolution": {
            "expected_pairs": sorted(want_pairs),
            "candidate_pairs": sorted(got_pairs),
            "unexpected": er_fp,
        },
        "per_fault_type": per_fault,
        "overall_fault_recall": round(detected / injected, 4) if injected else 0.0,
        "malicious_uploads": mal_results,
        "unauthorised_requests": unauth,
        "security_event_kinds": sec_kinds,
        "checks": checks,
    }
    Path(a.out).write_text(json.dumps(result, indent=1, default=list))
    print(
        f"\nfindings={len(conflicts)} precision={precision:.3f} recall(rule faults)={recall:.3f} "
        f"overall fault recall={result['overall_fault_recall']:.3f} ({detected}/{injected})"
    )
    for k, v in per_fault.items():
        print(f"  {k:24s} {v['detected']}/{v['injected']}")
    ok = all(c["ok"] for c in checks)
    if a.min_recall is not None and recall < a.min_recall:
        ok = False
    if a.min_precision is not None and precision < a.min_precision:
        ok = False
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
