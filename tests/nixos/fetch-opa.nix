{ pkgs, mcp-js, ... }:

let
  # ── Rego policy ──────────────────────────────────────────────────────
  #
  # Allow only GET requests to localhost:8080 under /allowed/.
  # Everything else is denied.

  regoPolicy = pkgs.writeText "policy.rego" ''
    package mcp.fetch

    default allow = false

    allow {
        input.method == "GET"
        input.url_parsed.host == "localhost"
        input.url_parsed.port == 8080
        startswith(input.url_parsed.path, "/allowed/")
    }
  '';

  # ── Static JSON served by nginx ──────────────────────────────────────

  allowedResponse = pkgs.writeText "data.json" ''
    {"message": "hello from allowed endpoint"}
  '';

  deniedResponse = pkgs.writeText "secret.json" ''
    {"secret": "you should not see this"}
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
          locations."/allowed/data" = {
            alias = "${allowedResponse}";
            extraConfig = ''
              default_type application/json;
            '';
          };
          locations."/denied/secret" = {
            alias = "${deniedResponse}";
            extraConfig = ''
              default_type application/json;
            '';
          };
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

    def exec_js(code):
        """Execute JS code via mcp-js /api/exec endpoint and return parsed response."""
        body = json.dumps({"code": code})
        raw = machine.succeed(
            "curl -sf -X POST http://localhost:3000/api/exec "
            "-H 'Content-Type: application/json' "
            "-d " + shlex.quote(body)
        )
        return json.loads(raw)

    # ── Test 1: Allowed fetch (GET to /allowed/) ────────────────────────

    with subtest("should allow GET fetch to permitted path"):
        result = exec_js("JSON.stringify(fetch(\"http://localhost:8080/allowed/data\").json())")
        print("Allowed fetch result: " + str(result))
        assert "Error" not in result["output"], "Expected success, got: " + str(result)
        body = json.loads(result["output"])
        assert body["message"] == "hello from allowed endpoint", "Unexpected body: " + str(body)

    # ── Test 2: Denied fetch (wrong path prefix) ────────────────────────

    with subtest("should deny fetch to non-allowed path"):
        result = exec_js("try { fetch(\"http://localhost:8080/denied/secret\"); \"no error\" } catch(e) { e.message }")
        print("Denied-by-path result: " + str(result))
        assert "denied by policy" in result["output"], \
            "Expected denied by policy error, got: " + str(result)

    # ── Test 3: Denied fetch (wrong method) ─────────────────────────────

    with subtest("should deny POST fetch even to allowed path"):
        result = exec_js("try { fetch(\"http://localhost:8080/allowed/data\", {method: \"POST\"}); \"no error\" } catch(e) { e.message }")
        print("Denied-by-method result: " + str(result))
        assert "denied by policy" in result["output"], \
            "Expected denied by policy error, got: " + str(result)

    # ── Test 4: Denied fetch (wrong host) ───────────────────────────────

    with subtest("should deny fetch to non-allowed host"):
        result = exec_js("try { fetch(\"http://example.com/allowed/data\"); \"no error\" } catch(e) { e.message }")
        print("Denied-by-host result: " + str(result))
        # Could be "denied by policy" or a connection error — either is acceptable.
        assert "no error" not in result["output"], \
            "Expected an error for non-allowed host, got: " + str(result)

    # ── Test 5: Verify fetch is available (typeof check) ────────────────

    with subtest("should have fetch available when OPA is configured"):
        result = exec_js("typeof fetch")
        print("typeof fetch: " + str(result))
        assert result["output"] == "function", "Expected function, got: " + str(result)
  '';
}
