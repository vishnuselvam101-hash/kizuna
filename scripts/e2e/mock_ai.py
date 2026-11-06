#!/usr/bin/env python3
"""Local mock of an OpenAI-compatible /v1/chat/completions endpoint for browser acceptance tests.

Usage: mock_ai.py PORT   (binds 127.0.0.1 only; stdlib only; no data leaves the host)

It answers the `ai-map@1` prompt deterministically: labels containing a known Japanese stem are mapped to a
vocabulary field, everything else is left unmapped. Each request is appended to $MOCK_AI_LOG (if set) so a test
can prove the provider was (or was not) called.
"""

import json
import os
import re
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

RULES = [("製造者", "supplier"), ("製造元", "supplier"), ("メーカー", "supplier"), ("全長", "length"), ("寸法", "length")]


def answer(user: str) -> dict:
    m = re.search(r"<document_data>\s*(.*?)\s*</document_data>", user, re.S)
    items = json.loads(m.group(1))["items"] if m else []
    out = []
    for it in items:
        for stem, field in RULES:
            if stem in it.get("label", ""):
                out.append({"id": it["id"], "field": field, "confidence": 0.9})
                break
    return {"mappings": out}


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):  # quiet
        pass

    def do_POST(self):
        n = int(self.headers.get("content-length") or 0)
        body = json.loads(self.rfile.read(n) or b"{}")
        user = next((m["content"] for m in body.get("messages", []) if m.get("role") == "user"), "")
        content = json.dumps(answer(user), ensure_ascii=False)
        if os.environ.get("MOCK_AI_LOG"):
            with open(os.environ["MOCK_AI_LOG"], "a", encoding="utf-8") as f:
                f.write(json.dumps({"path": self.path, "model": body.get("model"), "answer": content}, ensure_ascii=False) + "\n")
        resp = {
            "choices": [{"message": {"role": "assistant", "content": content}}],
            "usage": {"prompt_tokens": len(user) // 4, "completion_tokens": len(content) // 4},
        }
        data = json.dumps(resp).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


if __name__ == "__main__":
    ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
