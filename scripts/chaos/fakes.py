#!/usr/bin/env python3
"""Chaos fakes for KIZUNA: a clamd (unix socket, INSTREAM) and an OpenAI-compatible AI endpoint whose behaviour is
switched at runtime by writing a word into a mode file. Stdlib only.

  fakes.py clamd SOCKET MODEFILE   modes: ok | hang | close | error   (EICAR -> FOUND in ok mode)
  fakes.py ai PORT MODEFILE        modes: ok | slow | garbage | huge | e500 | e429
"""
import http.server, json, os, socket, socketserver, struct, sys, threading, time

def mode(f):
    try:
        return open(f).read().strip() or "ok"
    except OSError:
        return "ok"

def clamd(path, mf):
    try:
        os.unlink(path)
    except FileNotFoundError:
        pass
    srv = socket.socket(socket.AF_UNIX)
    srv.bind(path)
    srv.listen(64)

    def handle(c):
        try:
            m = mode(mf)
            if m == "close":
                return
            if m == "hang":
                time.sleep(3600)
                return
            buf = b""
            while b"\0" not in buf:
                d = c.recv(4096)
                if not d:
                    return
                buf += d
            cmd, rest = buf.split(b"\0", 1)
            if cmd in (b"zVERSION", b"zPING"):
                c.sendall(b"ClamAV 1.0.0-fake/1/Thu Jan  1 00:00:00 2026\0" if cmd == b"zVERSION" else b"PONG\0")
                return
            data = b""
            while True:
                while len(rest) < 4:
                    d = c.recv(65536)
                    if not d:
                        return
                    rest += d
                (n,) = struct.unpack(">I", rest[:4])
                rest = rest[4:]
                if n == 0:
                    break
                while len(rest) < n:
                    d = c.recv(65536)
                    if not d:
                        return
                    rest += d
                data += rest[:n]
                rest = rest[n:]
            if m == "error":
                c.sendall(b"Can't allocate memory ERROR\0")
            elif b"EICAR-STANDARD-ANTIVIRUS-TEST-FILE" in data:
                c.sendall(b"stream: Eicar-Test-Signature FOUND\0")
            else:
                c.sendall(b"stream: OK\0")
        except OSError:
            pass
        finally:
            c.close()

    while True:
        c, _ = srv.accept()
        threading.Thread(target=handle, args=(c,), daemon=True).start()

def ai(port, mf):
    class H(http.server.BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def do_POST(self):
            n = int(self.headers.get("Content-Length", 0))
            req = json.loads(self.rfile.read(n) or b"{}")
            m = mode(mf)
            if m == "slow":
                time.sleep(120)
            if m in ("e500", "e429"):
                self.send_response(int(m[1:]))
                self.end_headers()
                return
            if m == "garbage":
                body = os.urandom(512)
            elif m == "huge":
                body = b'{"choices":[{"message":{"content":"' + b"A" * 400_000 + b'"}}]}'
            else:
                # Valid but uninformative mapping: an empty suggestion set.
                body = json.dumps({"choices": [{"message": {"content": json.dumps({"mappings": []})}}],
                                   "usage": {"prompt_tokens": 1, "completion_tokens": 1}}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    class S(socketserver.ThreadingMixIn, http.server.HTTPServer):
        daemon_threads = True

    S(("127.0.0.1", int(port)), H).serve_forever()

if __name__ == "__main__":
    {"clamd": clamd, "ai": ai}[sys.argv[1]](sys.argv[2], sys.argv[3])
