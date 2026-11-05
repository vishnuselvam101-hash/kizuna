#!/usr/bin/env bash
# Network-level red-team probes against a LOCAL kizuna-server only (docs/security/REDTEAM_2026-11-05.md):
# request smuggling / framing, header injection, declared-size lies, CORS, slow-header (slowloris) bounds.
# Usage: scripts/redteam.sh [host:port]   (default 127.0.0.1:8080; refuses non-loopback targets)
# Prints one `RT|attack|expected|actual` line per attempt; exits non-zero when an attempt is not refused.
set -euo pipefail
TARGET=${1:-127.0.0.1:8080}
case ${TARGET%:*} in 127.0.0.1 | localhost | ::1) ;; *) echo "refusing non-loopback target $TARGET" >&2; exit 2 ;; esac
exec python3 -I - "$TARGET" <<'PY'
import socket, sys, time
host, port = sys.argv[1].rsplit(":", 1); port = int(port)
fails = 0

def rt(a, e, actual, ok):
    global fails
    print(f"RT|{a}|{e}|{actual}{'' if ok else '  <-- NOT REFUSED'}")
    fails += 0 if ok else 1

def raw(data, wait=3.0):
    s = socket.create_connection((host, port), timeout=wait)
    s.sendall(data)
    out = b""
    try:
        while True:
            d = s.recv(65536)
            if not d: break
            out += d
    except socket.timeout:
        pass
    s.close()
    return out

def status_line(r):
    return r.split(b"\r\n", 1)[0].decode(errors="replace") or "(closed, no response)"

def n_responses(r):
    return r.count(b"HTTP/1.1 ")

H = f"Host: {host}\r\n"
login = b'{"tenant":"x","email":"a@b.c","password":"p"}'

# 1. CL + TE together (classic CL.TE / TE.CL smuggling): must be rejected or treated as one request.
smug = (f"POST /api/v1/auth/login HTTP/1.1\r\n{H}Content-Type: application/json\r\nContent-Length: 4\r\n"
        "Transfer-Encoding: chunked\r\n\r\n0\r\n\r\nGET /healthz HTTP/1.1\r\n" + H + "\r\n").encode()
r = raw(smug); rt("CL+TE smuggling", "400 or 1 response", f"{status_line(r)} responses={n_responses(r)}", n_responses(r) <= 1 or r.startswith(b"HTTP/1.1 400"))
# 2. Duplicate, conflicting Content-Length.
r = raw((f"POST /api/v1/auth/login HTTP/1.1\r\n{H}Content-Length: 5\r\nContent-Length: 46\r\nContent-Type: application/json\r\n\r\n").encode() + login)
rt("duplicate conflicting Content-Length", "400", status_line(r), r.startswith(b"HTTP/1.1 400"))
# 3. Obfuscated Transfer-Encoding.
r = raw((f"POST /api/v1/auth/login HTTP/1.1\r\n{H}Transfer-Encoding: xchunked\r\nContent-Length: 46\r\n\r\n").encode() + login)
rt("Transfer-Encoding: xchunked", "400/501", status_line(r), not r.startswith(b"HTTP/1.1 2"))
# 4. Chunk size overflow.
r = raw((f"POST /api/v1/auth/login HTTP/1.1\r\n{H}Transfer-Encoding: chunked\r\n\r\nFFFFFFFFFFFFFFFFFF\r\nx\r\n0\r\n\r\n").encode())
rt("chunk size overflow", "400 / close", status_line(r), not r.startswith(b"HTTP/1.1 2"))
# 5. Declared size larger than the body (lie): must not hang past the request timeout or succeed.
t = time.time(); r = raw((f"POST /api/v1/auth/login HTTP/1.1\r\n{H}Content-Type: application/json\r\nContent-Length: 1000\r\n\r\n").encode() + login, wait=5)
rt("Content-Length 1000, 46 bytes sent", "no 2xx", f"{status_line(r)} after {time.time()-t:.1f}s", not r.startswith(b"HTTP/1.1 2"))
# 6. Declared size smaller than the body: the rest must not become a second (smuggled) request.
r = raw((f"POST /api/v1/auth/login HTTP/1.1\r\n{H}Content-Type: application/json\r\nContent-Length: 46\r\n\r\n").encode() + login + b"GET /api/v1/auth/me HTTP/1.1\r\n" + H.encode() + b"Authorization: Bearer x\r\n\r\n")
rt("Content-Length shorter than body (pipelined tail)", "2nd request is a separate, unauthenticated request", f"responses={n_responses(r)} {[l.decode() for l in r.split(b'\r\n') if l.startswith(b'HTTP/1.1')]}", b"200 OK" not in r.split(b"HTTP/1.1 ")[-1][:10])
# 7. Huge declared body on JSON routes (64 KiB limit) and upload route (25 MiB + 64 KiB).
r = raw((f"POST /api/v1/auth/login HTTP/1.1\r\n{H}Content-Type: application/json\r\nContent-Length: 999999999\r\n\r\n").encode())
rt("Content-Length 999999999 on JSON route", "413", status_line(r), b" 413 " in r[:20])
r = raw((f"POST /api/v1/artifacts HTTP/1.1\r\n{H}Authorization: Bearer x\r\nContent-Type: multipart/form-data; boundary=b\r\nContent-Length: 99999999999\r\n\r\n").encode())
rt("Content-Length 99999999999 on upload route", "401/413", status_line(r), not r.startswith(b"HTTP/1.1 2"))
# 8. Header injection: CR/LF / NUL inside header values, oversized headers, many headers.
r = raw((f"GET /healthz HTTP/1.1\r\n{H}X-A: a\rInjected: 1\r\n\r\n").encode())
rt("bare CR inside header value", "400", status_line(r), r.startswith(b"HTTP/1.1 400") or not r)
r = raw((f"GET /healthz HTTP/1.1\r\n{H}X-A: a\x00b\r\n\r\n").encode())
rt("NUL inside header value", "400", status_line(r), r.startswith(b"HTTP/1.1 400") or not r)
r = raw((f"GET /healthz HTTP/1.1\r\n{H}X-Big: " + "a" * 1_000_000 + "\r\n\r\n").encode(), wait=5)
rt("1 MB header", "431/400/close", status_line(r), not r.startswith(b"HTTP/1.1 2"))
r = raw((f"GET /healthz HTTP/1.1\r\n{H}" + "".join(f"X-{i}: v\r\n" for i in range(500)) + "\r\n").encode())
rt("500 headers", "431/400/close", status_line(r), not r.startswith(b"HTTP/1.1 2"))
r = raw((f"GET /healthz HTTP/1.1\r\n{H}X-Forwarded-For: 1.2.3.4\r\n Folded: continuation\r\n\r\n").encode())
rt("obsolete line folding", "400", status_line(r), r.startswith(b"HTTP/1.1 400") or not r)
r = raw(b"GET /api/v1/auth/me HTTP/1.1\r\n" + H.encode() + b"Authorization: Bearer kz_at.x.y\r\nAuthorization: Bearer kz_at.z.w\r\n\r\n")
rt("duplicate Authorization headers", "401", status_line(r), b" 401 " in r[:20])
# 9. Response header injection via reflected values (none expected): request id / path with CRLF.
r = raw(b"GET /api/v1/%0d%0aSet-Cookie:%20x=1 HTTP/1.1\r\n" + H.encode() + b"\r\n")
rt("CRLF in path reflected into headers", "no Set-Cookie", status_line(r), b"\r\nSet-Cookie" not in r.split(b"\r\n\r\n")[0])
# 10. CORS: preflight and simple request from a foreign origin.
r = raw((f"OPTIONS /api/v1/users HTTP/1.1\r\n{H}Origin: https://evil.example\r\nAccess-Control-Request-Method: POST\r\nAccess-Control-Request-Headers: authorization\r\n\r\n").encode())
rt("CORS preflight evil origin", "no Access-Control-Allow-Origin", status_line(r), b"access-control-allow-origin" not in r.lower())
r = raw((f"GET /healthz HTTP/1.1\r\n{H}Origin: null\r\n\r\n").encode())
rt("CORS Origin: null", "no ACAO", status_line(r), b"access-control-allow-origin" not in r.lower())
# 11. Security headers present on an error response.
r = raw((f"GET /api/v1/nope HTTP/1.1\r\n{H}\r\n").encode()).lower()
need = [b"x-content-type-options: nosniff", b"content-security-policy", b"cache-control: no-store", b"x-frame-options: deny"]
rt("security headers on 404", "all present", ",".join(n.decode().split(":")[0] for n in need if n not in r) or "all", all(n in r for n in need))
# 12. HTTP/2 prior knowledge / h2c upgrade (server is http1 only).
r = raw(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n")
rt("HTTP/2 prior-knowledge preface", "rejected", status_line(r), not r.startswith(b"HTTP/1.1 2"))
# 13. Slow headers (slowloris): an incomplete header block must be closed within a bounded time.
s = socket.create_connection((host, port)); s.sendall(b"GET /healthz HTTP/1.1\r\n" + H.encode())
s.settimeout(40); t = time.time()
try:
    d = s.recv(100); closed = True
except socket.timeout:
    d = b""; closed = False
el = time.time() - t; s.close()
rt("incomplete header block (slowloris)", "closed <= 35 s", f"{'closed' if closed else 'still open'} after {el:.0f}s {status_line(d) if d else ''}", closed and el <= 35)
# 14. Slow drip: one header byte every 5 s keeps the connection alive?
s = socket.create_connection((host, port)); s.sendall(b"GET /healthz HTTP/1.1\r\n"); t = time.time(); alive = True
try:
    while time.time() - t < 45:
        s.sendall(b"X"); time.sleep(5)
except OSError:
    alive = False
el = time.time() - t; s.close()
rt("header drip 1 byte / 5 s", "closed <= 35 s", f"{'open' if alive else 'closed'} after {el:.0f}s", not alive and el <= 40)
# 15. Idle keep-alive after one request.
s = socket.create_connection((host, port)); s.sendall(b"GET /healthz HTTP/1.1\r\n" + H.encode() + b"\r\n"); s.settimeout(3); s.recv(1000)
s.settimeout(70); t = time.time()
try:
    d = s.recv(100); el = time.time() - t; res = f"closed after {el:.0f}s"; ok = el <= 65
except socket.timeout:
    res = "open after 70s"; ok = False
s.close()
rt("idle keep-alive connection", "closed <= 65 s", res, ok)
print(f"RT-SUMMARY|{fails} attempt(s) not refused")
sys.exit(1 if fails else 0)
PY
