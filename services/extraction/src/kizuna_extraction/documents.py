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


def pdf_extract(data: bytes, limits: dict) -> tuple[list[Line], dict[str, str], list[str]]:
    """Returns (lines, metadata, warnings). Lines are positioned when pypdf reports text matrices."""
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

            def visit(text, cm, tm, _fd, size, frags=frags):
                t = text.strip()
                if not t or len(frags) >= limits["max_fragments_per_page"]:
                    return
                x = cm[0] * tm[4] + cm[2] * tm[5] + cm[4]
                y = cm[1] * tm[4] + cm[3] * tm[5] + cm[5]
                scale = abs(cm[3] * tm[3]) or 1.0
                frags.append((x, y, max(1.0, float(size) * scale), t))

            text = page.extract_text(visitor_text=visit) or ""
            if len(text) > limits["max_page_chars"]:
                raise ExtractionError("pdf_page_text_too_large")
            total += len(text)
            if total > limits["max_text_chars"]:
                raise ExtractionError("pdf_text_too_large")
            if len(frags) >= limits["max_fragments_per_page"]:
                warnings.append(f"pdf_page_fragments_truncated:{i + 1}")
            # Positioned lines when every fragment had usable coordinates, else plain text lines.
            lines.extend(lines_from_fragments(i + 1, frags) if frags else lines_from_text(i + 1, text))
        return lines, meta, warnings
    except LimitReachedError as e:
        raise ExtractionError("pdf_resource_limit") from e
    except PyPdfError as e:
        raise ExtractionError("pdf_unreadable") from e
    except RecursionError as e:
        raise ExtractionError("pdf_too_deeply_nested") from e
    except MemoryError as e:
        raise ExtractionError("pdf_resource_limit") from e
