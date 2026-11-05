"""OCR behind a small interface. The only engine is the tesseract CLI, run as a subprocess with a fixed argv,
no shell, a timeout and the worker's inherited rlimits. Word boxes and confidences come from tesseract's
TSV.

Scans are prepared for OCR with ImageMagick 7 when it is installed (`preprocess`): pages are rendered at
300 dpi (images are upsampled to about 300 dpi for an A4 page), detected 印影 regions are painted white, then
the page is converted to grey, deskewed and binarised (Otsu). Without ImageMagick the raw raster is used and
the worker warns. Word boxes are always reported in the 200-dpi base pixel space the seal detector uses."""

import shutil
import struct
import subprocess  # noqa: S404  (fixed argv, no shell, OCR engine only)

from .errors import ExtractionError
from .ja import join_cjk
from .layout import Cell, Line

OCR_DPI = 300  # raster resolution for OCR
BASE_DPI = 200  # coordinate space of OCR word boxes and seal regions (stamps.SCALE)
A4_LONG_300 = 3508  # px on the long side of an A4 page at 300 dpi
# ImageMagick 7 binaries, real files first: inside the sandbox /etc/alternatives is not mounted, so the
# `magick` symlink may dangle (shutil.which then skips it).
MAGICK = ("magick-im7.q16", "magick-im7.q16hdri", "magick")


def image_pixels(data: bytes) -> int | None:
    """Declared pixel count for PNG/JPEG headers (decompression-bomb guard); None if unknown."""
    size = image_size(data)
    return size[0] * size[1] if size else None


def image_size(data: bytes) -> tuple[int, int] | None:
    """Declared (width, height) of a PNG/JPEG; None if unknown."""
    if data[:8] == b"\x89PNG\r\n\x1a\n" and len(data) >= 24:
        w, h = struct.unpack(">II", data[16:24])
        return w, h
    if data[:2] == b"\xff\xd8":
        i = 2
        while i + 9 < len(data):
            if data[i] != 0xFF:
                return None
            marker = data[i + 1]
            seg = struct.unpack(">H", data[i + 2 : i + 4])[0]
            if marker in (0xC0, 0xC1, 0xC2):
                h, w = struct.unpack(">HH", data[i + 5 : i + 9])
                return w, h
            i += 2 + seg
    return None


class OcrEngine:
    name = "none"

    def available(self) -> bool:
        return False

    def recognize(self, data: bytes, page: int = 1, scale: float = 1.0) -> list[Line]:
        raise ExtractionError("ocr_unavailable")


class TesseractOcr(OcrEngine):
    name = "tesseract"

    def __init__(self, langs: str = "eng+jpn", timeout: int = 60):
        self.langs = langs
        self.timeout = timeout

    def available(self) -> bool:
        return shutil.which("tesseract") is not None

    def recognize(self, data: bytes, page: int = 1, scale: float = 1.0) -> list[Line]:
        """`scale`: raster pixels per base pixel; word boxes are divided by it."""
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
        return parse_tsv(proc.stdout.decode("utf-8", "replace"), page, scale)


def parse_tsv(tsv: str, page: int, scale: float = 1.0) -> list[Line]:
    """Groups TSV words (level 5) into lines; cells split on gaps wider than ~1.5 word heights."""
    groups: dict[tuple[int, int, int], list[tuple[int, int, int, int, float, str]]] = {}
    for row in tsv.splitlines()[1:]:
        f = row.split("\t")
        if len(f) < 12 or f[0] != "5" or not f[11].strip():
            continue
        try:
            left, top, width, height = (round(int(v) / scale) for v in f[6:10])
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
        for left, _top, width, height, wc, text in words:
            if cells and left - cells[-1].x1 <= 1.5 * height:
                # tesseract jpn emits one "word" per kanji/kana: rejoin without spaces at CJK boundaries
                cells[-1].text = join_cjk(cells[-1].text, text)
                cells[-1].x1 = float(left + width)
                prev = 1.0 if cells[-1].conf is None else cells[-1].conf
                cells[-1].conf = min(prev, max(wc, 0.0) / 100.0)
            else:
                cells.append(Cell(text, float(left), float(left + width), max(wc, 0.0) / 100.0))
        conf = sum(max(w[4], 0.0) for w in words) / len(words) / 100.0
        top = min(w[1] for w in words)
        bottom = max(w[1] + w[3] for w in words)
        ln = Line(page, len(lines) + 1, cells, confidence=round(conf, 3))
        ln.extra["px_bbox"] = [int(cells[0].x0), top, int(cells[-1].x1), bottom]
        lines.append(ln)
    return lines


def magick_exe() -> str | None:
    return next((e for e in map(shutil.which, MAGICK) if e), None)


def upsample_factor(data: bytes) -> float:
    """Image scans: factor that brings an A4-sized page to about 300 dpi (1.0-2.0; 1.0 when unknown)."""
    size = image_size(data)
    if not size:
        return 1.0
    return round(min(2.0, max(1.0, A4_LONG_300 / max(size))), 3)


def preprocess(
    data: bytes,
    fmt: str,
    factor: float = 1.0,
    masks: list[list[int]] | tuple = (),
    *,
    deskew: bool = True,
    binarize: bool = True,
    timeout: int = 60,
) -> bytes | None:
    """ImageMagick: paint `masks` (input pixels, [x0, y0, x1, y1]) white, resize by `factor`, grey, deskew
    (canvas kept: rotated about the centre), Otsu binarisation → PNG. The input coder is fixed
    (`png:`/`jpeg:`), so no other ImageMagick coder ever parses the upload. None when ImageMagick is missing
    or fails."""
    exe = magick_exe()
    if exe is None or fmt not in ("png", "jpeg"):
        return None
    argv = [exe, "-limit", "memory", "512MiB", "-limit", "map", "1GiB", "-limit", "disk", "0"]
    argv += ["-limit", "time", str(timeout), f"{fmt}:-", "-alpha", "off"]
    for x0, y0, x1, y1 in masks:
        argv += ["-fill", "white", "-stroke", "none", "-draw", f"rectangle {x0},{y0} {x1},{y1}"]
    if factor != 1.0:
        argv += ["-resize", f"{factor * 100:.1f}%"]
    argv += ["-colorspace", "Gray"]
    size = image_size(data)
    if deskew and size:
        w, h = round(size[0] * factor), round(size[1] * factor)
        argv += ["-background", "white", "-deskew", "40%", "+repage"]
        argv += ["-gravity", "center", "-extent", f"{w}x{h}"]
    if binarize:
        argv += ["-auto-threshold", "OTSU"]
    argv += ["-strip", "png:-"]
    try:
        proc = subprocess.run(  # noqa: S603
            argv, input=data, capture_output=True, timeout=timeout + 5, check=False, env={"PATH": "/usr/bin"}
        )
    except subprocess.TimeoutExpired:
        return None
    return proc.stdout if proc.returncode == 0 and proc.stdout[:4] == b"\x89PNG" else None


def page_dpi(long_side_in: float) -> int:
    """OCR resolution for a page: 300 dpi for A4/Letter-sized pages; pages larger than A4 (e.g. a 72-dpi image
    wrapped as a 19-inch page) are rendered to about the pixel size of an A4 page at 300 dpi, never below
    200 dpi, so oversized text is not upsampled further."""
    if long_side_in <= 0:
        return OCR_DPI
    return max(BASE_DPI, min(OCR_DPI, round(A4_LONG_300 / long_side_in)))


def rasterize_page(data: bytes, page: int, dpi: int = OCR_DPI, timeout: int = 60) -> bytes | None:
    """Renders one page of a PDF without a text layer to PNG with pdftoppm (stdin to stdout, no files);
    None past the last page (or when unreadable)."""
    exe = shutil.which("pdftoppm")
    if exe is None:
        raise ExtractionError("rasterizer_unavailable")
    try:
        proc = subprocess.run(  # noqa: S603
            [exe, "-png", "-r", str(dpi), "-f", str(page), "-l", str(page), "-singlefile", "-"],
            input=data,
            capture_output=True,
            timeout=timeout,
            check=False,
            env={"PATH": "/usr/bin"},
        )
    except subprocess.TimeoutExpired as e:
        raise ExtractionError("rasterize_timeout") from e
    return proc.stdout if proc.returncode == 0 and proc.stdout else None
