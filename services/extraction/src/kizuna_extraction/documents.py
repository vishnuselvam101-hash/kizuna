"""PDF text extraction (pypdf) and OCR (tesseract CLI) behind a small interface."""

import io
import shutil
import subprocess  # noqa: S404  (fixed argv, no shell, used for the OCR engine only)

from .tabular import ExtractionError


def pdf_pages(data: bytes, limits: dict) -> list[str]:
    from pypdf import PdfReader
    from pypdf.errors import PdfReadError

    try:
        reader = PdfReader(io.BytesIO(data), strict=False)
        if reader.is_encrypted:
            raise ExtractionError("pdf_encrypted")
        pages = []
        total = 0
        for i, page in enumerate(reader.pages):
            if i >= limits["max_pages"]:
                break
            text = page.extract_text() or ""
            total += len(text)
            if total > limits["max_text_chars"]:
                raise ExtractionError("pdf_text_too_large")
            pages.append(text)
        return pages
    except PdfReadError as e:
        raise ExtractionError("pdf_unreadable") from e


class OcrEngine:
    name = "none"

    def available(self) -> bool:
        return False

    def image_text(self, data: bytes) -> str:
        raise ExtractionError("ocr_unavailable")


class TesseractOcr(OcrEngine):
    name = "tesseract"

    def __init__(self, langs: str = "eng+jpn", timeout: int = 60):
        self.langs = langs
        self.timeout = timeout

    def available(self) -> bool:
        return shutil.which("tesseract") is not None

    def image_text(self, data: bytes) -> str:
        exe = shutil.which("tesseract")
        if exe is None:
            raise ExtractionError("ocr_unavailable")
        try:
            proc = subprocess.run(  # noqa: S603
                [exe, "stdin", "stdout", "-l", self.langs, "--psm", "6"],
                input=data,
                capture_output=True,
                timeout=self.timeout,
                check=False,
            )
        except subprocess.TimeoutExpired as e:
            raise ExtractionError("ocr_timeout") from e
        if proc.returncode != 0:
            raise ExtractionError("ocr_failed")
        return proc.stdout.decode("utf-8", "replace")
