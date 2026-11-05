"""Claim extraction behind an interface. The only implementation is deterministic and local (a fixed label
vocabulary, mirroring crates/domain-manufacturing `mfg-map@1`). The sandbox has no model and no network. The
optional AI label mapping (ADR 0018) runs in the API process on this worker's output, never in here.

Document text never changes the vocabulary, the rules or the output schema; unknown labels are reported, not
turned into claims. Instruction-like text is flagged as a warning and otherwise treated as plain data.
"""

import re
import unicodedata
from typing import Protocol

from .errors import ExtractionError

FIELDS: dict[str, str] = {}
for _field, _labels in {
    "part_number": "part number|part no|part #|part|partno|p/n|pn|item number|item no|品番|部品番号",
    "drawing_number": "drawing number|drawing no|dwg no|dwg number|図番",
    "revision": "revision|rev|rev no|drawing revision|spec revision|part revision|改訂|版",
    "material": "material|material grade|材質",
    "supplier": "supplier|vendor|supplier name|manufacturer|仕入先",
    "approval_status": "approval status|approval|status|承認",
    "approved_by": "approved by|approver|approved",
    "approval_date": "approval date|approved on|date approved",
    "expiry": "expiry|expiry date|expiration date|valid until|certificate expiry|有効期限",
    "po_number": "po number|po no|po|purchase order|purchase order number|注文番号",
    "po_line": "line|po line|line no",
    "quantity": "quantity|qty|数量",
    "lot": "lot|lot number|lot no|ロット",
    "result": "result|inspection result|judgement|判定",
    "tolerance": "tolerance|tol",
    "title": "title|drawing title",
    "scale": "scale|尺度",
    "sheet": "sheet",
    "drawn_by": "drawn|drawn by",
    "checked_by": "checked|checked by",
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
}.items():
    for _l in _labels.split("|"):
        FIELDS[_l] = _field

NUM_UNIT = re.compile(
    r"^\s*([-+]?\d+(?:[.,]\d+)?)\s*(mm|cm|m|in|um|µm|kg|g|n|mpa|gpa|hrc|hv|ra|pcs|%)?\s*$", re.I
)
PLUS_MINUS = re.compile(r"^\s*(?:±|\+/-)\s*(\d+(?:\.\d+)?)\s*(\w+)?\s*$")
INSTRUCTION_LIKE = re.compile(
    r"\b(ignore|disregard|override)\b.{0,40}\b(instructions?|rules?|polic(y|ies)|previous)\b"
    r"|^\s*(system|assistant)\s*[:：]|\byou are (now )?(an?|the) \w+",
    re.I,
)


def normalize_label(raw: str) -> tuple[str, str | None]:
    s = unicodedata.normalize("NFKC", raw).lower()
    unit = None
    a, b = s.find("("), s.rfind(")")
    if 0 <= a < b:
        unit = s[a + 1 : b].strip() or None
        s = s[:a] + s[b + 1 :]
    s = "".join(c if c.isalnum() or c in "#/" else " " for c in s)
    return " ".join(s.split()), unit


def _measured(label: str) -> str | None:
    for prefix, suffix in (("measured ", ""), ("", " measured"), ("actual ", "")):
        if label.startswith(prefix) and label.endswith(suffix) and label != f"{prefix}{suffix}".strip():
            rest = label[len(prefix) : len(label) - len(suffix)]
            f = FIELDS.get(rest, "")
            if f.startswith("dimension."):
                return "measured." + f.removeprefix("dimension.")
    return None


class ClaimExtractor(Protocol):
    name: str

    def extract(self, records: list[dict]) -> tuple[list[dict], list[str], list[str]]:
        """Returns (claims, unmapped_labels, warnings)."""
        ...


class RuleBasedClaimExtractor:
    name = "rules@1"

    def extract(self, records: list[dict]) -> tuple[list[dict], list[str], list[str]]:
        claims: list[dict] = []
        unmapped: set[str] = set()
        flagged = 0
        for r in records:
            if INSTRUCTION_LIKE.search(r["label"]) or INSTRUCTION_LIKE.search(r["value"]):
                flagged += 1
            label, label_unit = normalize_label(r["label"])
            field = FIELDS.get(label) or _measured(label)
            if field is None:
                if label:
                    unmapped.add(label[:80])
                continue
            claim = {
                "field": field,
                "value": r["value"],
                "label": r["label"],
                "location": r["location"],
                "group": r["group"],
                "method": f"{r['method']}+{self.name}",
                "confidence": r["confidence"],
            }
            m = NUM_UNIT.match(r["value"])
            pm = PLUS_MINUS.match(r["value"])
            if m and (field.startswith(("dimension.", "measured.", "property.")) or field == "quantity"):
                claim["number"] = m.group(1).replace(",", ".")
                claim["unit"] = (m.group(2) or label_unit or "").lower() or None
            elif pm and field == "tolerance":
                claim["number"] = pm.group(1)
                claim["unit"] = (pm.group(2) or label_unit or "").lower() or None
                claim["symmetric"] = True
            elif field.startswith(("dimension.", "measured.")):
                claim["confidence"] = round(
                    claim["confidence"] * 0.6, 3
                )  # numeric field with non-numeric value
            claims.append(claim)
        warnings = [f"instruction_like_text_treated_as_data:{flagged}"] if flagged else []
        return claims, sorted(unmapped), warnings


ALLOWED: dict[str, type] = {"rules": RuleBasedClaimExtractor}


def get_claim_extractor(name: str) -> ClaimExtractor:
    cls = ALLOWED.get(name)
    if cls is None:
        raise ExtractionError("claim_extractor_not_allowed")
    return cls()
