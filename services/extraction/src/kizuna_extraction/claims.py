"""Claim extraction behind an interface. The only implementation is deterministic and local (a fixed label
vocabulary, mirroring crates/domain-manufacturing `mfg-map@3`). The sandbox has no model and no network. The
optional AI label mapping (ADR 0018) runs in the API process on this worker's output, never in here.

Document text never changes the vocabulary, the rules or the output schema; unknown labels are reported, not
turned into claims. Instruction-like text is flagged as a warning and otherwise treated as plain data.

Every claim keeps the printed `value`; normalised forms are added next to it, never instead of it: `norm`
(NFKC / enum), `date` (ISO, also from 和暦), `number` + `unit` (canonical: N/mm² → mpa, ㎜ → mm, 個 → pcs),
`plus`/`minus` (tolerances). OCR claims below OCR_REVIEW_FLOOR carry `needs_review`.
"""

import re
from typing import Protocol

from . import ja
from .errors import ExtractionError

FIELDS: dict[str, str] = {}
for _field, _labels in {
    "part_number": "part number|part no|part #|part|partno|p/n|pn|item number|item no|品番|部品番号"
    "|品目コード|部品コード",
    "part_name": "part name|品名|部品名",
    "drawing_number": "drawing number|drawing no|dwg no|dwg number|図番|図面番号",
    "revision": "revision|rev|rev no|drawing revision|spec revision|part revision|改訂|版|版数|改訂番号",
    "material": "material|material grade|steel grade|grade|材質|材料|鋼種",
    "supplier": "supplier|vendor|supplier name|manufacturer|maker|仕入先|製造者|発行者|発注先|メーカー",
    "approval_status": "approval status|approval|status",
    "approved_by": "approved by|approver|approved",
    "approval_date": "approval date|approved on|date approved|承認日",
    "expiry": "expiry|expiry date|expiration date|valid until|certificate expiry|有効期限",
    "po_number": "po number|po no|po|purchase order|purchase order number|order no|order number|注文番号"
    "|発注番号|注文no",
    "po_line": "line|po line|line no|行|行番号",
    "quantity": "quantity|qty|数量|員数",
    "unit_price": "unit price|単価",
    "lot": "lot|lot number|lot no|ロット|ロットno|ロット番号",
    "heat_number": "heat no|heat number|charge no|溶鋼番号|チャージno|チャージ番号|ヒートno|溶解番号",
    "standard": "standard|規格|規格番号",
    "result": "result|inspection result|judgement|判定|総合判定|結果|判定結果",
    "inspection_date": "inspection date|検査日|検査年月日",
    "issue_date": "issue date|date of issue|発行日|作成日",
    "delivery_date": "delivery date|due date|納期|納入日|納入期限",
    "surface_treatment": "surface treatment|表面処理",
    "tolerance": "tolerance|tol",
    "title": "title|drawing title",
    "scale": "scale|尺度",
    "sheet": "sheet",
    "drawn_by": "drawn|drawn by|設計|作図|製図",
    "checked_by": "checked|checked by|検図|照査",
    "fax.sender": "fax sender",
    "fax.number": "fax number",
    "fax.sent_at": "fax sent at",
    "dimension.length": "length",
    "dimension.width": "width",
    "dimension.height": "height",
    "dimension.thickness": "thickness",
    "dimension.diameter": "diameter",
    "dimension.outer_diameter": "outer diameter|od",
    "dimension.inner_diameter": "inner diameter",
    "property.hardness": "hardness",
    "property.tensile_strength": "tensile strength",
    "property.yield_strength": "yield strength",
    "property.surface_roughness": "surface roughness",
    "property.elongation": "elongation",
}.items():
    for _l in _labels.split("|"):
        FIELDS[_l] = _field

# Whole-token Japanese terms translated before lookup: `外径 測定値` reads as `outer diameter measured`,
# `外径 規格値` as `outer diameter nominal`. Mirrors JA_TOKENS in crates/domain-manufacturing/src/ja.rs.
JA_TOKENS = {
    "外径": "outer diameter",
    "内径": "inner diameter",
    "全長": "length",
    "幅": "width",
    "厚さ": "thickness",
    "高さ": "height",
    "直径": "diameter",
    "硬さ": "hardness",
    "硬度": "hardness",
    "引張強さ": "tensile strength",
    "引張強度": "tensile strength",
    "降伏点": "yield strength",
    "耐力": "yield strength",
    "降伏強さ": "yield strength",
    "伸び": "elongation",
    "表面粗さ": "surface roughness",
    "測定値": "measured",
    "実測値": "measured",
    "公差": "tolerance",
    "許容差": "tolerance",
    "規格値": "nominal",
    "基準値": "nominal",
    "基準寸法": "nominal",
    "図面寸法": "nominal",
    "公称値": "nominal",
    "長さ": "length",
    "板厚": "thickness",
    "長さ公差": "length tolerance",
    "幅公差": "width tolerance",
    "高さ公差": "height tolerance",
    "厚さ公差": "thickness tolerance",
    "板厚公差": "thickness tolerance",
    "外径公差": "outer diameter tolerance",
    "内径公差": "inner diameter tolerance",
    "直径公差": "diameter tolerance",
}
SEAL_LABEL = "印影"
# 承認: a name → approved_by + mark, a status word → approval_status, a date → approval_date
APPROVAL_LABEL = "承認"
STAMP_LABELS = {"設計", "作図", "製図", "検図", "照査", "承認"}  # paper approval boxes: seal or written name
ROLE_OF = {"drawn_by": "drawn", "checked_by": "checked", "approved_by": "approved"}
ELEMENTS = {"c", "si", "mn", "p", "s", "cu", "ni", "cr", "mo", "v", "nb", "ti", "al", "b", "n"}
DATE_FIELDS = {"approval_date", "expiry", "inspection_date", "issue_date", "delivery_date"}
TEXT_ENUMS = {"result": ja.result, "approval_status": ja.approval}
OCR_REVIEW_FLOOR = 0.80
UNIT_LABELS = {"単位", "unit", "uom"}  # a row's unit column applies to its quantity

INSTRUCTION_LIKE = re.compile(
    r"\b(ignore|disregard|override)\b.{0,40}\b(instructions?|rules?|polic(y|ies)|previous)\b"
    r"|^\s*(system|assistant)\s*[:：]|\byou are (now )?(an?|the) \w+"
    r"|(指示|命令|ルール|規則|設定).{0,12}(無視|従わ|変更し)",
    re.I,
)


def normalize_label(raw: str, translate: bool = True) -> tuple[str, str | None]:
    s = ja.nfkc(raw).lower()
    unit = None
    a, b = s.find("("), s.rfind(")")
    if 0 <= a < b:
        unit = s[a + 1 : b].strip() or None
        s = s[:a] + s[b + 1 :]
    s = "".join(c if c.isalnum() or c in "#/" else " " for c in s)
    toks = s.split()
    if translate:
        toks = [JA_TOKENS.get(t, t) for t in toks]
    return " ".join(" ".join(toks).split()), unit


def lookup(label: str) -> str | None:
    """Field for a normalised label: the fixed vocabulary plus `measured <dim>`, `<dim> measured`,
    `<dim> tolerance` and `<property> measured` forms. Element symbols are context-dependent (see extract)."""
    if label == APPROVAL_LABEL:
        return "approved_by"
    f = FIELDS.get(label)
    if f is not None:
        return f
    for prefix, suffix in (("", " nominal"), ("nominal ", "")):
        if label.startswith(prefix) and label.endswith(suffix) and label != f"{prefix}{suffix}".strip():
            rest = FIELDS.get(label[len(prefix) : len(label) - len(suffix)], "")
            if rest.startswith("dimension."):
                return rest
    for prefix, suffix in (("measured ", ""), ("", " measured"), ("actual ", "")):
        if label.startswith(prefix) and label.endswith(suffix) and label != f"{prefix}{suffix}".strip():
            rest = FIELDS.get(label[len(prefix) : len(label) - len(suffix)], "")
            if rest.startswith("dimension."):
                return "measured." + rest.removeprefix("dimension.")
            if rest.startswith("property."):
                return rest
    for prefix, suffix in (("", " tolerance"), ("tolerance ", ""), ("", " tol")):
        if label.startswith(prefix) and label.endswith(suffix) and label != f"{prefix}{suffix}".strip():
            if FIELDS.get(label[len(prefix) : len(label) - len(suffix)], "").startswith("dimension."):
                return "tolerance"
    return None


class ClaimExtractor(Protocol):
    name: str

    def extract(self, records: list[dict]) -> tuple[list[dict], list[str], list[str]]:
        """Returns (claims, unmapped_labels, warnings)."""
        ...


def _numeric_claim(claim: dict, value: str, unit: str | None, extra: list[dict]) -> None:
    field = claim["field"]
    q = ja.parse_quantity(value, unit)
    if q is None and field.startswith(("dimension.", "measured.")):
        split = ja.split_inline_tolerance(value)  # '120.5㎜ ±0.1': dimension plus its tolerance
        if split is not None:
            q = ja.parse_quantity(split[0], unit)
            t = ja.parse_tolerance(split[1])
            if q is not None and t is not None:
                tol = {**claim, "field": "tolerance", "value": split[1], "plus": t[0], "minus": t[1]}
                tol["unit"], tol["dimension"] = t[2] or q[1], field.split(".", 1)[1]
                tol["symmetric"] = t[0] == t[1]
                extra.append(tol)
    if q is not None:
        claim["number"], claim["unit"] = q
    elif field.startswith(("dimension.", "measured.")):
        claim["confidence"] = round(claim["confidence"] * 0.6, 3)  # numeric field with non-numeric value


class RuleBasedClaimExtractor:
    name = "rules@2"

    def extract(self, records: list[dict]) -> tuple[list[dict], list[str], list[str]]:
        claims: list[dict] = []
        unmapped: set[str] = set()
        flagged = 0
        labels = [normalize_label(r["label"]) for r in records]
        # element symbols (C, Si, Mn...) are chemistry only in a row with at least three of them (mill sheets)
        chem_groups: dict[str, int] = {}
        row_unit: dict[str, str] = {}
        for r, (lab, _u) in zip(records, labels, strict=True):
            if lab in ELEMENTS:
                chem_groups[r["group"]] = chem_groups.get(r["group"], 0) + 1
            elif lab in UNIT_LABELS and ja.unit_of(r["value"]):
                row_unit.setdefault(r["group"], r["value"])
        for r, (label, label_unit) in zip(records, labels, strict=True):
            if INSTRUCTION_LIKE.search(r["label"]) or INSTRUCTION_LIKE.search(r["value"]):
                flagged += 1
            field = lookup(label)
            if label == SEAL_LABEL:  # detected 印影 region: a mark is present (never "approved" by itself)
                role = ROLE_OF.get(lookup(normalize_label(r["value"])[0]) or "", "unassigned")
                field = f"stamp.{role}"
            if field is None and label in ELEMENTS and chem_groups.get(r["group"], 0) >= 3:
                field = f"property.chem_{label}"
            if field is None:
                if label and label not in UNIT_LABELS:
                    unmapped.add(label[:80])
                continue
            value = r["value"]
            claim = {
                "field": field,
                "value": value,
                "label": r["label"],
                "location": r["location"],
                "group": r["group"],
                "method": f"{r['method']}+{self.name}",
                "confidence": r["confidence"],
            }
            norm = ja.nfkc(value)
            if norm != value:
                claim["norm"] = norm  # full-width / ㎜ / − look-alikes: original kept in `value`
            extra: list[dict] = []
            if field.startswith("stamp."):
                claim |= {"norm": "present", "mark": "red_ink_region"}
            elif field in ROLE_OF:  # approval box: name (a stamp/sign), status word or date
                iso = ja.parse_date(value)
                status = ja.approval(value) if label == APPROVAL_LABEL else None
                if iso is not None:
                    if label != APPROVAL_LABEL and field != "approved_by":
                        continue  # date in a 設計/検図 box: not a person, no claim
                    claim |= {"field": "approval_date", "date": iso}
                elif status is not None:
                    claim |= {"field": "approval_status", "norm": status}
                elif label in STAMP_LABELS:
                    mark = {**claim, "field": f"stamp.{ROLE_OF[field]}", "norm": "present"}
                    mark["mark"] = "name_in_approval_box"
                    extra.append(mark)
            elif field in DATE_FIELDS or field == "fax.sent_at":
                iso = value if field == "fax.sent_at" else ja.parse_date(value)
                if iso is not None:
                    claim["date"] = iso
            elif field in TEXT_ENUMS:
                n = TEXT_ENUMS[field](value)
                if n is not None:
                    claim["norm"] = n
            elif field == "tolerance":
                t = ja.parse_tolerance(value)
                if t is not None:
                    claim |= {"plus": t[0], "minus": t[1], "symmetric": t[0] == t[1]}
                    claim["number"], claim["unit"] = t[0], t[2] or ja.unit_of(label_unit) or label_unit
                    dim = label.removesuffix(" tolerance").removeprefix("tolerance ").removesuffix(" tol")
                    if FIELDS.get(dim, "").startswith("dimension."):
                        claim["dimension"] = FIELDS[dim].removeprefix("dimension.")
            elif field.startswith(("dimension.", "measured.", "property.")) or field in (
                "quantity",
                "unit_price",
            ):
                unit = label_unit or (row_unit.get(r["group"]) if field == "quantity" else None)
                _numeric_claim(claim, value, unit, extra)
            for c in (claim, *extra):
                if r["method"].startswith("ocr") and r["confidence"] < OCR_REVIEW_FLOOR:
                    c["needs_review"] = True  # must be confirmed by a person before it can back a decision
                claims.append(c)
        warnings = [f"instruction_like_text_treated_as_data:{flagged}"] if flagged else []
        return claims, sorted(unmapped), warnings


ALLOWED: dict[str, type] = {"rules": RuleBasedClaimExtractor}


def get_claim_extractor(name: str) -> ClaimExtractor:
    cls = ALLOWED.get(name)
    if cls is None:
        raise ExtractionError("claim_extractor_not_allowed")
    return cls()


# Endings of label words (製品番号, 図面No., 使用材料, 納入業者): a title-block or CSV header cell with
# one of these reads as a label even when the vocabulary does not know it; the Rust mapper then maps it with
# a fallback (mfg-map@5).
LABEL_ENDINGS = (
    "番号", "no", "名", "日", "材料", "材質", "記号", "規格", "業者", "処理", "仕上げ", "スケール", "尺度",
    "縮尺", "期日", "ロット", "コード", "型式", "型番",
)  # fmt: skip


def label_like(text: str) -> bool:
    """In the vocabulary, or a short word with a label ending."""
    if lookup(normalize_label(text)[0]) is not None:
        return True
    t = normalize_label(text, translate=False)[0]
    return 0 < len(t) <= 8 and not any(c.isdigit() for c in t) and t.endswith(LABEL_ENDINGS)
