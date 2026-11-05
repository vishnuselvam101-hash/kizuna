"""印影 (red seal) region detection on scanned pages, without image libraries in the sandbox.

pdftoppm (already used for OCR) renders the page small: a scanned PDF directly, a PNG/JPEG by wrapping its
compressed data unchanged in a one-image PDF (PNG IDAT is a Flate stream with PNG predictors, JPEG is DCT).
Red-dominant pixels of the PPM are grouped into 4-connected components; components of seal size and roughly
round shape are reported as regions in OCR pixel space. A region is evidence that a mark is present, never
that something was approved; the role comes from the nearest approval label above it (設計/検図/承認).
"""

import shutil
import struct
import subprocess  # noqa: S404  (fixed argv, no shell)

from .errors import ExtractionError

SCALE = 4  # OCR raster px per low-res px (OCR at 200 dpi, detection at 50 dpi)
MIN_SIDE, MAX_SIDE = 6, 60  # low-res px: about 3-30 mm at 50 dpi
MIN_FILL = 0.12  # share of red pixels inside the component's box (ring-shaped seals are sparse)


def png_as_pdf(data: bytes) -> bytes | None:
    """Wraps an 8-bit non-interlaced RGB / grey / palette PNG in a PDF without decoding it; else None."""
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        return None
    pos, idat, plte, hdr = 8, [], b"", None
    while pos + 8 <= len(data):
        (n,) = struct.unpack(">I", data[pos : pos + 4])
        kind, body = data[pos + 4 : pos + 8], data[pos + 8 : pos + 8 + n]
        if len(body) != n:
            return None
        if kind == b"IHDR" and n == 13:
            hdr = struct.unpack(">IIBBBBB", body)
        elif kind == b"PLTE":
            plte = body
        elif kind == b"IDAT":
            idat.append(body)
        elif kind == b"IEND":
            break
        pos += 12 + n
    if hdr is None or not idat:
        return None
    w, h, depth, ctype, _c, _f, interlace = hdr
    if depth != 8 or interlace != 0 or ctype not in (0, 2, 3) or not (0 < w <= 20000 and 0 < h <= 20000):
        return None
    colors = {0: 1, 2: 3, 3: 1}[ctype]
    if ctype == 3:
        if not plte or len(plte) % 3:
            return None
        cs = f"[/Indexed /DeviceRGB {len(plte) // 3 - 1} <{plte.hex()}>]"
    else:
        cs = "/DeviceRGB" if ctype == 2 else "/DeviceGray"
    parms = f"/DecodeParms << /Predictor 15 /Colors {colors} /BitsPerComponent 8 /Columns {w} >>"
    return _one_image_pdf(
        w, h, f"/ColorSpace {cs} /BitsPerComponent 8 /Filter /FlateDecode {parms}", b"".join(idat)
    )


def jpeg_as_pdf(data: bytes) -> bytes | None:
    """Wraps a baseline/progressive 3-component JPEG in a PDF (DCTDecode); None for other JPEGs."""
    i = 2
    while data[:2] == b"\xff\xd8" and i + 10 <= len(data) and data[i] == 0xFF:
        marker = data[i + 1]
        (seg,) = struct.unpack(">H", data[i + 2 : i + 4])
        if marker in (0xC0, 0xC1, 0xC2):
            h, w = struct.unpack(">HH", data[i + 5 : i + 9])
            if data[i + 9] != 3 or not (0 < w <= 20000 and 0 < h <= 20000):
                return None
            return _one_image_pdf(w, h, "/ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode", data)
        i += 2 + seg
    return None


def _one_image_pdf(w: int, h: int, image_dict: str, stream: bytes) -> bytes:
    content = f"q {w} 0 0 {h} 0 0 cm /Im0 Do Q".encode()
    objs = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Contents 4 0 R "
        f"/Resources << /XObject << /Im0 5 0 R >> >> >>".encode(),
        f"<< /Length {len(content)} >>\nstream\n".encode() + content + b"\nendstream",
        f"<< /Type /XObject /Subtype /Image /Width {w} /Height {h} {image_dict} /Length {len(stream)} >>\n"
        f"stream\n".encode()
        + stream
        + b"\nendstream",
    ]
    out = b"%PDF-1.4\n"
    offs = []
    for i, o in enumerate(objs, start=1):
        offs.append(len(out))
        out += f"{i} 0 obj\n".encode() + o + b"\nendobj\n"
    x = len(out)
    out += f"xref\n0 {len(objs) + 1}\n0000000000 65535 f \n".encode()
    out += b"".join(f"{o:010d} 00000 n \n".encode() for o in offs)
    return out + f"trailer\n<< /Size {len(objs) + 1} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n".encode()


def render_small(pdf: bytes, page: int, dpi: float, timeout: int = 60) -> tuple[int, int, bytes] | None:
    """pdftoppm page → (width, height, RGB bytes) at `dpi`; None when the page does not exist."""
    exe = shutil.which("pdftoppm")
    if exe is None:
        raise ExtractionError("rasterizer_unavailable")
    try:
        proc = subprocess.run(  # noqa: S603
            [exe, "-r", f"{dpi:g}", "-f", str(page), "-l", str(page), "-singlefile", "-"],
            input=pdf,
            capture_output=True,
            timeout=timeout,
            check=False,
            env={"PATH": "/usr/bin"},
        )
    except subprocess.TimeoutExpired as e:
        raise ExtractionError("rasterize_timeout") from e
    return parse_ppm(proc.stdout) if proc.returncode == 0 else None


def parse_ppm(data: bytes) -> tuple[int, int, bytes] | None:
    """Binary P6 with maxval 255 only."""
    parts: list[bytes] = []
    pos = 0
    while len(parts) < 4 and pos < min(len(data), 200):
        while pos < len(data) and data[pos : pos + 1].isspace():
            pos += 1
        if data[pos : pos + 1] == b"#":
            pos = data.find(b"\n", pos) + 1 or len(data)
            continue
        end = pos
        while end < len(data) and not data[end : end + 1].isspace():
            end += 1
        parts.append(data[pos:end])
        pos = end
    if len(parts) < 4 or parts[0] != b"P6" or parts[3] != b"255":
        return None
    try:
        w, h = int(parts[1]), int(parts[2])
    except ValueError:
        return None
    px = data[pos + 1 : pos + 1 + w * h * 3]
    return (w, h, px) if w > 0 and h > 0 and len(px) == w * h * 3 else None


def red_mask(w: int, h: int, px: bytes) -> bytearray:
    m = bytearray(w * h)
    for i in range(w * h):
        r, g, b = px[3 * i], px[3 * i + 1], px[3 * i + 2]
        if r > 140 and r - max(g, b) > 60:
            m[i] = 1
    return m


def components(w: int, h: int, mask: bytearray, limit: int = 200) -> list[tuple[int, int, int, int, int]]:
    """4-connected components of the mask: (x0, y0, x1, y1, pixel count), at most `limit` of them."""
    seen = bytearray(w * h)
    out = []
    for start in range(w * h):
        if not mask[start] or seen[start]:
            continue
        stack, n = [start], 0
        seen[start] = 1
        x0, y0, x1, y1 = w, h, 0, 0
        while stack:
            i = stack.pop()
            n += 1
            y, x = divmod(i, w)
            x0, y0, x1, y1 = min(x0, x), min(y0, y), max(x1, x), max(y1, y)
            for j in (i - 1 if x else -1, i + 1 if x + 1 < w else -1, i - w, i + w):
                if 0 <= j < w * h and mask[j] and not seen[j]:
                    seen[j] = 1
                    stack.append(j)
        out.append((x0, y0, x1, y1, n))
        if len(out) >= limit:
            break
    return out


def seal_regions(w: int, h: int, px: bytes) -> list[dict]:
    """Seal-sized, roughly round red components → [{bbox_px (OCR pixels), red_px}] top-to-bottom."""
    found = []
    for x0, y0, x1, y1, n in components(w, h, red_mask(w, h, px)):
        bw, bh = x1 - x0 + 1, y1 - y0 + 1
        if not (MIN_SIDE <= bw <= MAX_SIDE and MIN_SIDE <= bh <= MAX_SIDE and 0.6 <= bw / bh <= 1.6):
            continue
        if n < MIN_FILL * bw * bh:
            continue
        found.append({"bbox_px": [x0 * SCALE, y0 * SCALE, (x1 + 1) * SCALE, (y1 + 1) * SCALE], "red_px": n})
    return sorted(found, key=lambda r: (r["bbox_px"][1], r["bbox_px"][0]))


def assign_roles(regions: list[dict], lines: list, roles: set[str]) -> list[tuple[dict, str]]:
    """Each region gets the nearest approval label above it in the same column ('' when none is close)."""
    from .ja import nfkc

    labels = []
    for ln in lines:
        top_bottom = ln.extra.get("px_bbox")
        if top_bottom is None:
            continue
        for c in ln.cells:
            if nfkc(c.text) in roles:
                labels.append((nfkc(c.text), (c.x0 + c.x1) / 2, top_bottom[3]))
    out = []
    for r in regions:
        x0, y0, x1, y1 = r["bbox_px"]
        cx, size = (x0 + x1) / 2, max(x1 - x0, y1 - y0)
        near = [
            (y0 - bottom, role)
            for role, lx, bottom in labels
            if abs(lx - cx) <= size and -size * 0.5 <= y0 - bottom <= size * 3
        ]
        out.append((r, min(near)[1] if near else ""))
    return out
