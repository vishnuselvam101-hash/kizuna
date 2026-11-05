"""Worker entry point (run by crates/kizuna-api/src/extraction.rs inside bubblewrap + prlimit).

stdin:  {"media_type": str, "data_b64": str, "limits"?: {...}, "options"?: {...}, "claim_extractor"?: "rules"}
        options (tenant import settings, all optional, validated against a fixed whitelist):
        csv_delimiter ","|";"|"\t", csv_encoding "utf-8"|"shift_jis",
        header_row 0..100 (0 = first non-empty row), xlsx_sheet (name, "" = all), ocr bool,
        ocr_language "jpn+eng"|"eng+jpn"|"eng"|"jpn"
stdout: {"ok": true, "extractor": "pdf@2", "worker_version", "records": [...], "claims": [...],
         "unmapped_labels": [...], "warnings": [...]}  or  {"ok": false, "error": code}

The request carries document bytes only: no paths, tenant ids or URLs are accepted, so the worker cannot be
pointed
at another tenant's data; unknown request keys are refused. Records keep the {label, value, location, group}
shape
the Rust mapping consumes and add `method` and `confidence`.
"""

import base64
import binascii
import json
import resource
import socket
import sys

from . import __version__
from .claims import get_claim_extractor
from .documents import pdf_extract
from .drawing import STAMP_LABELS, dxf_records, pdf_metadata, title_block
from .errors import ExtractionError
from .ocr import TesseractOcr, image_pixels, rasterize_pdf
from .records import Sink, _rec, records_from_lines
from .stamps import assign_roles, jpeg_as_pdf, png_as_pdf, render_small, seal_regions
from .tabular import csv_records, xlsx_records

DEFAULT_LIMITS = {
    "max_records": 20000,
    "max_pages": 500,
    "max_page_chars": 1_000_000,
    "max_text_chars": 5_000_000,
    "max_fragments_per_page": 50_000,
    "max_stream_bytes": 32 * 1024 * 1024,
    "max_member_bytes": 50 * 1024 * 1024,
    "max_total_unzipped": 200 * 1024 * 1024,
    "max_zip_members": 2000,
    "max_sheets": 50,
    "max_cells": 2_000_000,
    "max_rows": 1_000_000,
    "max_cols": 1000,
    "max_cell_chars": 1_000_000,
    "max_dxf_pairs": 5_000_000,
    "max_ocr_pages": 20,
    "max_image_pixels": 60_000_000,
}
MAX_INPUT = 64 * 1024 * 1024
MAX_OUTPUT = 15 * 1024 * 1024  # API refuses > 16 MiB
REQUEST_KEYS = {"media_type", "data_b64", "limits", "options", "claim_extractor"}
DEFAULT_OPTIONS: dict = {
    "csv_delimiter": None,
    "csv_encoding": None,
    "header_row": 0,
    "xlsx_sheet": "",
    "ocr": True,
    "ocr_language": "eng+jpn",
}


def parse_options(raw: object) -> dict:
    """Tenant import settings, strictly validated: unknown keys or values make the request invalid."""
    if not isinstance(raw, dict) or set(raw) - set(DEFAULT_OPTIONS):
        raise ValueError("options")
    o = dict(DEFAULT_OPTIONS)
    for k, v in raw.items():
        ok = {
            "csv_delimiter": v in (",", ";", "\t"),
            "csv_encoding": v in ("utf-8", "shift_jis"),
            "header_row": type(v) is int and 0 <= v <= 100,
            "xlsx_sheet": isinstance(v, str) and len(v) <= 100,
            "ocr": type(v) is bool,
            "ocr_language": v in ("jpn+eng", "eng+jpn", "eng", "jpn"),
        }[k]
        if not ok:
            raise ValueError(f"option {k}")
        o[k] = v
    return o


XLSX = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
DXF = {"image/vnd.dxf", "application/dxf"}
IMAGES = {"image/png", "image/jpeg", "image/tiff"}


def harden() -> None:
    """Defence in depth inside the sandbox: no new files, no core dumps, no sockets."""
    resource.setrlimit(resource.RLIMIT_FSIZE, (0, 0))
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))

    def _blocked(*_a, **_k):
        raise PermissionError("network disabled in extraction worker")

    socket.socket = _blocked  # type: ignore[assignment,misc]
    socket.create_connection = _blocked  # type: ignore[assignment]


def extract(
    media_type: str, data: bytes, limits: dict, options: dict | None = None
) -> tuple[str, list[dict], list[str]]:
    opts = options or dict(DEFAULT_OPTIONS)
    sink = Sink(limits["max_records"])
    warnings: list[str] = []
    if media_type == "text/csv":
        warnings += csv_records(data, limits, sink, opts)
        name = "csv@2"
    elif media_type == XLSX:
        warnings += xlsx_records(data, limits, sink, opts)
        name = "xlsx@2"
    elif media_type == "application/pdf":
        lines, meta, w = pdf_extract(data, limits)
        warnings += w
        method = "pdf"
        if not lines and not opts["ocr"]:
            warnings.append("pdf_without_text_layer_ocr_disabled_by_settings")
        elif not lines:
            # Scanned PDF: render pages and OCR them (bounded page count), still inside the sandbox.
            ocr = TesseractOcr(opts["ocr_language"])
            if ocr.available():
                for pno, png in enumerate(
                    rasterize_pdf(data, min(limits["max_pages"], limits["max_ocr_pages"])), 1
                ):
                    page_lines = ocr.recognize(png, page=pno)
                    seals(data, pno, 50, page_lines, sink)
                    lines += page_lines
                method = "ocr"
                warnings.append("pdf_without_text_layer_ocr_applied")
            else:
                warnings.append("pdf_has_no_text_layer_ocr_unavailable")
        records_from_lines(lines, method, sink)
        title_block(lines, sink, method)
        pdf_metadata(meta, sink)
        name = "pdf@2"
    elif media_type in DXF:
        warnings += dxf_records(data, limits, sink)
        name = "dxf@1"
    elif media_type in IMAGES:
        px = image_pixels(data)
        if px is not None and px > limits["max_image_pixels"]:
            raise ExtractionError("image_too_large")
        if not opts["ocr"]:
            return "ocr-none@1", [], ["ocr_disabled_by_settings"]
        ocr = TesseractOcr(opts["ocr_language"])
        if not ocr.available():
            return "ocr-none@1", [], ["ocr_unavailable"]
        lines = ocr.recognize(data)
        records_from_lines(lines, "ocr", sink)
        title_block(lines, sink, "ocr")
        wrapped = png_as_pdf(data) or jpeg_as_pdf(data)
        if wrapped is None:
            warnings.append("stamp_detection_skipped:image_format")
        else:
            seals(wrapped, 1, 18, lines, sink)
        name = "ocr-tesseract@2"
    else:
        raise ExtractionError("unsupported_media_type")
    if sink.full:
        warnings.append("records_truncated")
    return name, sink.items, warnings


def seals(pdf: bytes, page: int, dpi: float, lines: list, sink: Sink) -> None:
    """印影 regions on one scanned page as records (label 印影, value = approval role or 'unassigned')."""
    small = render_small(pdf, page, dpi)
    if small is None:
        return
    for region, role in assign_roles(seal_regions(*small), lines, STAMP_LABELS):
        loc = {
            "page": page,
            "bbox_px": region["bbox_px"],
            "red_px": region["red_px"],
            "detector": "red-ink@1",
        }
        sink.add(_rec("印影", role or "unassigned", loc, "stamps", "ocr-stamp", 0.6))


def _bounded(out: dict) -> str:
    s = json.dumps(out, ensure_ascii=False)
    while len(s.encode()) > MAX_OUTPUT and (out.get("records") or out.get("claims")):
        out["records"] = out["records"][: len(out["records"]) // 2]
        out["claims"] = out["claims"][: len(out["claims"]) // 2]
        if "output_truncated" not in out["warnings"]:
            out["warnings"].append("output_truncated")
        s = json.dumps(out, ensure_ascii=False)
    return s


def _parse_request(raw: bytes) -> tuple[str, bytes, dict, dict, str]:
    req = json.loads(raw)
    if not isinstance(req, dict) or set(req) - REQUEST_KEYS:
        raise ValueError("unexpected request shape")
    data = base64.b64decode(req["data_b64"], validate=True)
    user_limits = req.get("limits", {})
    if not isinstance(user_limits, dict):
        raise TypeError("limits")
    limits = dict(DEFAULT_LIMITS)
    for k, v in user_limits.items():
        if k in DEFAULT_LIMITS:
            limits[k] = min(int(v), DEFAULT_LIMITS[k])  # callers may only tighten limits
    options = parse_options(req.get("options", {}))
    return str(req["media_type"]), data, limits, options, str(req.get("claim_extractor", "rules"))


def handle(raw: bytes) -> dict:
    try:
        media_type, data, limits, options, claimer_name = _parse_request(raw)
    except (KeyError, ValueError, TypeError, binascii.Error):
        return {"ok": False, "error": "bad_request"}
    try:
        claimer = get_claim_extractor(claimer_name)
        extractor, records, warnings = extract(media_type, data, limits, options)
        claims, unmapped, cw = claimer.extract(records)
        return {
            "ok": True,
            "extractor": extractor,
            "worker_version": __version__,
            "records": records,
            "claims": claims,
            "claim_extractor": claimer.name,
            "unmapped_labels": unmapped,
            "warnings": warnings + cw,
        }
    except ExtractionError as e:
        return {"ok": False, "error": str(e)}
    except RecursionError:
        return {"ok": False, "error": "document_too_deeply_nested"}
    except MemoryError:
        return {"ok": False, "error": "resource_limit"}
    except Exception:  # noqa: BLE001  (malformed documents must never crash without a structured answer)
        return {"ok": False, "error": "extractor_crashed"}


def main() -> None:
    harden()
    raw = sys.stdin.buffer.read(MAX_INPUT + 1)
    out = {"ok": False, "error": "input_too_large"} if len(raw) > MAX_INPUT else handle(raw)
    sys.stdout.write(_bounded(out) if out.get("ok") else json.dumps(out))
    sys.stdout.flush()


if __name__ == "__main__":
    main()
