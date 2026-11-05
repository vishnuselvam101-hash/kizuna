"""Japanese PDFs as office tools and browsers print them: one positioned fragment per glyph, full-width glyphs
about one em wide, and CJK radical code points in the font's ToUnicode map (U+2F0A for 入, U+2FA6 for 金)."""

from kizuna_extraction.layout import lines_from_fragments
from kizuna_extraction.records import Sink, records_from_lines


def glyphs(x: float, y: float, text: str, size: float = 10.5) -> list[tuple[float, float, float, str]]:
    out = []
    for ch in text:
        out.append((x, y, size, ch))
        x += size if ord(ch) > 0x2E7F else size * 0.55
    return out


def test_per_glyph_cjk_fragments_join_without_spaces():
    frags = (
        glyphs(30, 700, "引張強さ (MPa)：580")
        + glyphs(30, 680, "仕⼊先：北陸⾦属⼯業")
        + glyphs(30, 660, "⻑さ (mm)：120")
    )
    lines = lines_from_fragments(1, frags)
    texts = [ln.text for ln in lines]
    assert texts[0].startswith("引張強さ"), texts
    assert len(lines[0].cells) == 1, [c.text for c in lines[0].cells]
    assert texts[1].replace(" ", "") == "仕入先：北陸金属工業", texts
    assert "仕入先" in texts[1] and "北陸金属工業" in texts[1], texts
    assert texts[2].startswith("長さ"), texts


def test_japanese_labels_from_glyph_fragments_become_records():
    frags = (
        glyphs(30, 700, "品番：JA-BRK-1001")
        + glyphs(30, 680, "材質：SUS316")
        + glyphs(30, 660, "有効期限：2031-03-31")
    )
    sink = Sink(100)
    records_from_lines(lines_from_fragments(1, frags), "pdf-text", sink)
    labels = {r["label"]: r["value"] for r in sink.items}
    assert labels.get("品番") == "JA-BRK-1001", labels
    assert labels.get("材質") == "SUS316", labels
    assert labels.get("有効期限") == "2031-03-31", labels


def test_latin_words_keep_their_spaces():
    lines = lines_from_fragments(
        1, [(72, 700, 10, "Part"), (95, 700, 10, "Number:"), (140, 700, 10, "BRK-100")]
    )
    assert lines[0].text == "Part Number: BRK-100"
