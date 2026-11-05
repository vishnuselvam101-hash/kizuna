"""Positioned text lines. A line is a list of cells; a new cell starts where the horizontal gap between two
text
fragments is wider than about one character. Lines are numbered top-to-bottom per page, starting at 1."""

import re
from dataclasses import dataclass, field

SPLIT_CELLS = re.compile(r"\t+| {2,}")


@dataclass
class Cell:
    text: str
    x0: float
    x1: float


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


def lines_from_fragments(page: int, frags: list[tuple[float, float, float, str]]) -> list[Line]:
    """frags: (x, y, font_size, text). Groups fragments into lines by baseline and into cells by gaps."""
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
        for x, _y, size, text in row:
            width = len(text) * size * 0.5
            if cells and x - cells[-1].x1 <= size * 1.0:
                cells[-1].text += " " + text
                cells[-1].x1 = max(cells[-1].x1, x + width)
            else:
                cells.append(Cell(text, x, x + width))
        out.append(Line(page, len(out) + 1, cells, y=row[0][1], size=max(f[2] for f in row)))
    return out


def adjacent(a: Line, b: Line) -> bool:
    """b directly follows a: next line number and, when positioned, no more than ~2 line heights below."""
    if b.page != a.page or b.no != a.no + 1:
        return False
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
