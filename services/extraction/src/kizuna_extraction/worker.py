"""Worker entry point. Protocol: one JSON object on stdin
   {"media_type": str, "data_b64": str, "limits": {...}}
and one JSON object on stdout
   {"ok": true, "extractor": "pdf@1", "records": [...], "warnings": [...]} or {"ok": false, "error": code}.
"""

import base64
import binascii
import json
import resource
import socket
import sys

from . import __version__
from .documents import TesseractOcr, pdf_pages
from .tabular import ExtractionError, csv_records, xlsx_records
from .textrules import records_from_text

DEFAULT_LIMITS = {
    "max_records": 20000,
    "max_pages": 500,
    "max_text_chars": 5_000_000,
    "max_member_bytes": 50 * 1024 * 1024,
    "max_sheets": 50,
}
MAX_INPUT = 64 * 1024 * 1024

XLSX = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
IMAGES = {"image/png", "image/jpeg", "image/tiff"}


def harden() -> None:
    """Defence in depth inside the sandbox: no new files, bounded CPU, no sockets."""
    resource.setrlimit(resource.RLIMIT_FSIZE, (0, 0))
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))

    def _blocked(*_a, **_k):
        raise PermissionError("network disabled in extraction worker")

    socket.socket = _blocked  # type: ignore[assignment,misc]
    socket.create_connection = _blocked  # type: ignore[assignment]


def extract(media_type: str, data: bytes, limits: dict) -> tuple[str, list[dict], list[str]]:
    warnings: list[str] = []
    if media_type == "text/csv":
        return "csv@1", csv_records(data, limits), warnings
    if media_type == XLSX:
        return "xlsx@1", xlsx_records(data, limits), warnings
    if media_type == "application/pdf":
        pages = pdf_pages(data, limits)
        if not any(p.strip() for p in pages):
            warnings.append("pdf_has_no_text_layer_ocr_not_applied")
        return "pdf@1", records_from_text(pages, limits["max_records"]), warnings
    if media_type in IMAGES:
        ocr = TesseractOcr()
        if not ocr.available():
            return "ocr-none@1", [], ["ocr_unavailable"]
        return "ocr-tesseract@1", records_from_text([ocr.image_text(data)], limits["max_records"]), warnings
    raise ExtractionError("unsupported_media_type")


def handle(raw: bytes) -> dict:
    try:
        req = json.loads(raw)
        data = base64.b64decode(req["data_b64"], validate=True)
        limits = {
            **DEFAULT_LIMITS,
            **{k: int(v) for k, v in req.get("limits", {}).items() if k in DEFAULT_LIMITS},
        }
        extractor, records, warnings = extract(str(req["media_type"]), data, limits)
        return {
            "ok": True,
            "extractor": extractor,
            "worker_version": __version__,
            "records": records[: limits["max_records"]],
            "warnings": warnings,
        }
    except ExtractionError as e:
        return {"ok": False, "error": str(e)}
    except (KeyError, ValueError, TypeError, binascii.Error):
        return {"ok": False, "error": "bad_request"}
    except Exception:  # noqa: BLE001  (malformed documents must never crash without a structured answer)
        return {"ok": False, "error": "extractor_crashed"}


def main() -> None:
    harden()
    raw = sys.stdin.buffer.read(MAX_INPUT + 1)
    if len(raw) > MAX_INPUT:
        out = {"ok": False, "error": "input_too_large"}
    else:
        out = handle(raw)
    sys.stdout.write(json.dumps(out, ensure_ascii=False))
    sys.stdout.flush()


if __name__ == "__main__":
    main()
