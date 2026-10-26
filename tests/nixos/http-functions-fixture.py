"""Local JWKS and upstream HTTP fixture. The HMAC key is public test data."""
import base64
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/jwks.json":
            key = base64.urlsafe_b64encode(
                b"nixos-http-functions-test-key-0123456789abcdef"
            ).rstrip(b"=").decode()
            body = {"keys": [{"kty": "oct", "kid": "nixos-test", "alg": "HS256", "k": key}]}
        else:
            body = {"path": self.path, "injected": self.headers.get("X-Upstream-Token")}
        data = json.dumps(body).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


if __name__ == "__main__":
    ThreadingHTTPServer(("127.0.0.1", 8081), Handler).serve_forever()
