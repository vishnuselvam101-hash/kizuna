"""OCR behind a small interface. The only engine is the tesseract CLI, run as a subprocess with a fixed argv,
no shell, a timeout and the worker's inherited rlimits. Word boxes and confidences come from tesseract's
TSV."""

import shutil
import struct
import subprocess  # noqa: S404  (fixed argv, no shell, OCR engine only)

from .errors import ExtractionError
from .layout import Cell, Line


def image_pixels(data: bytes) -> int | None:
    """Declared pixel count for PNG/JPEG headers (decompression-bomb guard); None if unknown."""
    if data[:8] == b"\x89PNG\r\n\x1a\n" and len(data) >= 24:
        w, h = struct.unpack(">II", data[16:24])
        return w * h
    if data[:2] == b"\xff\xd8":
        i = 2
        while i + 9 < len(data):
            if data[i] != 0xFF:
                return None
            marker = data[i + 1]
            seg = struct.unpack(">H", data[i + 2 : i + 4])[0]
            if marker in (0xC0, 0xC1, 0xC2):
                h, w = struct.unpack(">HH", data[i + 5 : i + 9])
                return w * h
            i += 2 + seg
    return None


class OcrEngine:
    name = "none"

    def available(self) -> bool:
        return False

    def recognize(self, data: bytes, page: int = 1) -> list[Line]:
        raise ExtractionError("ocr_unavailable")


class TesseractOcr(OcrEngine):
    name = "tesseract"

    def __init__(self, langs: str = "eng+jpn", timeout: int = 60):
        self.langs = langs
        self.timeout = timeout

    def available(self) -> bool:
        return shutil.which("tesseract") is not None

    def recognize(self, data: bytes, page: int = 1) -> list[Line]:
        exe = shutil.which("tesseract")
        if exe is None:
            raise ExtractionError("ocr_unavailable")
        try:
            proc = subprocess.run(  # noqa: S603
                [exe, "stdin", "stdout", "-l", self.langs, "--psm", "6", "tsv"],
                input=data,
                capture_output=True,
                timeout=self.timeout,
                check=False,
                env={"PATH": "/usr/bin", "OMP_THREAD_LIMIT": "1"},
            )
        except subprocess.TimeoutExpired as e:
            raise ExtractionError("ocr_timeout") from e
        if proc.returncode != 0:
            raise ExtractionError("ocr_failed")
        return parse_tsv(proc.stdout.decode("utf-8", "replace"), page)


def parse_tsv(tsv: str, page: int) -> list[Line]:
    """Groups TSV words (level 5) into lines; cells split on gaps wider than ~1.5 word heights."""
    groups: dict[tuple[int, int, int], list[tuple[int, int, int, int, float, str]]] = {}
    for row in tsv.splitlines()[1:]:
        f = row.split("\t")
        if len(f) < 12 or f[0] != "5" or not f[11].strip():
            continue
        try:
            left, top, width, height = (int(v) for v in f[6:10])
            conf = float(f[10])
        except ValueError:
            continue
        groups.setdefault((int(f[2]), int(f[3]), int(f[4])), []).append(
            (left, top, width, height, conf, f[11].strip())
        )
    lines: list[Line] = []
    for key in sorted(groups, key=lambda k: min(w[1] for w in groups[k])):
        words = sorted(groups[key])
        cells: list[Cell] = []
        for left, _top, width, height, _c, text in words:
            if cells and left - cells[-1].x1 <= 1.5 * height:
                cells[-1].text += " " + text
                cells[-1].x1 = float(left + width)
            else:
                cells.append(Cell(text, float(left), float(left + width)))
        conf = sum(max(w[4], 0.0) for w in words) / len(words) / 100.0
        top = min(w[1] for w in words)
        bottom = max(w[1] + w[3] for w in words)
        ln = Line(page, len(lines) + 1, cells, confidence=round(conf, 3))
        ln.extra["px_bbox"] = [int(cells[0].x0), top, int(cells[-1].x1), bottom]
        lines.append(ln)
    return lines


def rasterize_pdf(data: bytes, max_pages: int, dpi: int = 200, timeout: int = 60) -> list[bytes]:
    """Renders pages of a PDF without a text layer to PNG with pdftoppm (stdin to stdout, no files)."""
    exe = shutil.which("pdftoppm")
    if exe is None:
        raise ExtractionError("rasterizer_unavailable")
    pages: list[bytes] = []
    for p in range(1, max_pages + 1):
        try:
            proc = subprocess.run(  # noqa: S603
                [exe, "-png", "-r", str(dpi), "-f", str(p), "-l", str(p), "-singlefile", "-"],
                input=data,
                capture_output=True,
                timeout=timeout,
                check=False,
                env={"PATH": "/usr/bin"},
            )
        except subprocess.TimeoutExpired as e:
            raise ExtractionError("rasterize_timeout") from e
        if proc.returncode != 0 or not proc.stdout:
            break  # past the last page (or unreadable)
        pages.append(proc.stdout)
    return pages
