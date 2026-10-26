"""HTTP helpers shared by the NixOS VM tests; all traffic stays inside each VM."""
import base64
import hashlib
import hmac
import json
import shlex
import time
import zlib

TEST_SECRET = b"nixos-http-functions-test-key-0123456789abcdef"


def bearer_token():
    def encode(value):
        return base64.urlsafe_b64encode(json.dumps(value).encode()).rstrip(b"=")

    signing_input = b".".join([
        encode({"alg": "HS256", "kid": "nixos-test"}),
        encode({"sub": "nixos-test", "exp": 4102444800}),
    ])
    signature = base64.urlsafe_b64encode(
        hmac.new(TEST_SECRET, signing_input, hashlib.sha256).digest()
    ).rstrip(b"=")
    return (signing_input + b"." + signature).decode()


def http(node, method, path, body=None, headers=None, port=3000, token=None):
    headers = dict(headers or {})
    if token is not None:
        headers["Authorization"] = "Bearer " + token
    if isinstance(body, dict):
        headers["Content-Type"] = "application/json"
        body = json.dumps(body).encode()
    config = {
        "method": method,
        "url": f"http://127.0.0.1:{port}{path}",
        "headers": headers,
        "body": None if body is None else base64.b64encode(zlib.compress(body)).decode(),
    }
    client = """
import base64, json, sys, urllib.request, urllib.error, zlib
config = json.loads(sys.argv[1])
request = urllib.request.Request(
    config['url'], method=config['method'], headers=config['headers'],
    data=None if config['body'] is None else zlib.decompress(base64.b64decode(config['body'])))
try:
    response = urllib.request.urlopen(request, timeout=15)
except urllib.error.HTTPError as error:
    response = error
with response:
    print(json.dumps({'status': response.status,
        'headers': {key.lower(): value for key, value in response.headers.items()},
        'body': base64.b64encode(response.read()).decode()}))
"""
    command = "python3 -c " + shlex.quote(client) + " " + shlex.quote(json.dumps(config))
    response = json.loads(node.succeed(command))
    response["body"] = base64.b64decode(response["body"])
    return response


def definition(source, methods=None):
    return {
        "version": 1,
        "methods": methods or ["POST"],
        "source": source,
        "export": "handler",
        "limits": {"timeout_seconds": 1, "heap_mb": 32},
    }


def assert_status(response, expected):
    assert response["status"] == expected, response
    return response


def committed_leader(candidates, management="/api/http-functions/v1/definitions"):
    deadline = time.monotonic() + 45
    while time.monotonic() < deadline:
        for node in candidates:
            try:
                status = http(node, "GET", "/raft/status", port=4000)
                if json.loads(status["body"])["role"] != "Leader":
                    continue
                # A reported role is insufficient: fence admission with a committed read.
                if http(node, "GET", management)["status"] == 200:
                    return node
            except Exception:
                pass
        time.sleep(0.5)
    raise AssertionError("No leader completed a registry read barrier")
