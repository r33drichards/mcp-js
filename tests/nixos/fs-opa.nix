{ pkgs, mcp-js, ... }:

let
  # ── Rego policy ──────────────────────────────────────────────────────
  #
  # Allow read/write/stat/exists/readdir operations only under /tmp/allowed/.
  # mkdir is allowed under /tmp/allowed/ (recursive or not).
  # rm, rename, copyFile, appendFile are allowed under /tmp/allowed/.
  # Everything else is denied.

  regoPolicy = pkgs.writeText "fs-policy.rego" ''
    package mcp.filesystem

    default allow = false

    allow if {
        startswith(input.path, "/tmp/allowed/")
    }

    # Also allow operations with a destination under /tmp/allowed/
    allow if {
        startswith(input.path, "/tmp/allowed/")
        startswith(input.destination, "/tmp/allowed/")
    }
  '';
in

{
  name = "mcp-js-fs-opa";

  nodes = {
    machine = { ... }: {
      imports = [ ../../nix/module.nix ];

      # ── mcp-js server (stateless, with filesystem policy) ────────────
      services.mcp-js = {
        enable = true;
        package = mcp-js;
        nodeId = "test";
        stateless = true;
        httpPort = 3000;
        policiesJson = builtins.toJSON {
          filesystem = {
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

    # Create the allowed directory
    machine.succeed("mkdir -p /tmp/allowed")

    def exec_js(code):
        """Execute JS code via mcp-js async /api/exec endpoint and return parsed response."""
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
                return {"output": status_resp.get("result", "")}
            if status in ("Failed", "failed", "TimedOut", "Cancelled"):
                return {"output": "Error: " + status_resp.get("error", status)}
            time.sleep(0.5)

        raise Exception("Execution " + exec_id + " did not complete within 30s")

    # ── Test 1: Write and read a file in allowed directory ────────────

    with subtest("should allow writeFile and readFile in /tmp/allowed/"):
        result = exec_js("""
            (async () => {
                await fs.writeFile("/tmp/allowed/test.txt", "hello world");
                const data = await fs.readFile("/tmp/allowed/test.txt");
                return data;
            })()
        """)
        print("Write+Read result: " + str(result))
        assert result["output"] == "hello world", "Expected 'hello world', got: " + str(result)

    # ── Test 2: Deny write to non-allowed directory ──────────────────

    with subtest("should deny writeFile outside /tmp/allowed/"):
        result = exec_js("""
            (async () => {
                try {
                    await fs.writeFile("/tmp/secret.txt", "bad data");
                    return "no error";
                } catch(e) { return e.message; }
            })()
        """)
        print("Denied write result: " + str(result))
        assert "denied by policy" in result["output"], \
            "Expected denied by policy error, got: " + str(result)

    # ── Test 3: Deny read from non-allowed directory ─────────────────

    with subtest("should deny readFile outside /tmp/allowed/"):
        result = exec_js("""
            (async () => {
                try {
                    const data = await fs.readFile("/etc/hostname");
                    return "no error: " + data;
                } catch(e) { return e.message; }
            })()
        """)
        print("Denied read result: " + str(result))
        assert "denied by policy" in result["output"], \
            "Expected denied by policy error, got: " + str(result)

    # ── Test 4: stat in allowed directory ─────────────────────────────

    with subtest("should allow stat in /tmp/allowed/"):
        result = exec_js("""
            (async () => {
                const info = await fs.stat("/tmp/allowed/test.txt");
                return JSON.stringify({isFile: info.isFile, size: info.size});
            })()
        """)
        print("Stat result: " + str(result))
        body = json.loads(result["output"])
        assert body["isFile"] == True, "Expected isFile=true, got: " + str(body)
        assert body["size"] == 11, "Expected size=11, got: " + str(body)

    # ── Test 5: exists in allowed directory ───────────────────────────

    with subtest("should allow exists in /tmp/allowed/"):
        result = exec_js("""
            (async () => {
                const e1 = await fs.exists("/tmp/allowed/test.txt");
                const e2 = await fs.exists("/tmp/allowed/nonexistent.txt");
                return JSON.stringify({exists: e1, notExists: e2});
            })()
        """)
        print("Exists result: " + str(result))
        body = json.loads(result["output"])
        assert body["exists"] == True, "Expected exists=true, got: " + str(body)
        assert body["notExists"] == False, "Expected notExists=false, got: " + str(body)

    # ── Test 6: mkdir and readdir ─────────────────────────────────────

    with subtest("should allow mkdir and readdir in /tmp/allowed/"):
        result = exec_js("""
            (async () => {
                await fs.mkdir("/tmp/allowed/subdir", {recursive: true});
                await fs.writeFile("/tmp/allowed/subdir/a.txt", "a");
                await fs.writeFile("/tmp/allowed/subdir/b.txt", "b");
                const entries = await fs.readdir("/tmp/allowed/subdir");
                entries.sort();
                return JSON.stringify(entries);
            })()
        """)
        print("mkdir+readdir result: " + str(result))
        entries = json.loads(result["output"])
        assert entries == ["a.txt", "b.txt"], "Expected ['a.txt', 'b.txt'], got: " + str(entries)

    # ── Test 7: appendFile ────────────────────────────────────────────

    with subtest("should allow appendFile in /tmp/allowed/"):
        result = exec_js("""
            (async () => {
                await fs.writeFile("/tmp/allowed/append.txt", "hello");
                await fs.appendFile("/tmp/allowed/append.txt", " world");
                return await fs.readFile("/tmp/allowed/append.txt");
            })()
        """)
        print("appendFile result: " + str(result))
        assert result["output"] == "hello world", "Expected 'hello world', got: " + str(result)

    # ── Test 8: rename ────────────────────────────────────────────────

    with subtest("should allow rename within /tmp/allowed/"):
        result = exec_js("""
            (async () => {
                await fs.writeFile("/tmp/allowed/old.txt", "renamed");
                await fs.rename("/tmp/allowed/old.txt", "/tmp/allowed/new.txt");
                return await fs.readFile("/tmp/allowed/new.txt");
            })()
        """)
        print("Rename result: " + str(result))
        assert result["output"] == "renamed", "Expected 'renamed', got: " + str(result)

    # ── Test 9: copyFile ──────────────────────────────────────────────

    with subtest("should allow copyFile within /tmp/allowed/"):
        result = exec_js("""
            (async () => {
                await fs.writeFile("/tmp/allowed/src.txt", "copied");
                await fs.copyFile("/tmp/allowed/src.txt", "/tmp/allowed/dst.txt");
                return await fs.readFile("/tmp/allowed/dst.txt");
            })()
        """)
        print("CopyFile result: " + str(result))
        assert result["output"] == "copied", "Expected 'copied', got: " + str(result)

    # ── Test 10: rm ───────────────────────────────────────────────────

    with subtest("should allow rm in /tmp/allowed/"):
        result = exec_js("""
            (async () => {
                await fs.writeFile("/tmp/allowed/del.txt", "delete me");
                await fs.rm("/tmp/allowed/del.txt");
                const exists = await fs.exists("/tmp/allowed/del.txt");
                return JSON.stringify({deleted: !exists});
            })()
        """)
        print("rm result: " + str(result))
        body = json.loads(result["output"])
        assert body["deleted"] == True, "Expected deleted=true, got: " + str(body)

    # ── Test 11: fs object is available ───────────────────────────────

    with subtest("should have fs available when filesystem policy is configured"):
        result = exec_js("typeof fs")
        print("typeof fs: " + str(result))
        assert result["output"] == "object", "Expected object, got: " + str(result)

    # ── Test 12: binary write/read with Uint8Array ────────────────────

    with subtest("should support binary write/read with Uint8Array"):
        result = exec_js("""
            (async () => {
                const data = new Uint8Array([72, 101, 108, 108, 111]);
                await fs.writeFile("/tmp/allowed/binary.bin", data);
                const read = await fs.readFile("/tmp/allowed/binary.bin", "buffer");
                return JSON.stringify(Array.from(read));
            })()
        """)
        print("Binary write/read result: " + str(result))
        arr = json.loads(result["output"])
        assert arr == [72, 101, 108, 108, 111], "Expected [72,101,108,108,111], got: " + str(arr)
  '';
}
