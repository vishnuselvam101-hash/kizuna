"""Fits the confidence calibration table of the Rust mapper (domain-manufacturing src/calibration.json).

Input: a `ja_eval.py --out` result (Rust pipeline claim rows, strict correctness). Fit set: train + dev +
dev_scan, never the independent holdout. Per group (`text|ocr` / `rule|fallback`): isotonic regression
(pool-adjacent-violators) of correctness on OCR confidence, blocks of at least 10 values, estimates smoothed
as (k+1)/(n+2) and re-pooled so they stay monotone. Tenant-synonym groups reuse the rule groups (a reviewer
confirmed the label mapping; not measurable on a synthetic corpus). The review gate is the smallest
threshold that, together with the mapper's fixed flags (OCR confidence below 0.80, guessed labels or
subjects), flags at least 90 % of the wrong values on dev + dev_scan.

    uv run python tests/ja_calibrate.py ev.json --write ../../crates/domain-manufacturing/src/calibration.json
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

FIT = ("train", "dev", "dev_scan")
GATE_SPLITS = ("dev", "dev_scan")
OCR_FLOOR_BPS = 8000  # mapping::OCR_REVIEW_FLOOR
TARGET_CATCH = 0.90
MIN_BLOCK = 10  # values per calibration step (fewer gives noisy, over-cautious steps)


def group(r: dict) -> str:
    src = "ocr" if r.get("ocr_conf_bps") is not None else "text"
    basis = "fallback" if r.get("subject_basis") == "fallback" else r.get("label_basis") or "rule"
    return f"{src}/{basis}"


def pav(points: list[tuple[int, float, float]]) -> list[list[float]]:
    """points (x, y-sum, weight) sorted by x → blocks [x_max, y_sum, weight] with non-decreasing means."""
    blocks: list[list[float]] = []
    for x, y, w in points:
        blocks.append([x, y, w])
        while len(blocks) > 1 and blocks[-2][1] / blocks[-2][2] > blocks[-1][1] / blocks[-1][2]:
            x2, y2, w2 = blocks.pop()
            blocks[-1] = [x2, blocks[-1][1] + y2, blocks[-1][2] + w2]
    return blocks


def fit_group(rows: list[dict]) -> list[list[int]]:
    by_x: dict[int, list[int]] = {}
    for r in rows:
        x = min(10_000, r["ocr_conf_bps"] if r.get("ocr_conf_bps") is not None else 10_000)
        by_x.setdefault(x, []).append(1 if r["ok"] else 0)
    pts = [(x, float(sum(v)), float(len(v))) for x, v in sorted(by_x.items())]
    raw: list[list[float]] = []
    for blk in pav(pts):  # merge isotonic blocks until each holds at least MIN_BLOCK values
        if raw and raw[-1][2] < MIN_BLOCK:
            raw[-1] = [blk[0], raw[-1][1] + blk[1], raw[-1][2] + blk[2]]
        else:
            raw.append(blk)
    if len(raw) > 1 and raw[-1][2] < MIN_BLOCK:
        last = raw.pop()
        raw[-1] = [last[0], raw[-1][1] + last[1], raw[-1][2] + last[2]]
    smooth = pav([(int(b[0]), (b[1] + 1) / (b[2] + 2) * b[2], b[2]) for b in raw])
    steps = [[int(b[0]), round(10_000 * b[1] / b[2])] for b in smooth]
    steps[-1][0] = 10_000
    return steps


def p_of(steps: list[list[int]], conf: int | None) -> int:
    c = 10_000 if conf is None else min(conf, 10_000)
    return next((p for upper, p in steps if c <= upper), steps[-1][1])


def flagged(r: dict, p: int, gate: int) -> bool:
    forced = (r.get("ocr_conf_bps") is not None and r["ocr_conf_bps"] < OCR_FLOOR_BPS) or group(r).endswith(
        "/fallback"
    )
    return forced or p < gate


def fit(rows: list[dict]) -> dict:
    fit_rows = [r for r in rows if r["split"] in FIT and not r["field"].startswith("pdf_info")]
    groups: dict[str, list[list[int]]] = {}
    base = {"text/rule", "ocr/rule", "text/fallback", "ocr/fallback"}
    for g in sorted({group(r) for r in fit_rows} | base):
        gr = [r for r in fit_rows if group(r) == g]
        groups[g] = fit_group(gr) if gr else [[10_000, 0]]
    for src in ("text", "ocr"):
        groups[f"{src}/tenant_synonym"] = groups[f"{src}/rule"]
    dev = [r for r in rows if r["split"] in GATE_SPLITS]
    wrong = [r for r in dev if not r["ok"]]
    cands = sorted({p for s in groups.values() for _, p in s} | {0, 10_000})
    gate = 10_000
    for t in cands:
        caught = sum(flagged(r, p_of(groups[group(r)], r.get("ocr_conf_bps")), t) for r in wrong)
        if not wrong or caught / len(wrong) >= TARGET_CATCH:
            gate = t
            break
    return {"version": "cal@1", "fitted_on": "+".join(FIT), "gate_bps": min(gate, 10_000), "groups": groups}


def report(table: dict, rows: list[dict]) -> dict:
    out = {}
    for split in sorted({r["split"] for r in rows}):
        rs = [r for r in rows if r["split"] == split]
        gate = table["gate_bps"]
        fl = [flagged(r, p_of(table["groups"][group(r)], r.get("ocr_conf_bps")), gate) for r in rs]
        wrong = [f for r, f in zip(rs, fl, strict=True) if not r["ok"]]
        out[split] = {
            "claims": len(rs),
            "wrong": len(wrong),
            "caught": sum(wrong),
            "catch_rate": round(sum(wrong) / len(wrong), 3) if wrong else None,
            "review_load": round(sum(fl) / len(rs), 3) if rs else None,
        }
    return out


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("eval_json")
    ap.add_argument("--write")
    a = ap.parse_args()
    rows = json.loads(Path(a.eval_json).read_text())["claims"]
    table = fit(rows)
    print(json.dumps({"gate_bps": table["gate_bps"], "groups": table["groups"]}, ensure_ascii=False))
    for split, v in report(table, rows).items():
        print(split, v)
    if a.write:
        Path(a.write).write_text(json.dumps(table, ensure_ascii=False, indent=1) + "\n")


if __name__ == "__main__":
    main()
