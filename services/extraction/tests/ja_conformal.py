"""Measures the review gate's coverage of wrong values (MATH v2 (b)): split-conformal gate vs the previous
empirical rule ("smallest threshold that catches 90 % on the calibration set"), on the claim rows of a
`ja_eval.py --holdout --out` run. Prints JSON. Deterministic (seeded splits).

    uv run python tests/ja_conformal.py ev.json [--reps 1000] [--seed 20261106]

E1  random 50/50 splits of the dev + dev_scan wrong values (exchangeable; note the calibration table itself
    was fitted on train + dev + dev_scan, so the scores are not fully independent of these values).
E2  random 50/50 splits of all evaluated wrong values (dev, dev_scan, holdout, holdout_scan): exchangeable
    by construction - what the guarantee gives once reviewed documents of new layouts join calibration.
E3  fit on dev + dev_scan, measure on the independent holdout (not exchangeable: a separate generator).
Coverage = share of test wrong values flagged; load = share of all test values flagged.
"""

from __future__ import annotations

import argparse
import json
import random
import statistics
from pathlib import Path

import ja_calibrate as jc

TARGET = jc.TARGET_CATCH


def scored(rows: list[dict]) -> list[tuple[dict, int]]:
    table = json.loads(
        (Path(__file__).parents[3] / "crates/domain-manufacturing/src/calibration.json").read_text()
    )
    return [(r, jc.score(r, jc.p_of(jc.steps_of(table["groups"], jc.group(r)), jc.conf(r)))) for r in rows]


def empirical_threshold(scores: list[int]) -> int:
    """Previous rule: the smallest t with share(score <= t) >= 90 % on the calibration set itself."""
    s = sorted(scores)
    for t in s:
        if sum(x <= t for x in s) / len(s) >= TARGET:
            return t
    return s[-1]


def share(scores: list[int], t: int | None) -> float:
    return sum(t is None or x <= t for x in scores) / len(scores) if scores else float("nan")


def splits(wrong: list[int], right: list[int], reps: int, seed: int) -> dict:
    rng = random.Random(seed)
    out: dict[str, list] = {"conformal": [], "empirical": []}
    for _ in range(reps):
        w = wrong[:]
        rng.shuffle(w)
        r = right[:]
        rng.shuffle(r)
        cal, test = w[: len(w) // 2], w[len(w) // 2 :]
        test_all = test + r[len(r) // 2 :]
        for name, t in (("conformal", jc.conformal_threshold(cal)), ("empirical", empirical_threshold(cal))):
            out[name].append((share(test, t), share(test_all, t)))
    res = {}
    for name, v in out.items():
        cov = [c for c, _ in v]
        res[name] = {
            "mean_coverage": round(statistics.fmean(cov), 4),
            "p05_coverage": round(sorted(cov)[len(cov) // 20], 4),
            "share_splits_ge_target": round(sum(c >= TARGET for c in cov) / len(cov), 4),
            "mean_review_load": round(statistics.fmean(ld for _, ld in v), 4),
        }
    res["calibration_wrong"] = len(wrong) // 2
    res["test_wrong"] = len(wrong) - len(wrong) // 2
    return res


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("eval_json")
    ap.add_argument("--reps", type=int, default=1000)
    ap.add_argument("--seed", type=int, default=20261106)
    a = ap.parse_args()
    claims = json.loads(Path(a.eval_json).read_text())["claims"]
    rows = [r for r in claims if not r["field"].startswith("pdf_info")]
    sc = scored(rows)
    dev = ("dev", "dev_scan")
    hold = ("holdout", "holdout_scan")

    def pick(splits_: tuple, ok: bool) -> list[int]:
        return [s for r, s in sc if r["split"] in splits_ and r["ok"] == ok]

    e3 = {}
    cal = pick(dev, False)
    for name, t in (("conformal", jc.conformal_threshold(cal)), ("empirical", empirical_threshold(cal))):
        hw, ha = pick(hold, False), pick(hold, False) + pick(hold, True)
        e3[name] = {
            "threshold": t,
            "holdout_coverage": round(share(hw, t), 4),
            "holdout_review_load": round(share(ha, t), 4),
            "holdout_wrong": len(hw),
        }
    hw = pick(hold, False)
    unflagged = sorted(s for s in hw if s > -1)
    t_hold = jc.conformal_threshold(hw)
    e3["needed_for_90pct_on_holdout"] = {
        "threshold": t_hold,
        "holdout_review_load": round(share(pick(hold, False) + pick(hold, True), t_hold), 4),
        "unforced_wrong_scores": unflagged,
    }
    res = {
        "command": f"tests/ja_conformal.py {Path(a.eval_json).name} --reps {a.reps} --seed {a.seed}",
        "target": TARGET,
        "E1_dev_splits": splits(pick(dev, False), pick(dev, True), a.reps, a.seed),
        "E2_pooled_splits": splits(pick(dev + hold, False), pick(dev + hold, True), a.reps, a.seed + 1),
        "E3_dev_to_holdout": e3,
    }
    print(json.dumps(res, indent=1))


if __name__ == "__main__":
    main()
