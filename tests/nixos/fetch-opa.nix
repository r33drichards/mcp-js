{ pkgs, mcp-js, ... }:

let
  # ── Rego policy ──────────────────────────────────────────────────────
  #
  # Allow only GET requests to localhost:8080 under /allowed/.
  # Everything else is denied.

  regoPolicy = pkgs.writeText "policy.rego" ''
    package mcp.fetch

    default allow = false

    allow if {
        input.method == "GET"
        input.url_parsed.host == "localhost"
        input.url_parsed.port == 8080
        startswith(input.url_parsed.path, "/allowed/")
    }
  '';

  # ── Static JSON served by nginx ──────────────────────────────────────

  staticFiles = pkgs.runCommand "test-static" {} ''
    mkdir -p $out/allowed $out/denied
    echo '{"message": "hello from allowed endpoint"}' > $out/allowed/data
    echo '{"secret": "you should not see this"}' > $out/denied/secret
  '';
in

{
  name = "mcp-js-fetch-opa";

  nodes = {
    machine = { ... }: {
      imports = [ ../../nix/module.nix ];

      # ── mcp-js server (stateless, with OPA) ──────────────────────────
      services.mcp-js = {
        enable = true;
        package = mcp-js;
        nodeId = "test";
        stateless = true;
        httpPort = 3000;
        opaUrl = "http://localhost:8181";
        opaFetchPolicy = "mcp/fetch";
      };

      # ── OPA server ───────────────────────────────────────────────────
      systemd.services.opa = {
        description = "Open Policy Agent";
        after = [ "network.target" ];
        wantedBy = [ "multi-user.target" ];
        serviceConfig = {
          ExecStart = "${pkgs.open-policy-agent}/bin/opa run --server --addr :8181 ${regoPolicy}";
          Restart = "on-failure";
          DynamicUser = true;
        };
      };

      # ── nginx target server (port 8080) ──────────────────────────────
      services.nginx = {
        enable = true;
        virtualHosts.target = {
          listen = [{ addr = "127.0.0.1"; port = 8080; }];
          root = "${staticFiles}";
          extraConfig = ''
            default_type application/json;
          '';
        };
      };

      networking.firewall.allowedTCPPorts = [ 3000 8080 8181 ];
    };
  };

  testScript = ''
    import json
    import shlex

    machine.start()
    machine.wait_for_unit("opa.service")
    machine.wait_for_unit("nginx.service")
    machine.wait_for_unit("mcp-js.service")

    # Wait for mcp-js HTTP server to be ready
    machine.wait_for_open_port(3000)
    machine.wait_for_open_port(8080)
    machine.wait_for_open_port(8181)

    import time

    def exec_js(code):
        """Execute JS code via mcp-js async /api/exec endpoint and return parsed response.

        Submits the code (returns 202 + execution_id), then polls for completion.
        Returns a dict with 'output' key for backward-compat with assertions.
        """
        body = json.dumps({"code": code})
        raw = machine.succeed(
            "curl -s -X POST http://localhost:3000/api/exec "
            "-H 'Content-Type: application/json' "
            "-d " + shlex.quote(body)
        )
        submit_resp = json.loads(raw)
        exec_id = submit_resp["execution_id"]

        # Poll until execution completes (up to 30s)
        for _ in range(60):
            status_raw = machine.succeed(
                "curl -s http://localhost:3000/api/executions/" + exec_id
            )
            status_resp = json.loads(status_raw)
            status = status_resp.get("status", "")
            if status in ("Completed", "completed"):
                return {"output": status_resp.get("result", "")}
            if status in ("Failed", "failed", "TimedOut", "Cancelled"):
                return {"output": "Error: " + status_resp.get("error", status)}
            time.sleep(0.5)

        raise Exception("Execution " + exec_id + " did not complete within 30s")

    # ── Test 1: Allowed fetch (GET to /allowed/) ────────────────────────

    with subtest("should allow GET fetch to permitted path"):
        result = exec_js("""
            (async () => {
                const resp = await fetch("http://localhost:8080/allowed/data");
                return JSON.stringify(await resp.json());
            })()
        """)
        print("Allowed fetch result: " + str(result))
        assert "Error" not in result["output"], "Expected success, got: " + str(result)
        body = json.loads(result["output"])
        assert body["message"] == "hello from allowed endpoint", "Unexpected body: " + str(body)

    # ── Test 2: Denied fetch (wrong path prefix) ────────────────────────

    with subtest("should deny fetch to non-allowed path"):
        result = exec_js("""
            (async () => {
                try { await fetch("http://localhost:8080/denied/secret"); return "no error"; }
                catch(e) { return e.message; }
            })()
        """)
        print("Denied-by-path result: " + str(result))
        assert "denied by policy" in result["output"], \
            "Expected denied by policy error, got: " + str(result)

    # ── Test 3: Denied fetch (wrong method) ─────────────────────────────

    with subtest("should deny POST fetch even to allowed path"):
        result = exec_js("""
            (async () => {
                try { await fetch("http://localhost:8080/allowed/data", {method: "POST"}); return "no error"; }
                catch(e) { return e.message; }
            })()
        """)
        print("Denied-by-method result: " + str(result))
        assert "denied by policy" in result["output"], \
            "Expected denied by policy error, got: " + str(result)

    # ── Test 4: Denied fetch (wrong host) ───────────────────────────────

    with subtest("should deny fetch to non-allowed host"):
        result = exec_js("""
            (async () => {
                try { await fetch("http://example.com/allowed/data"); return "no error"; }
                catch(e) { return e.message; }
            })()
        """)
        print("Denied-by-host result: " + str(result))
        # Could be "denied by policy" or a connection error — either is acceptable.
        assert "no error" not in result["output"], \
            "Expected an error for non-allowed host, got: " + str(result)

    # ── Test 5: Verify fetch is available (typeof check) ────────────────

    with subtest("should have fetch available when OPA is configured"):
        result = exec_js("typeof fetch")
        print("typeof fetch: " + str(result))
        assert result["output"] == "function", "Expected function, got: " + str(result)

    # ── Test 6: Fetch with async/await syntax ─────────────────────────

    with subtest("should work with async/await syntax"):
        result = exec_js("""
            (async () => {
                const resp = await fetch("http://localhost:8080/allowed/data");
                return JSON.stringify(await resp.json());
            })()
        """)
        print("Async/await fetch result: " + str(result))
        body = json.loads(result["output"])
        assert body["message"] == "hello from allowed endpoint", \
            "Unexpected async/await body: " + str(result)

    # ── Test 7: Fetch with Promise .then() callback syntax ────────────

    with subtest("should work with Promise.then callback syntax"):
        result = exec_js("""
            fetch("http://localhost:8080/allowed/data")
                .then(function(resp) { return resp.json(); })
                .then(function(data) { return JSON.stringify(data); })
        """)
        print("Promise.then fetch result: " + str(result))
        body = json.loads(result["output"])
        assert body["message"] == "hello from allowed endpoint", \
            "Unexpected Promise.then body: " + str(result)

    # ── Test 8: Nested async fetch calls ──────────────────────────────

    with subtest("should work with nested async fetch calls"):
        result = exec_js("""
            (async () => {
                const first = await fetch("http://localhost:8080/allowed/data");
                const body = await first.json();
                const second = await fetch("http://localhost:8080/allowed/data");
                const body2 = await second.json();
                return JSON.stringify({
                    first: body.message,
                    second: body2.message,
                    combined: body.message + " + " + body2.message
                });
            })()
        """)
        print("Nested async fetch result: " + str(result))
        body = json.loads(result["output"])
        assert body["first"] == "hello from allowed endpoint", \
            "Unexpected first fetch: " + str(body)
        assert body["second"] == "hello from allowed endpoint", \
            "Unexpected second fetch: " + str(body)
        assert body["combined"] == "hello from allowed endpoint + hello from allowed endpoint", \
            "Unexpected combined: " + str(body)
  '';
}
