#!/usr/bin/env python3
"""Local, synthetic HTTP target for the GitHub Actions demo recording."""

import base64
import json
import os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlsplit

PRODUCTS = {
    1: ("Router", 49.0),
    2: ("Cable", 9.5),
    42: ("Prototype widget", 129.0),
}


def encode(value):
    return base64.b64encode(value.encode("utf-8")).decode("ascii")


def product(product_id):
    return PRODUCTS.get(product_id, ("Demo product", 0.0))


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        parsed = urlsplit(self.path)
        with open(os.environ["DEMO_REQUEST_LOG"], "a", encoding="utf-8") as log:
            log.write(json.dumps({"path": self.path}) + "\n")

        query = parse_qs(parsed.query)
        if parsed.path == "/api/products":
            raw_id = query.get("id", ["1"])[0]
            product_id = int(raw_id) if raw_id.isdigit() else 1
            name, price = product(product_id)
            data = {
                "id": product_id,
                "name": name,
                "in_stock": True,
                "receipt": encode(f"order {product_id}: {name} - ${price:.2f}"),
            }
            self.respond(json.dumps(data, indent=2).encode("utf-8"), "application/json")
        elif parsed.path == "/api/export":
            raw_id = query.get("id", ["1"])[0]
            product_id = int(raw_id) if raw_id.isdigit() else 1
            name, price = product(product_id)
            body = encode(
                f"export product {product_id}: {name} - ${price:.2f} - in stock"
            )
            self.respond(body.encode("utf-8"), "text/plain; charset=utf-8")
        elif parsed.path == "/api/catalog":
            data = {
                "service": "Perseid demo",
                "products": [1, 2, 42],
                "banner": encode("Perseid demo catalog"),
            }
            self.respond(json.dumps(data, indent=2).encode("utf-8"), "application/json")
        else:
            self.respond(b"Not found", "text/plain; charset=utf-8", status=404)

    def respond(self, body, content_type, status=200):
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format, *args):
        pass


if __name__ == "__main__":
    ThreadingHTTPServer(("127.0.0.1", 18090), Handler).serve_forever()
