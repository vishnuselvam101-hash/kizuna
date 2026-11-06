"""PDF text with positions (pypdf), bounded against hostile files."""

import io

from .errors import ExtractionError
from .layout import Line, lines_from_fragments, lines_from_text

META_KEYS = (
    "/Title",
    "/Subject",
    "/Author",
    "/Keywords",
    "/Creator",
    "/Producer",
    "/CreationDate",
    "/ModDate",
)


def _caps(limit: int) -> dict[str, int]:
    """pypdf decompression/work ceilings (defence against flate/LZW bombs and pathological content
    streams)."""
    names = (
        "maximum_declared_stream_length",
        "array_based_stream_maximum_output_length",
        "jbig2_maximum_output_length",
        "lzw_maximum_output_length",
        "run_length_maximum_output_length",
        "zlib_maximum_output_length",
        "brotli_maximum_output_length",
        "image_maximum_buffer_size",
    )
    return {n: limit for n in names}


MAX_RULES_PER_PAGE = 4000
PAINT = {b"S", b"s", b"B", b"B*", b"b", b"b*", b"f", b"F", b"f*"}


class PageGraphics:
    """What one page draws besides text, read from the content stream while the text is extracted: ruling
    lines (horizontal/vertical stroked segments and thin filled rectangles, PDF points) for table structure,
    whether it paints an image, and the text render mode of each text fragment (3 = invisible, the hidden
    text layer scanners put over the page image)."""

    def __init__(self) -> None:
        self.rules: list[tuple[float, float, float, float]] = []
        self.image = False
        self.mode = 0
        self.stack: list[int] = []
        self.path: list[tuple[float, float, float, float]] = []
        self.cur: tuple[float, float] | None = None

    def operand(self, op: bytes, args: list, cm: list, _tm: list) -> None:
        def pt(x: float, y: float) -> tuple[float, float]:
            return cm[0] * x + cm[2] * y + cm[4], cm[1] * x + cm[3] * y + cm[5]

        try:
            nums = [float(a) for a in args] if op in (b"m", b"l", b"re", b"Tr") else []
        except (TypeError, ValueError):
            return
        if op == b"q":
            self.stack.append(self.mode)
        elif op == b"Q":
            self.mode = self.stack.pop() if self.stack else 0
        elif op == b"Tr" and nums:
            self.mode = int(nums[0])
        elif op == b"Do":
            self.image = True  # an XObject (image or form) is painted
        elif op == b"m" and len(nums) == 2:
            self.cur = pt(*nums)
        elif op == b"l" and len(nums) == 2 and self.cur is not None:
            nxt = pt(*nums)
            self.path.append((*self.cur, *nxt))
            self.cur = nxt
        elif op == b"re" and len(nums) == 4:
            x, y, w, h = nums
            a, b, c, d = pt(x, y), pt(x + w, y), pt(x + w, y + h), pt(x, y + h)
            if abs(h) < 2.0 or abs(w) < 2.0:  # a thin filled or stroked bar is one rule
                self.path.append((*a, *c))
            else:
                self.path += [(*a, *b), (*b, *c), (*c, *d), (*d, *a)]
        elif op in PAINT:
            for x0, y0, x1, y1 in self.path:
                if len(self.rules) >= MAX_RULES_PER_PAGE:
                    break
                if abs(y1 - y0) < 2.0 and abs(x1 - x0) >= 4.0:
                    self.rules.append((min(x0, x1), (y0 + y1) / 2, max(x0, x1), (y0 + y1) / 2))
                elif abs(x1 - x0) < 2.0 and abs(y1 - y0) >= 4.0:
                    self.rules.append(((x0 + x1) / 2, min(y0, y1), (x0 + x1) / 2, max(y0, y1)))
            self.path, self.cur = [], None
        elif op == b"n":
            self.path, self.cur = [], None


def pdf_extract(data: bytes, limits: dict) -> tuple[list[Line], dict[str, str], list[str]]:
    """Returns (lines, metadata, warnings). Lines are positioned when pypdf reports text matrices. Positioned
    lines carry `extra["rules"]` (the page's ruling lines, shared per page), `extra["page_h"]` and, on pages
    whose only text is invisible over an image, `extra["hidden_text"] = True`."""
    from pypdf import PdfReader, apply_configuration

    with apply_configuration(**_caps(limits["max_stream_bytes"])):
        return _extract(PdfReader, data, limits)


def _extract(PdfReader, data: bytes, limits: dict) -> tuple[list[Line], dict[str, str], list[str]]:  # noqa: N803
    from pypdf.errors import LimitReachedError, PyPdfError

    warnings: list[str] = []
    try:
        reader = PdfReader(io.BytesIO(data), strict=False)
        if reader.is_encrypted:
            raise ExtractionError("pdf_encrypted")
        meta: dict[str, str] = {}
        info = reader.metadata or {}
        for k in META_KEYS:
            v = info.get(k)
            if v is not None and str(v).strip():
                meta[k] = str(v).strip()[:500]
        n_pages = len(reader.pages)
        if n_pages > limits["max_pages"]:
            warnings.append(f"pdf_pages_truncated:{n_pages}>{limits['max_pages']}")
        lines: list[Line] = []
        total = 0
        for i in range(min(n_pages, limits["max_pages"])):
            page = reader.pages[i]
            frags: list[tuple[float, float, float, str]] = []
            g = PageGraphics()
            modes: list[int] = []

            def visit(text, cm, tm, _fd, size, frags=frags, g=g, modes=modes):
                t = text.strip()
                if not t or len(frags) >= limits["max_fragments_per_page"]:
                    return
                x = cm[0] * tm[4] + cm[2] * tm[5] + cm[4]
                y = cm[1] * tm[4] + cm[3] * tm[5] + cm[5]
                scale = abs(cm[3] * tm[3]) or 1.0
                frags.append((x, y, max(1.0, float(size) * scale), t))
                modes.append(g.mode)

            text = page.extract_text(visitor_text=visit, visitor_operand_before=g.operand) or ""
            if len(text) > limits["max_page_chars"]:
                raise ExtractionError("pdf_page_text_too_large")
            total += len(text)
            if total > limits["max_text_chars"]:
                raise ExtractionError("pdf_text_too_large")
            if len(frags) >= limits["max_fragments_per_page"]:
                warnings.append(f"pdf_page_fragments_truncated:{i + 1}")
            # Positioned lines when every fragment had usable coordinates, else plain text lines.
            plines = lines_from_fragments(i + 1, frags, g.rules) if frags else lines_from_text(i + 1, text)
            try:
                page_h = float(page.mediabox.top)
            except (TypeError, ValueError, AttributeError):
                page_h = 842.0
            hidden = bool(modes) and g.image and all(m == 3 for m in modes)
            for ln in plines:
                ln.extra["rules"] = g.rules
                ln.extra["page_h"] = page_h
                if hidden:
                    ln.extra["hidden_text"] = True
            lines.extend(plines)
        return lines, meta, warnings
    except LimitReachedError as e:
        raise ExtractionError("pdf_resource_limit") from e
    except PyPdfError as e:
        raise ExtractionError("pdf_unreadable") from e
    except RecursionError as e:
        raise ExtractionError("pdf_too_deeply_nested") from e
    except MemoryError as e:
        raise ExtractionError("pdf_resource_limit") from e
