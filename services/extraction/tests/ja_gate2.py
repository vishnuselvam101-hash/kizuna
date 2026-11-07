"""Gate v2 (queue item 28): a consistency-based nonconformity score for the review gate, and its measurement.

The MATH v2 gate ranked values by calibrated p_correct alone; on the independent holdout 40 of 60 wrong values
had p_correct >= 0.996, so no threshold on that score could catch them. This score first counts independent
*inconsistency signals* a value raises, each computed from what the extractor and mapper already produce
(records, mapped claims, layout provenance); p_correct only orders values that raise none:

  forced      the mapper always flags it (OCR < 0.80, guessed label or subject) - the previous gate
  shape       value does not have the shape of its field (date, enum, grade code, person/company name, ...)
  unit        unit does not belong to the field's dimension (length, stress, %, hardness, ...)
  range       outside a physical range for the field (generic engineering bounds, not fitted to the corpus)
  spec        measured vs nominal +/- tolerance grossly inconsistent, or tolerance implausible for its nominal
  cross       cross-field: yield >= tensile, chemistry sum > 100 %, result vs measured-in-tolerance disagree
  dup         a single-valued field of one subject mapped twice with different values in one document
  table       table structure: the value's table row/column has a different cell count from the table's mode
  novel       the printed label (table cell: its column header) never occurs in the calibration documents
  xdoc        cross-document: the same part / heat has a different value for a single-valued spec field
              (material, nominal dimension, tolerance, drawing number, part name) in another document

score = -10 000 * (number of signal families raised) when at least one is raised, else p_correct_bps (see
`score`); lower = more suspicious. The split-conformal threshold (k = ceil(0.9 (n + 1))-th smallest score of
the n calibration wrong values, flag score <= it) is fitted on dev + dev_scan only. The independent holdout
is evaluated once, after the signals and bounds were frozen.

    uv run python tests/ja_gate2.py ev.json [--signals shape,unit,...]   # ev.json from ja_eval.py --holdout
"""

from __future__ import annotations

import argparse
import json
import math
import re
import unicodedata
from collections import Counter, defaultdict
from pathlib import Path

import ja_calibrate as jc

CAL = ("dev", "dev_scan")  # conformal calibration split (wrong values) - as MATH v2
VOCAB = ("train", "dev", "dev_scan")  # documents whose labels count as "seen" (reviewed documents)
HOLD = ("holdout", "holdout_scan")
FAMILIES = ("forced", "shape", "unit", "range", "spec", "cross", "dup", "table", "novel", "xdoc")
PDF_INFO = {"title", "author", "creator", "producer", "creationdate", "moddate", "subject", "keywords"}


def nfkc(s: object) -> str:
    return " ".join(unicodedata.normalize("NFKC", str(s or "")).split())


def num(x: object) -> float | None:
    try:
        f = float(str(x))
    except (TypeError, ValueError):
        return None
    return f if math.isfinite(f) else None


def leaf(pred: str) -> str:
    return (pred or "").split(".", 1)[-1]


# ------------------------------------------------------------------ shape / unit / range (per value)
DATE_FIELDS = ("issue_date", "inspection_date", "delivery_date", "approval.date", "certificate.expiry")
PERSON_FIELDS = ("approval.by", "approval.checked_by", "approval.drawn_by")
COMPANY = re.compile(
    r"(株式会社|有限会社|合同会社|\(株\)|\(有\)|工業|製作所|製鋼|金属|精工|"
    r"精機|鋼業|商事|産業|co\.|ltd|inc|corp|gmbh|kk\b)",
    re.I,
)
GRADE = re.compile(r"^[A-Z]{1,6}[0-9A-Z\-]*$")
REVISION = re.compile(r"^[A-Z0-9]{1,3}$")
SCALE = re.compile(r"^\d+(\.\d+)?:\d+(\.\d+)?$")
ISO_DATE = re.compile(r"^(\d{4})-(\d{2})-(\d{2})")
DIGITS = re.compile(r"\d")
LENGTH_UNITS = {None, "", "mm", "m", "cm", "um", "µm", "μm", "in", "inch"}
STRESS_UNITS = {"mpa", "n/mm2", "n/mm²", "ksi", "gpa"}
# generic engineering bounds (machined/sheet parts and steel/aluminium certificates), written from handbooks,
# deliberately wide; NOT fitted to the corpus values
RANGES = {
    "property.tensile_strength": (100, 2500),
    "property.yield_strength": (50, 2200),
    "property.elongation": (0.5, 80),
    "property.surface_roughness": (0.01, 50),
    "chem.c": (0, 2.5),
    "chem.si": (0, 5),
    "chem.mn": (0, 25),
    "chem.p": (0, 0.3),
    "chem.s": (0, 0.5),
    "chem.cr": (0, 35),
    "chem.ni": (0, 45),
    "chem.mo": (0, 10),
    "chem.cu": (0, 12),
}
HARDNESS = {"hrc": (10, 72), "hrb": (20, 110), "hv": (50, 1200), "hb": (50, 750), "hbw": (50, 750)}


def shape_ok(r: dict) -> bool:
    lf, t = leaf(r["predicate"]), nfkc(r.get("value_text"))
    if lf in DATE_FIELDS or lf.endswith("sent_at"):
        m = ISO_DATE.match(t)
        return bool(m) and 1990 <= int(m.group(1)) <= 2040
    if lf in PERSON_FIELDS:
        return 1 <= len(t) <= 20 and not DIGITS.search(t)
    if lf.endswith("supplier") or lf == "fax.sender":
        return bool(COMPANY.search(t)) and not ISO_DATE.match(t)
    if lf == "material":
        return bool(GRADE.match(t.upper().replace(" ", "")))
    if lf in ("revision", "revision_inspected", "revision_ordered"):
        return bool(REVISION.match(t.upper()))
    if lf == "scale":
        return bool(SCALE.match(t.replace(" ", "")))
    if lf == "part_name" or lf == "surface_treatment":
        # a name, not a number, date, code or person-length fragment
        return (
            2 <= len(t) <= 40
            and not ISO_DATE.match(t)
            and num(t) is None
            and not re.match(r"^[A-Z]{1,4}-?\d", t)
        )
    if lf == "result":
        return t in ("pass", "fail")
    if lf == "approval.status":
        return t in ("approved", "pending", "rejected")
    if lf in ("quantity", "unit_price"):
        v = num(r.get("value_num"))
        return v is not None and v > 0 and (lf != "quantity" or v == int(v))
    return True


def unit_ok(r: dict) -> bool:
    lf, u = leaf(r["predicate"]), (r.get("value_unit") or "").lower()
    if lf.startswith(("dimension.", "measured.", "tolerance.")):
        return u in LENGTH_UNITS
    if lf in ("property.tensile_strength", "property.yield_strength"):
        return u in STRESS_UNITS
    if lf == "property.elongation" or lf.startswith("chem."):
        return u in ("%", "")
    if lf.endswith("property.hardness"):
        return u in HARDNESS
    return True


def range_ok(r: dict) -> bool:
    lf, v = leaf(r["predicate"]), num(r.get("value_num"))
    if v is None:
        return True
    if lf.startswith(("dimension.", "measured.")):
        return 0 < v <= 10_000
    if lf.startswith("tolerance."):
        return 0 <= v <= 10
    if lf.endswith("property.hardness"):
        lo, hi = HARDNESS.get((r.get("value_unit") or "").lower(), (0, math.inf))
        return lo <= v <= hi
    if lf in RANGES:
        lo, hi = RANGES[lf]
        return lo <= v <= hi
    return True


# ------------------------------------------------------------------ cross-value checks (per document)
def dim_name(pred: str) -> str | None:
    p = pred.split(".")
    if pred.startswith(("mfg.dimension.", "qa.measured.", "mfg.tolerance.")):
        return p[2]
    return None


def doc_signals(rows: list[dict], meta: dict, vocab: set[str]) -> list[set[str]]:
    sig: list[set[str]] = [set() for _ in rows]
    for i, r in enumerate(rows):
        if jc.flagged(r, 10**9, 0):
            sig[i].add("forced")
        if not shape_ok(r):
            sig[i].add("shape")
        if not unit_ok(r):
            sig[i].add("unit")
        if not range_ok(r):
            sig[i].add("range")
        if r["field"] == "tolerance":
            p, m = num(r.get("plus")), num(r.get("minus"))
            if p is None or m is None or not (0 <= p <= 10 and 0 <= m <= 10) or p + m == 0:
                sig[i].add("range")

    # spec: measured vs nominal +/- tolerance, tolerance vs nominal (per subject + dimension)
    by_dim: dict[tuple, dict[str, list[int]]] = defaultdict(lambda: defaultdict(list))
    spec_of: dict[tuple, tuple[float, float]] = {}
    for i, r in enumerate(rows):
        d = dim_name(r.get("predicate") or "")
        if d:
            kind = r["predicate"].split(".")[1]
            by_dim[(r.get("subject"), d)][kind].append(i)
    for idx in by_dim.values():
        noms = {num(rows[i].get("value_num")) for i in idx.get("dimension", [])} - {None}
        tols = [(num(rows[i].get("plus")), num(rows[i].get("minus"))) for i in idx.get("tolerance", [])]
        tols = [(p, m) for p, m in tols if p is not None and m is not None]
        if len(noms) != 1:
            continue
        n = noms.pop()
        if len(tols) == 1:
            spec_of[
                (rows[idx["dimension"][0]].get("subject"), dim_name(rows[idx["dimension"][0]]["predicate"]))
            ] = (
                n - tols[0][1],
                n + tols[0][0],
            )
        width = max((p + m for p, m in tols), default=0.0) or max(0.002 * n, 0.01)
        for i in idx.get("tolerance", []):
            p, m = num(rows[i].get("plus")), num(rows[i].get("minus"))
            if p is not None and m is not None and n > 0 and (p + m) > 0.25 * n:
                sig[i].add("spec")
                for j in idx.get("dimension", []):
                    sig[j].add("spec")
        for i in idx.get("measured", []):
            v = num(rows[i].get("value_num"))
            if v is not None and abs(v - n) > 5 * width + 1e-9:
                sig[i].add("spec")
                for j in idx.get("dimension", []) + idx.get("tolerance", []):
                    sig[j].add("spec")

    # cross: yield vs tensile, chemistry sum (per subject)
    by_subj: dict[object, dict[str, list[int]]] = defaultdict(lambda: defaultdict(list))
    for i, r in enumerate(rows):
        by_subj[r.get("subject")][leaf(r.get("predicate") or "")].append(i)
    for d in by_subj.values():
        ys = d.get("property.yield_strength", [])
        ts = d.get("property.tensile_strength", [])
        if len(ys) == 1 and len(ts) == 1:
            y, t = num(rows[ys[0]].get("value_num")), num(rows[ts[0]].get("value_num"))
            if y is not None and t is not None and y >= t:
                sig[ys[0]].add("cross")
                sig[ts[0]].add("cross")
        chem = [i for k, v in d.items() if k.startswith("chem.") for i in v]
        if sum(num(rows[i].get("value_num")) or 0 for i in chem) > 100:
            for i in chem:
                sig[i].add("cross")

    # cross: a table row's (column's, htable) result disagrees with its measured values vs the specification
    line: dict[tuple, list[int]] = defaultdict(list)
    for i, r in enumerate(rows):
        loc = r.get("loc") or {}
        if loc.get("layout") in ("table", "htable") and "table" in loc:
            ax = loc.get("row") if loc["layout"] == "table" else loc.get("col")
            line[(r.get("subject"), loc.get("page"), loc["table"], loc["layout"], ax)].append(i)
    for idx in line.values():
        res = [i for i in idx if leaf(rows[i].get("predicate") or "") == "result"]
        meas = [i for i in idx if (rows[i].get("predicate") or "").startswith("qa.measured.")]
        if len(res) != 1 or not meas:
            continue
        inside = []
        for i in meas:
            lim = spec_of.get((rows[i].get("subject"), dim_name(rows[i]["predicate"])))
            v = num(rows[i].get("value_num"))
            if lim is None or v is None:
                break
            inside.append(lim[0] - 1e-9 <= v <= lim[1] + 1e-9)
        else:
            verdict = nfkc(rows[res[0]].get("value_text"))
            if (verdict == "pass" and not all(inside)) or (verdict == "fail" and all(inside)):
                for i in res + meas:
                    sig[i].add("cross")

    # dup: a single-valued field mapped twice for one subject with different values
    multi = ("measured.", "result", "approval.mark.", "tolerance.")
    seen: dict[tuple, list[int]] = defaultdict(list)
    for i, r in enumerate(rows):
        lf = leaf(r.get("predicate") or "")
        if lf.startswith(multi) or r["field"] == "tolerance":
            continue
        seen[(r.get("subject"), lf)].append(i)
    for idx in seen.values():
        vals = {(nfkc(rows[i].get("value_text")), rows[i].get("value_num")) for i in idx}
        if len(vals) > 1:
            for i in idx:
                sig[i].add("dup")

    # table structure + label novelty, from the extractor's records
    recs = meta.get("records", [])
    cells: Counter = Counter()
    for rec in recs:
        loc = rec.get("location") or {}
        if loc.get("layout") == "table" and "table" in loc:
            cells[(loc.get("page"), loc["table"], "r", loc.get("row"))] += 1
        elif loc.get("layout") == "htable" and "table" in loc:
            cells[(loc.get("page"), loc["table"], "c", loc.get("col"))] += 1
    mode: dict[tuple, int] = {}
    for (pg, tb, ax, _), n in cells.items():
        mode.setdefault((pg, tb, ax), Counter())[n] += 1  # type: ignore[index]
    mode = {k: c.most_common(1)[0][0] for k, c in mode.items()}  # type: ignore[union-attr]
    by_key = {loc_key(rec.get("location") or {}): rec for rec in recs}
    for i, r in enumerate(rows):
        loc = r.get("loc") or {}
        if loc.get("layout") in ("table", "htable") and "table" in loc:
            ax, pos = ("r", loc.get("row")) if loc["layout"] == "table" else ("c", loc.get("col"))
            n = cells.get((loc.get("page"), loc["table"], ax, pos))
            if n is not None and n != mode.get((loc.get("page"), loc["table"], ax)):
                sig[i].add("table")
        rec = by_key.get(loc_key(loc))
        if rec is not None:
            keys = label_keys(rec)
            if keys and not any(k in vocab for k in keys):
                sig[i].add("novel")
    return sig


def loc_key(loc: dict) -> tuple:
    return tuple(loc.get(k) for k in ("page", "line", "table", "row", "col", "cell"))


def label_keys(rec: dict) -> list[str]:
    """What identifies how a value was printed: its label; for a table cell its row item and column header."""
    loc = rec.get("location") or {}
    if loc.get("layout") in ("table", "htable") and loc.get("header") is not None:
        return [norm_label(loc.get("header"))] if loc.get("header") else []
    lab = norm_label(rec.get("label"))
    return [lab] if lab else []


def norm_label(s: object) -> str:
    t = nfkc(s).casefold().replace(" ", "")
    return re.sub(r"[\d.:：()（）\[\]]+$", "", t)  # numbering / units in brackets / trailing colons


def build_vocab(meta: dict) -> set[str]:
    out: set[str] = set()
    for m in meta.values():
        if m["split"] in VOCAB:
            for rec in m["records"]:
                out.update(label_keys(rec))
    return out


# ------------------------------------------------------------------ scoring + conformal measurement
def score_rows(rows: list[dict], meta: dict, use: set[str]) -> list[tuple[dict, int, set[str]]]:
    table = json.loads(
        (Path(__file__).parents[3] / "crates/domain-manufacturing/src/calibration.json").read_text()
    )
    vocab = build_vocab(meta)
    by_doc: dict[str, list[int]] = defaultdict(list)
    for i, r in enumerate(rows):
        by_doc[r["doc"]].append(i)
    sig: list[set[str]] = [set() for _ in rows]
    for doc, idx in by_doc.items():
        for i, s in zip(idx, doc_signals([rows[i] for i in idx], meta.get(doc, {}), vocab), strict=True):
            sig[i] = s
    xdoc_signals(rows, sig)
    out: list[tuple[dict, int, set[str]]] = []
    for r, s in zip(rows, sig, strict=True):
        s = s & use
        p = jc.p_of(jc.steps_of(table["groups"], jc.group(r)), jc.conf(r))
        out.append((r, score(p, s), s))
    return out


def score(p: int, sig: set[str]) -> int:
    """Nonconformity score, lower = more suspicious. With only `forced` in use this is exactly MATH v2's
    score (-1 if forced, else p_correct). A value raising n >= 1 signal families scores -10 000 n; p_correct
    only orders values that raise none. p_correct is deliberately NOT a tie-breaker inside a signal class: the
    calibration wrong values are almost all forced (low-p OCR) values, so a p tie-break would teach the
    threshold "low p" - the very shift that failed on the holdout."""
    if sig == {"forced"}:
        return -1 if MATH_V2_COMPAT else -10_000
    return p if not sig else -10_000 * len(sig)


MATH_V2_COMPAT = False


XDOC = ("material", "drawing_number", "part_name")


def xdoc_signals(rows: list[dict], sig: list[set[str]]) -> None:
    """Same subject, same single-valued spec field, different value in two documents → flag every copy."""
    seen: dict[tuple, dict[str, set]] = defaultdict(lambda: defaultdict(set))
    for r in rows:
        lf = leaf(r.get("predicate") or "")
        if r.get("subject") and (lf in XDOC or lf.startswith(("dimension.", "tolerance."))):
            v = (nfkc(r.get("value_text")), r.get("value_num"), r.get("plus"), r.get("minus"))
            seen[(r["subject"], lf)][r["doc"]].add(v)
    for i, r in enumerate(rows):
        d = seen.get((r.get("subject"), leaf(r.get("predicate") or "")))
        if d and len(d) > 1 and len(set().union(*d.values())) > 1:
            sig[i].add("xdoc")


def measure(sc: list[tuple[dict, int, set[str]]]) -> dict:
    def pick(splits: tuple, ok: bool) -> list[int]:
        return [s for r, s, _ in sc if r["split"] in splits and r["ok"] == ok]

    t = jc.conformal_threshold(pick(CAL, False))

    def share(xs: list[int]) -> float | None:
        return round(sum(t is None or x <= t for x in xs) / len(xs), 4) if xs else None

    res: dict = {"threshold": t, "calibration_wrong": len(pick(CAL, False))}
    for name, splits in (("train", ("train",)), ("dev", CAL), ("holdout", HOLD)):
        w, c = pick(splits, False), pick(splits, True)
        res[name] = {
            "wrong": len(w),
            "correct": len(c),
            "coverage": share(w),
            "correct_flagged": share(c),
            "review_load": share(w + c),
        }
    return res


def per_family(sc: list[tuple[dict, int, set[str]]]) -> dict:
    out = {}
    for f in FAMILIES:
        row = {}
        for name, splits in (("train", ("train",)), ("dev", CAL), ("holdout", HOLD)):
            w = [s for r, _, s in sc if r["split"] in splits and not r["ok"]]
            c = [s for r, _, s in sc if r["split"] in splits and r["ok"]]
            row[name] = {"wrong_hit": sum(f in s for s in w), "correct_hit": sum(f in s for s in c)}
        out[f] = row
    return out


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("eval_json")
    ap.add_argument("--signals", default=",".join(FAMILIES))
    ap.add_argument("--no-holdout", action="store_true", help="drop holdout rows (design-time runs)")
    ap.add_argument("--dump", help="write per-value scores + signals here")
    a = ap.parse_args()
    ev = json.loads(Path(a.eval_json).read_text())
    rows = [r for r in ev["claims"] if not r["field"].startswith("pdf_info")]
    if a.no_holdout:
        rows = [r for r in rows if r["split"] not in HOLD]
    use = set(a.signals.split(","))
    global MATH_V2_COMPAT
    MATH_V2_COMPAT = True
    base = measure(score_rows(rows, ev["doc_meta"], {"forced"}))
    MATH_V2_COMPAT = False
    sc = score_rows(rows, ev["doc_meta"], use)
    res = {
        "signals": sorted(use),
        "before (forced + p_correct, MATH v2)": base,
        "after (gate v2)": measure(sc),
        "per_family": per_family(sc),
    }
    print(json.dumps(res, indent=1, ensure_ascii=False))
    if a.dump:
        Path(a.dump).write_text(
            json.dumps(
                [
                    {
                        k: r.get(k)
                        for k in (
                            "split",
                            "doc",
                            "field",
                            "ok",
                            "predicate",
                            "value_text",
                            "value_num",
                            "value_unit",
                            "subject",
                        )
                    }
                    | {"score": s, "signals": sorted(g)}
                    for r, s, g in sc
                ],
                ensure_ascii=False,
                indent=0,
            )
        )


if __name__ == "__main__":
    main()
