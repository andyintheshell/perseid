#!/usr/bin/env python3
"""Local, synthetic HTTP target for the GitHub Actions demo recording."""

import json
import os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlsplit


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        parsed = urlsplit(self.path)
        with open(os.environ["DEMO_REQUEST_LOG"], "a", encoding="utf-8") as log:
            log.write(json.dumps({"path": self.path}) + "\n")
        if parsed.path == "/api/products":
            product_id = parse_qs(parsed.query).get("id", ["1"])[0]
            data = {"id": product_id, "name": "Demo product", "in_stock": True}
        else:
            data = {"service": "Perseid demo", "products": [1, 2, 42]}
        body = json.dumps(data, indent=2).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format, *args):
        pass


if __name__ == "__main__":
    ThreadingHTTPServer(("127.0.0.1", 18090), Handler).serve_forever()
