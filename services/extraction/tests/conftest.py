"""Test-session hooks.

`KIZUNA_REQUEST_DUMP=<dir>` records every worker request the suite builds (CSV, XLSX, PDF, OCR,
DXF, images, hostile inputs) as one file per distinct request. scripts/seccomp-profile.sh replays
that corpus through the real sandbox under strace to derive the seccomp allowlist (ADR 0016).
Off by default; no effect on the tests.
"""

import base64
import hashlib
import json
import os
from pathlib import Path

from kizuna_extraction import documents, worker

_DUMP = os.environ.get("KIZUNA_REQUEST_DUMP")

if _DUMP:
    _dir = Path(_DUMP)
    _dir.mkdir(parents=True, exist_ok=True)
    _handle = worker.handle
    _pdf_extract = documents.pdf_extract

    def _record(raw: bytes) -> None:
        if len(raw) <= worker.MAX_INPUT:
            (_dir / hashlib.sha256(raw).hexdigest()[:24]).write_bytes(raw)

    def _recording_handle(raw: bytes) -> dict:
        _record(raw)
        return _handle(raw)

    def _recording_pdf_extract(data: bytes, *a, **k):  # scanned/Japanese PDFs tested below the worker entry
        req = {"media_type": "application/pdf", "data_b64": base64.b64encode(data).decode()}
        _record(json.dumps(req).encode())
        return _pdf_extract(data, *a, **k)

    worker.handle = _recording_handle
    documents.pdf_extract = _recording_pdf_extract
