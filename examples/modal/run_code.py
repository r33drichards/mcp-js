"""Submit JavaScript to a deployed Modal interpreter."""

import argparse
import json
import os
import time
from functools import partial
from urllib.error import URLError
from urllib.request import Request, urlopen


def request(base_url, token, path, body=None):
    headers = {"Authorization": f"Bearer {token}"}
    data = None
    if body is not None:
        headers["Content-Type"] = "application/json"
        data = json.dumps(body).encode()
    with urlopen(Request(base_url.rstrip("/") + path, data, headers), timeout=60) as response:
        return json.load(response)


def run_js(call, code, *, session="tutorial"):
    execution = call(
        "/api/exec",
        {"code": code, "session": session, "execution_timeout_secs": 5},
    )
    path = f"/api/executions/{execution['execution_id']}"
    deadline = time.monotonic() + 90
    while time.monotonic() < deadline:
        result = call(path)
        status = result["status"].lower()
        if status in {"completed", "failed", "timed_out", "cancelled"}:
            output = call(f"{path}/output")["data"]
            if output:
                print(output.rstrip())
            if status != "completed" or result.get("error"):
                raise RuntimeError(result.get("error") or status)
            return
        time.sleep(0.25)
    raise TimeoutError("The execution did not finish before the client deadline")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("code", help="JavaScript source")
    parser.add_argument("--session", default="tutorial", help="Session name")
    args = parser.parse_args()

    url = os.environ.get("MCP_JS_URL")
    token = os.environ.get("MCP_JS_TOKEN")
    if not url or not token:
        parser.error("Set MCP_JS_URL to the deployed URL and MCP_JS_TOKEN to wk-ID.ws-SECRET.")

    call = partial(request, url, token)
    try:
        run_js(call, args.code, session=args.session)
    except (RuntimeError, URLError, TimeoutError) as error:
        parser.exit(1, f"{error}\n")


if __name__ == "__main__":
    main()
