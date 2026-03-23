{ pkgs, mcp-js, ... }:

let
  # ── Rego policy ──────────────────────────────────────────────────────
  #
  # Deno.Command (command_output): allow "echo" and "cat".
  # child_process.exec (exec):    allow commands starting with "echo" or "cat".
  # Everything else is denied.

  regoPolicy = pkgs.writeText "subprocess-policy.rego" ''
    package mcp.subprocess

    default allow = false

    # ── Deno.Command (command_output) ─────────────────────────────────
    allowed_commands := {"echo", "cat"}

    allow if {
        input.operation == "command_output"
        allowed_commands[input.command]
    }

    # ── child_process.exec ────────────────────────────────────────────
    allowed_exec_patterns := {"echo", "cat"}

    allow if {
        input.operation == "exec"
        some pattern in allowed_exec_patterns
        startswith(input.args[1], pattern)
    }
  '';
in

{
  name = "mcp-js-exec-opa";

  nodes = {
    machine = { ... }: {
      imports = [ ../../nix/module.nix ];

      # ── mcp-js server (stateless, with subprocess policy) ───────────
      services.mcp-js = {
        enable = true;
        package = mcp-js;
        nodeId = "test";
        stateless = true;
        httpPort = 3000;
        policiesJson = builtins.toJSON {
          subprocess = {
            policies = [{
              url = "file://${regoPolicy}";
            }];
          };
        };
      };

      networking.firewall.allowedTCPPorts = [ 3000 ];
    };
  };

  testScript = ''
    import json
    import shlex
    import time

    machine.start()
    machine.wait_for_unit("mcp-js.service")
    machine.wait_for_open_port(3000)

    def exec_js(code):
        """Execute JS code via mcp-js async /api/exec endpoint and return parsed response.

        All code runs as ES modules, so results must be captured via console.log().
        Returns a dict with 'output' key containing console output.
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
                output_raw = machine.succeed(
                    "curl -s http://localhost:3000/api/executions/" + exec_id + "/output"
                )
                output_resp = json.loads(output_raw)
                console_data = output_resp.get("data", "").strip()
                return {"output": console_data}
            if status in ("Failed", "failed", "TimedOut", "Cancelled"):
                return {"output": "Error: " + status_resp.get("error", status)}
            time.sleep(0.5)

        raise Exception("Execution " + exec_id + " did not complete within 30s")

    # ── Test 1: Deno.Command – allowed command (echo) ──────────────────

    with subtest("Deno.Command should allow echo"):
        result = exec_js("""
            const cmd = new Deno.Command("echo", { args: ["hello", "world"] });
            const { code, stdout } = await cmd.output();
            console.log(new TextDecoder().decode(stdout).trim());
        """)
        print("Deno.Command echo result: " + str(result))
        assert result["output"] == "hello world", \
            "Expected 'hello world', got: " + str(result)

    # ── Test 2: Deno.Command – allowed command (cat) ───────────────────

    with subtest("Deno.Command should allow cat"):
        result = exec_js("""
            const cmd = new Deno.Command("cat", { args: ["/etc/hostname"] });
            const { code, stdout } = await cmd.output();
            const output = new TextDecoder().decode(stdout).trim();
            console.log(output.length > 0 ? "ok" : "empty");
        """)
        print("Deno.Command cat result: " + str(result))
        assert result["output"] == "ok", \
            "Expected 'ok', got: " + str(result)

    # ── Test 3: Deno.Command – denied command (ls) ─────────────────────

    with subtest("Deno.Command should deny ls"):
        result = exec_js("""
            try {
                const cmd = new Deno.Command("ls", { args: ["/tmp"] });
                await cmd.output();
                console.log("no error");
            } catch(e) { console.log(e.message); }
        """)
        print("Deno.Command deny result: " + str(result))
        assert "denied by policy" in result["output"], \
            "Expected denied by policy error, got: " + str(result)

    # ── Test 4: Deno.Command – denied command (rm) ─────────────────────

    with subtest("Deno.Command should deny rm"):
        result = exec_js("""
            try {
                const cmd = new Deno.Command("rm", { args: ["-rf", "/tmp/foo"] });
                await cmd.output();
                console.log("no error");
            } catch(e) { console.log(e.message); }
        """)
        print("Deno.Command deny rm result: " + str(result))
        assert "denied by policy" in result["output"], \
            "Expected denied by policy error, got: " + str(result)

    # ── Test 5: child_process.exec – allowed command ───────────────────

    with subtest("child_process.exec should allow echo"):
        result = exec_js("""
            const { stdout } = await child_process.exec("echo hello from exec");
            console.log(stdout.trim());
        """)
        print("child_process.exec echo result: " + str(result))
        assert result["output"] == "hello from exec", \
            "Expected 'hello from exec', got: " + str(result)

    # ── Test 6: child_process.exec – allowed cat command ───────────────

    with subtest("child_process.exec should allow cat"):
        result = exec_js("""
            const { stdout } = await child_process.exec("cat /etc/hostname");
            console.log(stdout.trim().length > 0 ? "ok" : "empty");
        """)
        print("child_process.exec cat result: " + str(result))
        assert result["output"] == "ok", \
            "Expected 'ok', got: " + str(result)

    # ── Test 7: child_process.exec – denied command ────────────────────

    with subtest("child_process.exec should deny ls"):
        result = exec_js("""
            try {
                await child_process.exec("ls -la /tmp");
                console.log("no error");
            } catch(e) { console.log(e.message); }
        """)
        print("child_process.exec deny result: " + str(result))
        assert "denied by policy" in result["output"], \
            "Expected denied by policy error, got: " + str(result)

    # ── Test 8: child_process.exec – denied dangerous command ──────────

    with subtest("child_process.exec should deny rm -rf"):
        result = exec_js("""
            try {
                await child_process.exec("rm -rf /tmp");
                console.log("no error");
            } catch(e) { console.log(e.message); }
        """)
        print("child_process.exec deny rm result: " + str(result))
        assert "denied by policy" in result["output"], \
            "Expected denied by policy error, got: " + str(result)

    # ── Test 9: Deno.Command – verify exit code ────────────────────────

    with subtest("Deno.Command should return correct exit code"):
        result = exec_js("""
            const cmd = new Deno.Command("echo", { args: [] });
            const { code, success } = await cmd.output();
            console.log(JSON.stringify({ code, success }));
        """)
        print("Deno.Command exit code result: " + str(result))
        body = json.loads(result["output"])
        assert body["code"] == 0, "Expected exit code 0, got: " + str(body)
        assert body["success"] == True, "Expected success=true, got: " + str(body)

    # ── Test 10: child_process.exec – with cwd option ──────────────────

    with subtest("child_process.exec should respect cwd option"):
        result = exec_js("""
            const { stdout } = await child_process.exec("echo from-cwd", { cwd: "/tmp" });
            console.log(stdout.trim());
        """)
        print("child_process.exec cwd result: " + str(result))
        assert result["output"] == "from-cwd", \
            "Expected 'from-cwd', got: " + str(result)

    # ── Test 11: Deno.Command and child_process are available ──────────

    with subtest("subprocess APIs should be available when policy is configured"):
        result = exec_js("""
            console.log(JSON.stringify({
                denoCommand: typeof Deno.Command,
                childProcess: typeof child_process.exec,
            }));
        """)
        print("API availability: " + str(result))
        body = json.loads(result["output"])
        assert body["denoCommand"] == "function", \
            "Expected Deno.Command to be function, got: " + str(body)
        assert body["childProcess"] == "function", \
            "Expected child_process.exec to be function, got: " + str(body)

    # ── Test 12: Deno.Command with stdout and stderr ───────────────────

    with subtest("Deno.Command should capture stdout correctly"):
        result = exec_js("""
            const cmd = new Deno.Command("echo", { args: ["line1"] });
            const { stdout, stderr } = await cmd.output();
            const out = new TextDecoder().decode(stdout).trim();
            const err = new TextDecoder().decode(stderr);
            console.log(JSON.stringify({ out, err }));
        """)
        print("Deno.Command stdout/stderr result: " + str(result))
        body = json.loads(result["output"])
        assert body["out"] == "line1", "Expected 'line1', got: " + str(body)
        assert body["err"] == "", "Expected empty stderr, got: " + str(body)
  '';
}
