"""Scores extraction of the Japanese corpus (tests/ja_corpus.py) per field: precision / recall.

Two levels: `strict` needs the canonical value (ISO date, number + canonical unit, tolerance plus/minus,
result enum, NFKC text); `detect` also accepts the printed text. Two pipelines: the worker's claims and the
production path worker records → Rust `map_records` (via `cargo run --example map_records`).

    uv run python tests/ja_eval.py --out /path/results.json [--rust /path/to/map_records-binary]
"""

from __future__ import annotations

import argparse
import base64
import json
import subprocess
import sys
import time
import unicodedata
from collections import defaultdict
from decimal import Decimal, InvalidOperation
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
sys.path.insert(0, str(Path(__file__).parents[1] / "src"))

from ja_corpus import Doc, build  # noqa: E402

from kizuna_extraction.worker import handle  # noqa: E402

RUST_SKIP = {"part_number", "po_number", "po_line", "lot", "heat_number"}  # subject/scope in Rust, not claims


def norm_text(s: object) -> str:
    return " ".join(unicodedata.normalize("NFKC", str(s)).casefold().split())


def dec(s: object) -> Decimal | None:
    try:
        return Decimal(str(s)).normalize()
    except (InvalidOperation, ValueError):
        return None


def canon_truth(t: dict) -> tuple:
    if "date" in t:
        return ("date", t["date"])
    if "plus" in t:
        return ("tol", dec(t["plus"]), dec(t["minus"]))
    if "number" in t:
        return ("num", dec(t["number"]), t["unit"].lower())
    return ("text", norm_text(t["text"]))


def canon_claim(c: dict) -> tuple:
    if c.get("date"):
        return ("date", c["date"])
    if "plus" in c:
        return ("tol", dec(c["plus"]), dec(c["minus"]))
    if c["field"] == "tolerance" and "number" in c:
        return ("tol", dec(c["number"]), dec(c["number"]))
    if c.get("number") is not None:
        return ("num", dec(c["number"]), (c.get("unit") or "").lower())
    if c.get("norm") is not None:
        return ("text", norm_text(c["norm"]))
    return ("text", norm_text(c.get("value", "")))


def match(truth: list[dict], claims: list[dict], lenient: bool) -> tuple[dict, dict, dict]:
    tp: dict[str, int] = defaultdict(int)
    fn: dict[str, int] = defaultdict(int)
    fp: dict[str, int] = defaultdict(int)
    used = [False] * len(claims)
    cc = [canon_claim(c) for c in claims]
    pending = []
    for t in truth:
        ct = canon_truth(t)
        hit = next(
            (i for i, c in enumerate(claims) if not used[i] and c["field"] == t["field"] and cc[i] == ct),
            None,
        )
        if hit is None:
            pending.append(t)
        else:
            used[hit] = True
            tp[t["field"]] += 1
    for t in pending:
        hit = None
        if lenient:
            raw = norm_text(t.get("raw", ""))
            hit = next(
                (
                    i
                    for i, c in enumerate(claims)
                    if not used[i] and c["field"] == t["field"] and norm_text(c.get("value", "")) == raw
                ),
                None,
            )
        if hit is None:
            fn[t["field"]] += 1
        else:
            used[hit] = True
            tp[t["field"]] += 1
    for i, c in enumerate(claims):
        if not used[i]:
            fp[c["field"]] += 1
    return tp, fp, fn


def worker_run(doc: Doc) -> dict:
    req = {"media_type": doc.media_type, "data_b64": base64.b64encode(doc.data).decode()}
    return handle(json.dumps(req).encode())


def rust_fields(mapped: dict) -> list[dict]:
    """Rust ClaimInput → {field, canonical keys}; tolerance plus/minus pairs are rejoined by location."""
    out: list[dict] = []
    tols: dict[tuple, dict] = {}
    for c in mapped.get("claims", []):
        p = c["predicate"]
        parts = p.split(".")
        if p.startswith("mfg.tolerance."):
            key = (json.dumps(c.get("source_location"), sort_keys=True), parts[2])
            tols.setdefault(key, {})[parts[3]] = c.get("value_num")
            continue
        rest = ".".join(parts[1:])
        f = {
            "approval.status": "approval_status",
            "approval.by": "approved_by",
            "approval.date": "approval_date",
            "approval.drawn_by": "drawn_by",
            "approval.checked_by": "checked_by",
            "certificate.expiry": "expiry",
            "revision_ordered": "revision",
            "revision_inspected": "revision",
            # mfg-map@3 shares predicate names with the three-way match / integrity signals (innov lanes)
            "certificate_date": "issue_date",
            "ship_date": "delivery_date",
        }.get(rest, rest)
        if f.startswith("chem."):
            f = "property.chem_" + f.removeprefix("chem.")
        if f.startswith("approval.mark."):
            f = "stamp." + f.removeprefix("approval.mark.")
        d: dict = {"field": f, "value": c.get("value_text") or c.get("value_num") or ""}
        if c.get("value_num") is not None:
            d["number"], d["unit"] = c["value_num"], c.get("value_unit")
        elif f.endswith(("date", "expiry", "sent_at")):
            d["date"] = c.get("value_text")
        out.append(d)
    for pm in tols.values():
        out.append({"field": "tolerance", "plus": pm.get("plus"), "minus": pm.get("minus"), "value": ""})
    return out


def worker_fields(claims: list[dict]) -> list[dict]:
    return [c for c in claims if not c["field"].startswith("pdf_info")]


def summarise(rows: list[tuple[str, dict, dict, dict]]) -> dict:
    per: dict[str, dict[str, int]] = defaultdict(lambda: {"tp": 0, "fp": 0, "fn": 0})
    for _split, tp, fp, fn in rows:
        for k, v in tp.items():
            per[k]["tp"] += v
        for k, v in fp.items():
            per[k]["fp"] += v
        for k, v in fn.items():
            per[k]["fn"] += v

    def pr(d: dict) -> dict:
        p = d["tp"] / (d["tp"] + d["fp"]) if d["tp"] + d["fp"] else 0.0
        r = d["tp"] / (d["tp"] + d["fn"]) if d["tp"] + d["fn"] else 0.0
        f1 = 2 * p * r / (p + r) if p + r else 0.0
        return {**d, "precision": round(p, 3), "recall": round(r, 3), "f1": round(f1, 3)}

    tot = {"tp": 0, "fp": 0, "fn": 0}
    for d in per.values():
        for k in tot:
            tot[k] += d[k]
    return {"micro": pr(tot), "fields": {k: pr(v) for k, v in sorted(per.items())}}


def evaluate(docs: list[Doc], rust_bin: str | None = None) -> dict:
    res: dict = {}
    rows: dict[str, list] = defaultdict(list)
    rust_in = []
    t0 = time.monotonic()
    failures = []
    for d in docs:
        r = worker_run(d)
        if not r.get("ok"):
            failures.append((d.name, r.get("error")))
            r = {"claims": [], "records": []}
        claims = worker_fields(r["claims"])
        for level in ("strict", "detect"):
            tp, fp, fn = match(d.truth, claims, level == "detect")
            rows[f"worker/{level}/{d.split}"].append((d.split, tp, fp, fn))
            rows[f"worker/{level}/all"].append((d.split, tp, fp, fn))
        rust_in.append({"kind": d.kind, "records": r["records"]})
    res["worker_seconds"] = round(time.monotonic() - t0, 2)
    if rust_bin:
        inp = "\n".join(json.dumps(x, ensure_ascii=False) for x in rust_in).encode()
        out = subprocess.run([rust_bin], input=inp, capture_output=True, check=True, timeout=600).stdout  # noqa: S603
        for d, line in zip(docs, out.decode().splitlines(), strict=True):
            claims = rust_fields(json.loads(line))
            truth = [t for t in d.truth if t["field"] not in RUST_SKIP]
            for level in ("strict", "detect"):
                tp, fp, fn = match(truth, claims, level == "detect")
                rows[f"rust/{level}/{d.split}"].append((d.split, tp, fp, fn))
                rows[f"rust/{level}/all"].append((d.split, tp, fp, fn))
    res["failures"] = failures
    res["docs"] = {s: sum(1 for d in docs if d.split == s) for s in sorted({d.split for d in docs})}
    res["results"] = {k: summarise(v) for k, v in sorted(rows.items())}
    return res


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--rust")
    ap.add_argument("--no-scans", action="store_true")
    a = ap.parse_args()
    docs = build(scans=not a.no_scans)
    res = evaluate(docs, a.rust)
    Path(a.out).write_text(json.dumps(res, ensure_ascii=False, indent=1))
    for k, v in res["results"].items():
        m = v["micro"]
        print(
            f"{k:28s} P={m['precision']:.3f} R={m['recall']:.3f} F1={m['f1']:.3f} "
            f"tp={m['tp']} fp={m['fp']} fn={m['fn']}"
        )
    print("docs", res["docs"], "failures", res["failures"], "worker_s", res["worker_seconds"])


if __name__ == "__main__":
    main()
