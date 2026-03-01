{ pkgs, mcp-js, ... }:

let
  # ── Rego policy ──────────────────────────────────────────────────────
  #
  # Allow filesystem operations only under /tmp/.
  # Deny everything else. Dual-path ops (rename, copyFile) require both
  # source and destination under /tmp/.

  fsPolicy = pkgs.writeText "fs-policy.rego" ''
    package mcp.fs

    default allow = false

    allow if {
        input.operation == "readFile"
        path_allowed
        not path_denied
    }
    allow if {
        input.operation == "readdir"
        path_allowed
        not path_denied
    }
    allow if {
        input.operation == "stat"
        path_allowed
        not path_denied
    }
    allow if {
        input.operation == "exists"
        path_allowed
        not path_denied
    }
    allow if {
        input.operation == "writeFile"
        path_allowed
        not path_denied
    }
    allow if {
        input.operation == "appendFile"
        path_allowed
        not path_denied
    }
    allow if {
        input.operation == "mkdir"
        path_allowed
        not path_denied
    }
    allow if {
        input.operation == "rm"
        path_allowed
        not path_denied
    }
    allow if {
        input.operation == "rename"
        path_allowed
        destination_allowed
        not path_denied
        not destination_denied
    }
    allow if {
        input.operation == "copyFile"
        path_allowed
        destination_allowed
        not path_denied
        not destination_denied
    }

    allowed_prefixes := { "/tmp/" }

    path_allowed if {
        some prefix in allowed_prefixes
        startswith(input.path, prefix)
    }
    path_allowed if {
        input.path == "/tmp"
    }

    destination_allowed if {
        some prefix in allowed_prefixes
        startswith(input.destination, prefix)
    }

    denied_prefixes := set()

    path_denied if {
        some prefix in denied_prefixes
        startswith(input.path, prefix)
    }
    destination_denied if {
        some prefix in denied_prefixes
        startswith(input.destination, prefix)
    }
  '';

  # Minimal fetch policy (required since --opa-url enables fetch too)
  fetchPolicy = pkgs.writeText "fetch-policy.rego" ''
    package mcp.fetch
    default allow = false
  '';
in

{
  name = "mcp-js-fs-opa";

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
      };

      # ── OPA server ───────────────────────────────────────────────────
      systemd.services.opa = {
        description = "Open Policy Agent";
        after = [ "network.target" ];
        wantedBy = [ "multi-user.target" ];
        serviceConfig = {
          ExecStart = "${pkgs.open-policy-agent}/bin/opa run --server --addr :8181 ${fsPolicy} ${fetchPolicy}";
          Restart = "on-failure";
          DynamicUser = true;
        };
      };

      networking.firewall.allowedTCPPorts = [ 3000 8181 ];
    };
  };

  testScript = ''
    import json
    import shlex
    import time

    machine.start()
    machine.wait_for_unit("opa.service")
    machine.wait_for_unit("mcp-js.service")
    machine.wait_for_open_port(3000)
    machine.wait_for_open_port(8181)

    def exec_js(code):
        """Execute JS code via mcp-js async /api/exec endpoint.

        Returns a dict with keys: status, output (result or error message).
        """
        body = json.dumps({"code": code})
        raw = machine.succeed(
            "curl -s -X POST http://localhost:3000/api/exec "
            "-H 'Content-Type: application/json' "
            "-d " + shlex.quote(body)
        )
        submit_resp = json.loads(raw)
        exec_id = submit_resp["execution_id"]

        for _ in range(60):
            status_raw = machine.succeed(
                "curl -s http://localhost:3000/api/executions/" + exec_id
            )
            status_resp = json.loads(status_raw)
            status = status_resp.get("status", "")
            if status in ("Completed", "completed"):
                return {
                    "status": "completed",
                    "output": status_resp.get("result", ""),
                    "exec_id": exec_id,
                }
            if status in ("Failed", "failed", "TimedOut", "Cancelled",
                          "timed_out", "cancelled"):
                return {
                    "status": "failed",
                    "output": "Error: " + status_resp.get("error", status),
                    "exec_id": exec_id,
                }
            time.sleep(0.5)

        raise Exception("Execution " + exec_id + " did not complete within 30s")

    def get_console(exec_id):
        """Fetch console output for an execution."""
        raw = machine.succeed(
            "curl -s http://localhost:3000/api/executions/" + exec_id + "/output"
        )
        resp = json.loads(raw)
        lines = resp.get("lines", [])
        return "\n".join(line.get("text", "") for line in lines)

    # ════════════════════════════════════════════════════════════════════
    # Allowed operations (paths under /tmp/)
    # ════════════════════════════════════════════════════════════════════

    with subtest("should write and read a file under /tmp"):
        result = exec_js("""
            (async () => {
                await fs.writeFile("/tmp/test.txt", "hello world");
                const data = await fs.readFile("/tmp/test.txt");
                console.log(data);
            })()
        """)
        assert result["status"] == "completed", \
            "Expected completed, got: " + str(result)
        console = get_console(result["exec_id"])
        assert "hello world" in console, \
            "Expected 'hello world' in console output, got: " + console

    with subtest("should mkdir and stat a directory under /tmp"):
        result = exec_js("""
            (async () => {
                await fs.mkdir("/tmp/testdir", { recursive: true });
                const info = await fs.stat("/tmp/testdir");
                console.log(JSON.stringify(info));
            })()
        """)
        assert result["status"] == "completed", \
            "Expected completed, got: " + str(result)
        console = get_console(result["exec_id"])
        info = json.loads(console.strip())
        assert info["isDirectory"] == True, \
            "Expected isDirectory=true, got: " + console

    with subtest("should readdir /tmp"):
        result = exec_js("""
            (async () => {
                const entries = await fs.readdir("/tmp");
                console.log(JSON.stringify(Array.isArray(entries)));
                console.log(JSON.stringify(entries.length > 0));
            })()
        """)
        assert result["status"] == "completed", \
            "Expected completed, got: " + str(result)
        console = get_console(result["exec_id"])
        assert "true" in console, \
            "Expected readdir to return a non-empty array, got: " + console

    with subtest("should appendFile and verify content"):
        result = exec_js("""
            (async () => {
                await fs.writeFile("/tmp/append.txt", "hello");
                await fs.appendFile("/tmp/append.txt", " world");
                const data = await fs.readFile("/tmp/append.txt");
                console.log(data);
            })()
        """)
        assert result["status"] == "completed", \
            "Expected completed, got: " + str(result)
        console = get_console(result["exec_id"])
        assert "hello world" in console, \
            "Expected 'hello world', got: " + console

    with subtest("should check exists (true and false)"):
        result = exec_js("""
            (async () => {
                await fs.writeFile("/tmp/exists-test.txt", "x");
                const e1 = await fs.exists("/tmp/exists-test.txt");
                const e2 = await fs.exists("/tmp/nonexistent-xyz.txt");
                console.log(JSON.stringify({ exists: e1, notExists: e2 }));
            })()
        """)
        assert result["status"] == "completed", \
            "Expected completed, got: " + str(result)
        console = get_console(result["exec_id"])
        data = json.loads(console.strip())
        assert data["exists"] == True, "Expected exists=true, got: " + console
        assert data["notExists"] == False, "Expected notExists=false, got: " + console

    with subtest("should rename within /tmp"):
        result = exec_js("""
            (async () => {
                await fs.writeFile("/tmp/rename-src.txt", "rename-data");
                await fs.rename("/tmp/rename-src.txt", "/tmp/rename-dst.txt");
                const data = await fs.readFile("/tmp/rename-dst.txt");
                const srcGone = !(await fs.exists("/tmp/rename-src.txt"));
                console.log(JSON.stringify({ data, srcGone }));
            })()
        """)
        assert result["status"] == "completed", \
            "Expected completed, got: " + str(result)
        console = get_console(result["exec_id"])
        data = json.loads(console.strip())
        assert data["data"] == "rename-data", \
            "Expected 'rename-data', got: " + console
        assert data["srcGone"] == True, \
            "Expected source to be gone, got: " + console

    with subtest("should copyFile within /tmp"):
        result = exec_js("""
            (async () => {
                await fs.writeFile("/tmp/copy-src.txt", "copy-data");
                await fs.copyFile("/tmp/copy-src.txt", "/tmp/copy-dst.txt");
                const data = await fs.readFile("/tmp/copy-dst.txt");
                const srcExists = await fs.exists("/tmp/copy-src.txt");
                console.log(JSON.stringify({ data, srcExists }));
            })()
        """)
        assert result["status"] == "completed", \
            "Expected completed, got: " + str(result)
        console = get_console(result["exec_id"])
        data = json.loads(console.strip())
        assert data["data"] == "copy-data", \
            "Expected 'copy-data', got: " + console
        assert data["srcExists"] == True, \
            "Expected source to still exist, got: " + console

    with subtest("should rm a file under /tmp"):
        result = exec_js("""
            (async () => {
                await fs.writeFile("/tmp/rm-test.txt", "delete me");
                const before = await fs.exists("/tmp/rm-test.txt");
                await fs.rm("/tmp/rm-test.txt");
                const after = await fs.exists("/tmp/rm-test.txt");
                console.log(JSON.stringify({ before, after }));
            })()
        """)
        assert result["status"] == "completed", \
            "Expected completed, got: " + str(result)
        console = get_console(result["exec_id"])
        data = json.loads(console.strip())
        assert data["before"] == True, "Expected before=true, got: " + console
        assert data["after"] == False, "Expected after=false, got: " + console

    with subtest("should write and read binary data (Uint8Array)"):
        result = exec_js("""
            (async () => {
                const bytes = new Uint8Array([0, 1, 2, 255, 128, 64]);
                await fs.writeFile("/tmp/binary.bin", bytes);
                const result = await fs.readFile("/tmp/binary.bin", "buffer");
                console.log(JSON.stringify({
                    isUint8: result instanceof Uint8Array,
                    len: result.length,
                    vals: [result[0], result[1], result[2], result[3], result[4], result[5]]
                }));
            })()
        """)
        assert result["status"] == "completed", \
            "Expected completed, got: " + str(result)
        console = get_console(result["exec_id"])
        data = json.loads(console.strip())
        assert data["isUint8"] == True, "Expected Uint8Array, got: " + console
        assert data["len"] == 6, "Expected length 6, got: " + console
        assert data["vals"] == [0, 1, 2, 255, 128, 64], \
            "Expected [0,1,2,255,128,64], got: " + console

    # ════════════════════════════════════════════════════════════════════
    # Denied operations (paths outside /tmp/)
    # ════════════════════════════════════════════════════════════════════

    with subtest("should deny readFile outside /tmp"):
        result = exec_js("""
            (async () => { await fs.readFile("/etc/passwd"); })()
        """)
        assert result["status"] == "failed", \
            "Expected failed, got: " + str(result)
        assert "denied by policy" in result["output"], \
            "Expected 'denied by policy', got: " + result["output"]

    with subtest("should deny writeFile outside /tmp"):
        result = exec_js("""
            (async () => { await fs.writeFile("/etc/evil", "x"); })()
        """)
        assert result["status"] == "failed", \
            "Expected failed, got: " + str(result)
        assert "denied by policy" in result["output"], \
            "Expected 'denied by policy', got: " + result["output"]

    with subtest("should deny readdir outside /tmp"):
        result = exec_js("""
            (async () => { await fs.readdir("/home"); })()
        """)
        assert result["status"] == "failed", \
            "Expected failed, got: " + str(result)
        assert "denied by policy" in result["output"], \
            "Expected 'denied by policy', got: " + result["output"]

    with subtest("should deny stat outside /tmp"):
        result = exec_js("""
            (async () => { await fs.stat("/etc/hostname"); })()
        """)
        assert result["status"] == "failed", \
            "Expected failed, got: " + str(result)
        assert "denied by policy" in result["output"], \
            "Expected 'denied by policy', got: " + result["output"]

    with subtest("should deny mkdir outside /tmp"):
        result = exec_js("""
            (async () => { await fs.mkdir("/var/evildir"); })()
        """)
        assert result["status"] == "failed", \
            "Expected failed, got: " + str(result)
        assert "denied by policy" in result["output"], \
            "Expected 'denied by policy', got: " + result["output"]

    with subtest("should deny exists outside /tmp"):
        result = exec_js("""
            (async () => { await fs.exists("/root/.bashrc"); })()
        """)
        assert result["status"] == "failed", \
            "Expected failed, got: " + str(result)
        assert "denied by policy" in result["output"], \
            "Expected 'denied by policy', got: " + result["output"]

    with subtest("should deny rm outside /tmp"):
        result = exec_js("""
            (async () => { await fs.rm("/bin/sh"); })()
        """)
        assert result["status"] == "failed", \
            "Expected failed, got: " + str(result)
        assert "denied by policy" in result["output"], \
            "Expected 'denied by policy', got: " + result["output"]

    # ════════════════════════════════════════════════════════════════════
    # Dual-path operations boundary
    # ════════════════════════════════════════════════════════════════════

    with subtest("should deny rename when destination is outside /tmp"):
        result = exec_js("""
            (async () => {
                await fs.writeFile("/tmp/rename-escape.txt", "data");
                await fs.rename("/tmp/rename-escape.txt", "/etc/rename-escape.txt");
            })()
        """)
        assert result["status"] == "failed", \
            "Expected failed, got: " + str(result)
        assert "denied by policy" in result["output"], \
            "Expected 'denied by policy', got: " + result["output"]

    with subtest("should deny copyFile when destination is outside /tmp"):
        result = exec_js("""
            (async () => {
                await fs.writeFile("/tmp/copy-escape.txt", "data");
                await fs.copyFile("/tmp/copy-escape.txt", "/etc/copy-escape.txt");
            })()
        """)
        assert result["status"] == "failed", \
            "Expected failed, got: " + str(result)
        assert "denied by policy" in result["output"], \
            "Expected 'denied by policy', got: " + result["output"]

    with subtest("should deny rename when source is outside /tmp"):
        result = exec_js("""
            (async () => {
                await fs.rename("/etc/hostname", "/tmp/stolen-hostname");
            })()
        """)
        assert result["status"] == "failed", \
            "Expected failed, got: " + str(result)
        assert "denied by policy" in result["output"], \
            "Expected 'denied by policy', got: " + result["output"]

    # ════════════════════════════════════════════════════════════════════
    # Sanity
    # ════════════════════════════════════════════════════════════════════

    with subtest("should still execute plain JS when fs policy is active"):
        result = exec_js("1 + 2")
        assert result["status"] == "completed", \
            "Expected completed, got: " + str(result)
        assert result["output"] == "3", \
            "Expected '3', got: " + result["output"]
  '';
}
