"""Positioned text lines. A line is a list of cells; a new cell starts where the horizontal gap between two
text
fragments is wider than about one character. Lines are numbered top-to-bottom per page, starting at 1."""

import re
import unicodedata
from dataclasses import dataclass, field

from .ja import is_cjk, join_cjk

SPLIT_CELLS = re.compile(r"\t+| {2,}")


@dataclass
class Cell:
    text: str
    x0: float
    x1: float
    conf: float | None = None  # OCR: lowest word confidence in the cell (0..1)


@dataclass
class Line:
    page: int
    no: int
    cells: list[Cell]
    y: float | None = None  # PDF user-space baseline (origin bottom-left); None for plain text
    confidence: float = 1.0
    size: float = 0.0  # font size (PDF points) when positioned
    extra: dict = field(default_factory=dict)

    @property
    def text(self) -> str:
        return " ".join(c.text for c in self.cells)

    def bbox(self) -> list[float] | None:
        if self.y is None:
            return None
        return [round(self.cells[0].x0, 1), round(self.y, 1), round(self.cells[-1].x1, 1), round(self.y, 1)]


# CJK fonts often map glyphs to Kangxi / CJK radical or compatibility code points in their ToUnicode table
# (U+2F0A "⼊" for 入, U+2FA6 "⾦" for 金). They look identical but never match a label or another document.
_COMPAT = re.compile("[\u2e80-\u2fdf\uf900-\ufaff]")


# CJK Radicals Supplement forms have no NFKC mapping; these are the ones fonts use for whole characters.
_RADICALS = dict(zip("⻑⻄⻘⻝⻤⻩⻭⻨⻫⻲⻯⻆⺠⻁", "長西青食鬼黄歯麦斉亀竜角民虎", strict=True))


def fold_cjk_compat(s: str) -> str:
    return _COMPAT.sub(lambda m: _RADICALS.get(m.group()) or unicodedata.normalize("NFKC", m.group()), s)


def lines_from_fragments(
    page: int, frags: list[tuple[float, float, float, str]], rules: list | None = None
) -> list[Line]:
    """frags: (x, y, font_size, text). Groups fragments into lines by baseline and into cells by gaps; a
    vertical ruling line between two fragments (`rules`: (x0, y0, x1, y1) segments, PDF points) always starts
    a new cell, however small the gap (tightly ruled tables)."""
    verts = [(r[0], r[1], r[3]) for r in rules or [] if r[0] == r[2]]

    def ruled(x0: float, x1: float, y: float, size: float) -> bool:
        return any(x0 <= vx <= x1 and lo - size <= y <= hi + size for vx, lo, hi in verts)

    frags = [(x, y, s, fold_cjk_compat(t)) for x, y, s, t in frags]
    frags = sorted(frags, key=lambda f: (-round(f[1], 0), f[0]))
    rows: list[list[tuple[float, float, float, str]]] = []
    for f in frags:
        if rows and abs(rows[-1][0][1] - f[1]) <= max(1.0, 0.4 * f[2]):
            rows[-1].append(f)
        else:
            rows.append([f])
    out: list[Line] = []
    for row in rows:
        row.sort(key=lambda f: f[0])
        cells: list[Cell] = []
        last, prev_x = "", 0.0
        for x, _y, size, text in row:
            # Full-width (CJK) glyphs advance about one em, Latin about half an em.
            width = sum(size if is_cjk(ch) else size * 0.5 for ch in text)
            gap = x - cells[-1].x1 if cells else 0.0
            if cells and verts and ruled(prev_x + 1.0, x - 0.5, _y, size):
                cells.append(Cell(text, x, x + width))
            # Per-glyph exports (single characters almost touching) are one word; Latin words keep spaces.
            elif cells and gap <= size * 0.3 and len(last) == 1 and len(text) == 1:
                cells[-1].text += text
                cells[-1].x1 = max(cells[-1].x1, x + width)
            elif cells and gap <= size * 1.0:
                cells[-1].text = join_cjk(cells[-1].text, text)
                cells[-1].x1 = max(cells[-1].x1, x + width)
            else:
                cells.append(Cell(text, x, x + width))
            last, prev_x = text, x
        out.append(Line(page, len(out) + 1, cells, y=row[0][1], size=max(f[2] for f in row)))
    return out


def adjacent(a: Line, b: Line) -> bool:
    """b directly follows a: next line number and, when positioned, no more than ~2 line heights below."""
    if b.page != a.page or b.no != a.no + 1:
        return False
    pa, pb = a.extra.get("px_bbox"), b.extra.get("px_bbox")
    if pa is not None and pb is not None:  # OCR lines: vertical gap at most ~one line height
        return pb[1] - pa[3] <= max(pa[3] - pa[1], pb[3] - pb[1], 1)
    if a.y is None or b.y is None:
        return True
    return abs(a.y - b.y) <= 2.0 * max(a.size, b.size, 1.0)


def lines_from_text(page: int, text: str, confidence: float = 1.0) -> list[Line]:
    """Plain text: cells are separated by tabs or two or more spaces; x is the character column."""
    out: list[Line] = []
    for no, raw in enumerate(text.splitlines(), start=1):
        cells = []
        pos = 0
        for part in SPLIT_CELLS.split(raw):
            start = raw.find(part, pos) if part else pos
            pos = start + len(part)
            if part.strip():
                cells.append(Cell(part.strip(), float(start), float(start + len(part))))
        if cells:
            out.append(Line(page, no, cells, confidence=confidence))
    return out
