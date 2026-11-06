#!/usr/bin/env python3
"""Mixed-load soak driver for a running kizuna-server (stdlib only).

Usage: soak.py --base URL --creds creds.json --datasets DIR --minutes N --out result.json
       [--workers 8] [--pid SERVER_PID] [--psql-cmd "psql ... -d DB"] [--storage DIR]
       [--stability --metrics URL --owner-psql-cmd "psql ... -d DB (owner role)"]

--stability (STABILITY_2026-11-06) adds: Idempotency-Key on half of all mutations with immediate same-key retries
(a replay must return the identical status and body), background verification jobs, the supplier portal (two
onboarded supplier users: list/get requests, upload, list uploads; internal portal-upload view) and CAPA 8D
(open from a conflict, D1/D2 save + complete, list, notifications); the sampler also scrapes --metrics.

--datasets holds gen_dataset.py outputs (one sub-directory per seed) so uploads stay unique for the whole run.
Each worker loops over a weighted mix: upload+extract, claim review, conflict review, verification run, decision
create/trace/report, supersede, org report, dashboard, audit verify. Records per-op latency and status; a sampler
records server RSS, open fds and DB connections every 10 s. 5xx and transport errors count as errors; 4xx business
refusals (409/422/403 for four-eyes etc.) are counted separately.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import hmac
import json
import struct
import os
import random
import subprocess
import threading
import time
import urllib.error
import urllib.request
import uuid
from collections import defaultdict
from pathlib import Path


class Api:
    def __init__(self, base, creds):
        self.base, self.creds, self.tok, self.lock = base.rstrip("/"), creds, {}, threading.Lock()
        self.totp, self.last_step = {}, {}

    def code(self, role):
        """TOTP for the next unused step (the server accepts +-1 step and refuses replays)."""
        now = int(time.time()) // 30
        step = max(now - 1, self.last_step.get(role, -1) + 1)
        while step > now + 1:
            time.sleep(5)
            now = int(time.time()) // 30
        self.last_step[role] = step
        key = base64.b32decode(self.totp[role] + "=" * (-len(self.totp[role]) % 8))
        h = hmac.new(key, struct.pack(">Q", step), hashlib.sha1).digest()
        o = h[-1] & 15
        return f"{(struct.unpack('>I', h[o:o + 4])[0] & 0x7FFFFFFF) % 1_000_000:06d}"

    def enroll_mfa(self, role):
        assert self.login(role) == 200
        s, b = self.req("POST", "/api/v1/auth/mfa/totp/setup", role,
                          body={"password": self.creds["password"]})
        assert s == 200, b
        self.totp[role] = b["secret_base32"]
        s, b = self.req("POST", "/api/v1/auth/mfa/totp/confirm", role, body={"code": self.code(role)})
        assert s == 200, b
        assert self.login(role) == 200

    def login(self, role):
        tenant = self.creds["other_tenant"] if role == "outsider" else self.creds["tenant"]
        body = {"tenant": tenant, "email": self.creds["users"][role], "password": self.creds["password"]}
        if role in self.totp:
            body["totp_code"] = self.code(role)
        path = "/api/v1/portal/login" if role.startswith("sup") else "/api/v1/auth/login"
        s, b = self.req("POST", path, None, body=body)
        if s == 200:
            with self.lock:
                self.tok[role] = b["access_token"]
        return s

    def req(self, method, path, role, body=None, raw=None, ctype=None, timeout=120, headers=None):
        data = raw if raw is not None else (json.dumps(body).encode() if body is not None else None)
        r = urllib.request.Request(self.base + path, data=data, method=method)
        for k, v in (headers or {}).items():
            r.add_header(k, v)
        if role:
            r.add_header("Authorization", f"Bearer {self.tok.get(role, '')}")
        if data is not None:
            r.add_header("Content-Type", ctype or "application/json")
        try:
            with urllib.request.urlopen(r, timeout=timeout) as resp:
                status, text = resp.status, resp.read()
        except urllib.error.HTTPError as e:
            status, text = e.code, e.read()
        except Exception as e:  # transport error (reset, refused, timeout)
            return -1, f"{type(e).__name__}: {e}"
        try:
            return status, json.loads(text)
        except ValueError:
            return status, text.decode("utf-8", "replace")

    @staticmethod
    def multipart(name, data, kind):
        b = "kzsoak" + uuid.uuid4().hex
        parts = f'--{b}\r\nContent-Disposition: form-data; name="kind"\r\n\r\n{kind}\r\n'.encode() if kind else b""
        parts += (f'--{b}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\n'
                  "Content-Type: application/octet-stream\r\n\r\n").encode() + data + f"\r\n--{b}--\r\n".encode()
        return parts, f"multipart/form-data; boundary={b}"

    def upload(self, role, name, data, kind, headers=None):
        parts, ctype = self.multipart(name, data, kind)
        return self.req("POST", "/api/v1/artifacts", role, raw=parts, ctype=ctype, headers=headers)


def psql(cmd, sql):
    out = subprocess.run(cmd.split() + ["-v", "ON_ERROR_STOP=1", "-qAt"], input=sql, capture_output=True, text=True)
    if out.returncode:
        raise RuntimeError(out.stderr)
    return out.stdout.strip()


def stability_setup(api, a):
    """Two supplier portal users (invite -> password + TOTP), five approved evidence requests each (inserted as the
    owner, as the portal tests do), and a high daily upload cap for the soak."""
    sups, requests = [], {}
    _, me = api.req("GET", "/api/v1/auth/me", "admin")
    tenant_id = psql(a.owner_psql_cmd, f"SELECT id FROM tenants WHERE slug = '{api.creds['tenant']}'")
    for i in range(2):
        role, email = f"sup{i}", f"sup{i}-{uuid.uuid4().hex[:8]}@supplier.example"
        s, b = api.req("POST", "/api/v1/portal-users", "admin",
                       body={"supplier": f"SOAK SUPPLIER {i}", "email": email, "display_name": f"Soak {i}"})
        assert s == 200, b
        inv, sid = b["invite_token"], b["user"]["supplier_id"]
        s, b = api.req("POST", "/api/v1/portal/invite/start", None, body={"token": inv, "password": api.creds["password"]})
        assert s == 200, b
        api.totp[role] = b["secret_base32"]
        s, b = api.req("POST", "/api/v1/portal/invite/finish", None, body={"token": inv, "code": api.code(role)})
        assert s == 200, b
        api.tok[role] = b["access_token"]
        api.creds["users"][role] = email
        ids = [str(uuid.uuid4()) for _ in range(5)]
        rows = ",".join(
            f"('{r}', '{tenant_id}', 'RR-SOAK-{r[:8].upper()}', 'SOAK SUPPLIER {i}', 'mfg-resupply@1', "
            f"ARRAY['{uuid.uuid4()}']::uuid[], '[]'::jsonb, 1, 1, true, '再提出のお願い', 'Please resend', "
            f"decode(repeat('07', 32), 'hex'), '{me['user_id']}', 'approved', '{me['user_id']}', now(), current_date + 14, '{sid}')"
            for r in ids)
        psql(a.owner_psql_cmd, f"""BEGIN;
SELECT set_config('kizuna.tenant_id', '{tenant_id}', true);
INSERT INTO evidence_requests (id, tenant_id, reference, supplier, engine, decision_ids, documents, requested_count,
  baseline_count, exact, body_ja, body_en, content_sha256, created_by, status, reviewed_by, reviewed_at, due_on,
  supplier_id) VALUES {rows};
UPDATE suppliers SET max_uploads_per_day = 1000 WHERE id = '{sid}';
COMMIT;""")
        sups.append(role)
        requests[role] = ids
    return {"sups": sups, "requests": requests}


def pct(xs, p):
    if not xs:
        return None
    xs = sorted(xs)
    return round(xs[min(len(xs) - 1, int(p / 100 * len(xs)))] * 1000, 1)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", required=True)
    ap.add_argument("--creds", required=True)
    ap.add_argument("--datasets", required=True)
    ap.add_argument("--minutes", type=float, default=30)
    ap.add_argument("--workers", type=int, default=8)
    ap.add_argument("--pid", type=int)
    ap.add_argument("--psql-cmd")
    ap.add_argument("--storage")
    ap.add_argument("--out", required=True)
    ap.add_argument("--stability", action="store_true")
    ap.add_argument("--metrics")
    ap.add_argument("--owner-psql-cmd")
    a = ap.parse_args()
    api = Api(a.base, json.loads(Path(a.creds).read_text()))
    for role in ("analyst", "viewer", "auditor"):
        assert api.login(role) == 200, role
    totp_file = Path(a.creds + ".totp.json")  # TOTP secrets survive between runs on the same database
    if totp_file.exists():
        api.totp = json.loads(totp_file.read_text())
        for role in api.totp:
            assert api.login(role) == 200, role
    else:
        for role in ("reviewer", "reviewer2") + (("admin",) if a.stability else ()):  # reviews/decisions need MFA
            api.enroll_mfa(role)
        totp_file.write_text(json.dumps(api.totp))
        os.chmod(totp_file, 0o600)

    files = []
    for d in sorted(Path(a.datasets).iterdir()):
        for m in json.loads((d / "manifest.json").read_text()):
            if m["expect"] == "accepted":  # malicious uploads trigger the upload-restriction incident response
                files.append((d / "files" / m["file"], m["kind"], m["expect"]))
    file_lock, fidx = threading.Lock(), [0]

    portal = stability_setup(api, a) if a.stability else {"sups": [], "requests": {}}
    pdfs = [f for f, _, _ in files if f.suffix == ".pdf"][:500]
    _, me = api.req("GET", "/api/v1/auth/me", "reviewer")
    reviewer_id = me.get("user_id") if isinstance(me, dict) else None
    _, me2 = api.req("GET", "/api/v1/auth/me", "reviewer2")
    reviewer2_id = me2.get("user_id") if isinstance(me2, dict) else None

    lat = defaultdict(list)
    status = defaultdict(lambda: defaultdict(int))
    errors = []
    st_lock = threading.Lock()
    deadline = time.time() + a.minutes * 60
    stop = threading.Event()
    known = {"artifacts": [], "decisions": []}

    def call(op, role, fn):
        t = time.perf_counter()
        s, b = fn()
        if s == 401:  # access token expired: log in again once
            api.login(role)
            s, b = fn()
        dt = time.perf_counter() - t
        with st_lock:
            lat[op].append(dt)
            status[op][str(s)] += 1
            if s == -1 or s >= 500:
                errors.append({"t": round(time.time()), "op": op, "status": s, "body": str(b)[:300]})
        return s, b

    idem = {"sent": 0, "replayed": 0, "mismatch": 0}

    def mut(op, role, method, path, body=None, raw=None, ctype=None):
        """A mutation; with --stability half carry an Idempotency-Key and a fifth of those are retried at once."""
        if not a.stability or random.random() < 0.5:
            return call(op, role, lambda: api.req(method, path, role, body=body, raw=raw, ctype=ctype))
        h = {"Idempotency-Key": uuid.uuid4().hex}
        s, b = call(op, role, lambda: api.req(method, path, role, body=body, raw=raw, ctype=ctype, headers=h))
        with st_lock:
            idem["sent"] += 1
        if s != -1 and s < 500 and s not in (401, 403, 408, 429) and random.random() < 0.2:
            s2, b2 = call("idem_replay", role, lambda: api.req(method, path, role, body=body, raw=raw, ctype=ctype,
                                                             headers=h))
            with st_lock:
                idem["replayed"] += 1
                if s2 in (429, 503):
                    idem.setdefault("replay_deferred", 0)
                    idem["replay_deferred"] += 1  # admission/outage: "try again later", not a different answer
                elif (s2, b2) != (s, b):
                    idem["mismatch"] += 1
                    errors.append({"t": round(time.time()), "op": "idem_replay_mismatch", "status": s2,
                                   "body": f"{op}: first {s} {str(b)[:120]} / replay {s2} {str(b2)[:120]}"})
        return s, b

    def op_upload():
        with file_lock:
            if fidx[0] >= len(files):
                fidx[0] = 0  # wrap: re-uploads become duplicates (recapture path)
            f, kind, expect = files[fidx[0]]
            fidx[0] += 1
        raw, ctype = api.multipart(f.name, f.read_bytes(), kind)
        s, b = mut("upload", "analyst", "POST", "/api/v1/artifacts", raw=raw, ctype=ctype)
        if s == 200 and isinstance(b, dict) and b["artifact"]["status"] == "accepted" and not b.get("duplicate"):
            aid = b["artifact"]["id"]
            s, x = mut("extract", "analyst", "POST", f"/api/v1/artifacts/{aid}/extract", body={})
            if s == 200:
                with st_lock:
                    known["artifacts"].append(aid)

    def pick_claims(role="reviewer"):
        s, cl = call("claims_list", role, lambda: api.req("GET", "/api/v1/claims", role))
        return cl if s == 200 and isinstance(cl, list) else []

    def op_review_claim():
        cl = [c for c in pick_claims() if c.get("review_status") in ("unreviewed", "ai_suggested", None)]
        if cl:
            c = random.choice(cl)
            mut("claim_review", "reviewer", "POST", f"/api/v1/claims/{c['id']}/review",
                body={"decision": random.choice(["approve", "reject", "request_evidence"]), "comment": "soak"})

    def op_conflict():
        s, cf = call("conflicts_list", "viewer", lambda: api.req("GET", "/api/v1/conflicts?status=open", "viewer"))
        if s == 200 and isinstance(cf, list) and cf:
            c = random.choice(cf)
            call("conflict_get", "viewer", lambda: api.req("GET", f"/api/v1/conflicts/{c['id']}", "viewer"))
            mut("conflict_review", "reviewer2", "POST", f"/api/v1/conflicts/{c['id']}/review",
                body={"decision": random.choice(["approve", "request_evidence"]), "comment": "soak"})

    def op_verify_run():
        call("verification_run", "analyst", lambda: api.req("POST", "/api/v1/verification/run", "analyst", body={}))

    def op_decision():
        cl = pick_claims("viewer")
        if not cl:
            return
        c = random.choice(cl)
        same = [x for x in cl if x["subject_key"] == c["subject_key"]][:3]
        s, d = mut("decision_create", "reviewer", "POST", "/api/v1/decisions", body={
            "kind": "supplier_part_release", "title": f"Soak release {c['subject_key']}",
            "subject_type": c["subject_type"], "subject_key": c["subject_key"], "outcome": "approved",
            "rationale": "soak", "evidence": [{"claim_id": x["id"]} for x in same], "depends_on": []})
        if s == 200:
            did = d["id"]
            with st_lock:
                known["decisions"].append(did)
            call("decision_trace", "viewer", lambda: api.req("GET", f"/api/v1/decisions/{did}/trace", "viewer"))
            call("decision_report", "auditor", lambda: api.req("GET", f"/api/v1/decisions/{did}/report", "auditor"))

    def op_supersede():
        cl = [c for c in pick_claims() if c.get("value_num") is not None]
        if cl:
            c = random.choice(cl)
            body = {k: c[k] for k in ("subject_type", "subject_key", "predicate", "value_unit", "revision", "scope")
                    if c.get(k) is not None}
            body["value_num"] = str(round(float(c["value_num"]) + random.choice([0.01, 1, 5]), 3))
            body["reason"] = "soak correction"
            # As the analyst: a reviewer may not review claims they recorded (no_self_review), and repeated
            # denials would (correctly) trip the authorization-probing incident rule.
            mut("supersede", "analyst", "POST", f"/api/v1/claims/{c['id']}/supersede", body=body)

    def op_report():
        call("org_report", "auditor", lambda: api.req("POST", "/api/v1/reports", "auditor",
                                                       body={"scope": random.choice(["open_findings", "all"])}))

    def op_read():
        call("dashboard", "viewer", lambda: api.req("GET", "/api/v1/dashboard/summary", "viewer"))
        call("artifacts_list", "viewer", lambda: api.req("GET", "/api/v1/artifacts", "viewer"))

    def op_audit():
        s, v = call("audit_verify", "auditor", lambda: api.req("GET", "/api/v1/audit/verify", "auditor"))
        if s == 200 and v.get("intact") is not True:
            with st_lock:
                errors.append({"t": round(time.time()), "op": "audit_verify", "status": s, "body": str(v)[:300]})

    def op_verify_job():
        s, j = mut("verification_job", "analyst", "POST", "/api/v1/verification/jobs", body={})
        if s == 202 and isinstance(j, dict):
            call("job_get", "analyst", lambda: api.req("GET", f"/api/v1/jobs/{j['job_id']}", "analyst"))

    def op_portal():
        if not portal["sups"]:
            return
        role = random.choice(portal["sups"])
        reqs = portal["requests"][role]
        call("portal_list", role, lambda: api.req("GET", "/api/v1/portal/requests", role))
        rid = random.choice(reqs)
        call("portal_get", role, lambda: api.req("GET", f"/api/v1/portal/requests/{rid}", role))
        if pdfs and random.random() < 0.5:
            f = random.choice(pdfs)
            raw, ctype = api.multipart(f"{uuid.uuid4().hex[:8]}-{f.name}", f.read_bytes() + uuid.uuid4().hex.encode(), None)
            mut("portal_upload", role, "POST", f"/api/v1/portal/requests/{rid}/uploads", raw=raw, ctype=ctype)
        call("portal_uploads", role, lambda: api.req("GET", f"/api/v1/portal/requests/{rid}/uploads", role))
        call("portal_internal_view", "admin",
             lambda: api.req("GET", f"/api/v1/evidence-requests/{rid}/portal-uploads", "admin"))

    def op_capa():
        s, cf = call("conflicts_list", "viewer", lambda: api.req("GET", "/api/v1/conflicts?status=open", "viewer"))
        if s == 200 and isinstance(cf, list) and cf and reviewer_id:
            c = random.choice(cf)
            s, o = mut("capa_open", "reviewer", "POST", "/api/v1/capa/cases",
                       body={"source_kind": "conflict", "source_id": c["id"]})
            if s == 200 and isinstance(o, dict):
                case = o["case"]["id"]
                base = f"/api/v1/capa/cases/{case}"
                mut("capa_step_save", "reviewer", "PUT", f"{base}/steps/1",
                    body={"content": {"members": [{"user": reviewer_id, "role": "leader"},
                                                {"user": reviewer2_id, "role": "member"}]}})
                mut("capa_step_complete", "reviewer", "POST", f"{base}/steps/1/complete", body={})
                links = o.get("source_links") or []
                mut("capa_step_save", "reviewer", "PUT", f"{base}/steps/2", body={"content": {
                    "statement": "soak: values disagree", "is_is_not": [{"dimension": "what", "is": "x", "is_not": "y"}],
                    "evidence": links[:1]}})
                mut("capa_step_complete", "reviewer", "POST", f"{base}/steps/2/complete", body={})
                call("capa_get", "reviewer", lambda: api.req("GET", base, "reviewer"))
        call("capa_list", "reviewer", lambda: api.req("GET", "/api/v1/capa/cases", "reviewer"))
        call("capa_notifications", "reviewer", lambda: api.req("GET", "/api/v1/capa/notifications", "reviewer"))

    mix = [(op_upload, 30), (op_review_claim, 15), (op_conflict, 10), (op_verify_run, 4), (op_decision, 10),
           (op_supersede, 8), (op_report, 3), (op_read, 15), (op_audit, 2)]
    if a.stability:
        # Uploads lighter than the 30-min chaos soak (data growth over 2 h), plus jobs, portal and 8D.
        mix = [(op_upload, 20), (op_review_claim, 12), (op_conflict, 9), (op_verify_run, 2), (op_verify_job, 3),
               (op_decision, 9), (op_supersede, 7), (op_report, 2), (op_read, 14), (op_audit, 2), (op_portal, 12),
               (op_capa, 8)]
    fns, weights = zip(*mix)

    def worker(i):
        rnd = random.Random(i)
        while time.time() < deadline and not stop.is_set():
            rnd.choices(fns, weights)[0]()

    samples = []

    def sampler():
        while not stop.is_set():
            s = {"t": round(time.time())}
            if a.pid:
                try:
                    for line in open(f"/proc/{a.pid}/status"):
                        if line.startswith(("VmRSS", "Threads")):
                            k, v = line.split(":")
                            s[k] = int(v.split()[0])
                    s["fds"] = len(os.listdir(f"/proc/{a.pid}/fd"))
                    kids = subprocess.run(["ps", "--ppid", str(a.pid), "--no-headers"], capture_output=True,
                                          text=True).stdout
                    s["children"] = len([x for x in kids.splitlines() if x.strip()])
                except OSError:
                    s["server"] = "gone"
            if a.metrics:
                try:
                    with urllib.request.urlopen(a.metrics, timeout=10) as r:
                        for line in r.read().decode().splitlines():
                            for k in ("kizuna_db_pool_connections", "kizuna_jobs_queue_depth", "kizuna_http_in_flight",
                                      "kizuna_admission_rejected_total", "kizuna_admission_waiting"):
                                if line.startswith(k):
                                    name, v = line.rsplit(" ", 1)
                                    s.setdefault("m", {})[name.replace("kizuna_", "")] = float(v)
                except Exception as e:  # noqa: BLE001 - sampler must not die
                    s["metrics_error"] = str(e)[:100]
            if a.psql_cmd:
                out = subprocess.run(a.psql_cmd.split() + ["-qAt", "-c", "SELECT count(*) FROM pg_stat_activity WHERE "
                                     "datname = current_database() AND pid <> pg_backend_pid()"],
                                     capture_output=True, text=True).stdout.strip()
                s["db_conns"] = int(out) if out.isdigit() else None
            samples.append(s)
            stop.wait(10)

    th = [threading.Thread(target=worker, args=(i,)) for i in range(a.workers)]
    sm = threading.Thread(target=sampler, daemon=True)
    sm.start()
    t0 = time.time()
    for t in th:
        t.start()
    for t in th:
        t.join()
    stop.set()
    sm.join(timeout=15)
    elapsed = time.time() - t0
    ops = {}
    for op, xs in sorted(lat.items()):
        ops[op] = {"n": len(xs), "p50_ms": pct(xs, 50), "p95_ms": pct(xs, 95), "p99_ms": pct(xs, 99),
                   "max_ms": round(max(xs) * 1000, 1), "status": dict(status[op])}
    allx = [x for xs in lat.values() for x in xs]
    res = {
        "minutes": round(elapsed / 60, 2), "workers": a.workers, "requests": len(allx),
        "rps": round(len(allx) / elapsed, 2),
        "overall": {"p50_ms": pct(allx, 50), "p95_ms": pct(allx, 95), "p99_ms": pct(allx, 99)},
        "errors_5xx_or_transport": len(errors), "error_samples": errors[:30],
        "artifacts_extracted": len(known["artifacts"]), "decisions": len(known["decisions"]), "idempotency": idem,
        "ops": ops, "samples": samples,
    }
    Path(a.out).write_text(json.dumps(res, indent=1))
    print(json.dumps({k: v for k, v in res.items() if k not in ("ops", "samples", "error_samples")}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
