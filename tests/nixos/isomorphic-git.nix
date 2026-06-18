{ pkgs, mcp-js, ... }:

# End-to-end proof that the sandbox `fs` object is a working Node-style
# filesystem: a real `isomorphic-git` (vendored, offline) drives
# git.init → writeFile → git.add → git.commit → git.push against an mcp-v8
# server, and we verify the commit lands in a bare repo on a separate
# git-over-HTTP server.
#
# The runtime never reaches the public internet (NixOS VMs are offline):
#   * isomorphic-git is a self-contained ESM bundle served over localhost and
#     imported as a URL module (gated by the modules policy);
#   * git.push talks to a local git-http-backend (gated by the fetch policy).

let
  # ── Vendored, self-contained isomorphic-git bundle (git + http/web) ──────
  bundleDir = pkgs.runCommand "iso-git-bundle" { } ''
    mkdir -p "$out"
    cp ${./vendor/isomorphic-git-bundle.mjs} "$out/isomorphic-git-bundle.mjs"
  '';

  # ── Policies ─────────────────────────────────────────────────────────────
  # fs: the agent works under /tmp/work/. modules: allow the localhost URL
  # import. fetch: allow git pushes to the localhost git server.
  fsPolicy = pkgs.writeText "fs.rego" ''
    package mcp.filesystem
    default allow = false
    allow if { startswith(input.path, "/tmp/work/") }
    allow if {
        startswith(input.path, "/tmp/work/")
        startswith(input.destination, "/tmp/work/")
    }
  '';
  modPolicy = pkgs.writeText "modules.rego" ''
    package mcp.modules
    default allow = false
    allow if {
        input.specifier_type == "url"
        input.url_parsed.host == "127.0.0.1"
    }
  '';
  fetchPolicy = pkgs.writeText "fetch.rego" ''
    package mcp.fetch
    default allow = false
    allow if { input.url_parsed.host == "127.0.0.1" }
  '';

  # ── Minimal git smart-HTTP server (git-http-backend CGI gateway) ─────────
  gitHttp = pkgs.writeText "git-http.py" ''
    import os, subprocess
    from http.server import BaseHTTPRequestHandler, HTTPServer
    BACKEND = os.environ["GIT_HTTP_BACKEND"]
    ROOT = os.environ["GIT_PROJECT_ROOT"]
    PORT = int(os.environ.get("GIT_HTTP_PORT", "8732"))

    class H(BaseHTTPRequestHandler):
        def _run(self, body=b""):
            u = self.path.split("?", 1)
            qs = u[1] if len(u) > 1 else ""
            env = dict(os.environ)
            env.update(dict(
                GIT_PROJECT_ROOT=ROOT, GIT_HTTP_EXPORT_ALL="1",
                PATH_INFO=u[0], QUERY_STRING=qs, REQUEST_METHOD=self.command,
                CONTENT_TYPE=self.headers.get("Content-Type", ""),
                CONTENT_LENGTH=str(len(body)),
            ))
            p = subprocess.run([BACKEND], input=body, env=env, capture_output=True)
            hdr, _, out = p.stdout.partition(b"\r\n\r\n")
            status = 200
            lines = []
            for ln in hdr.split(b"\r\n"):
                if ln.lower().startswith(b"status:"):
                    status = int(ln.split(b" ")[1])
                elif b":" in ln:
                    lines.append(ln)
            self.send_response(status)
            for ln in lines:
                k, _, v = ln.partition(b":")
                self.send_header(k.decode().strip(), v.decode().strip())
            self.end_headers()
            self.wfile.write(out)
        def log_message(self, *a):
            pass
        def do_GET(self):
            self._run()
        def do_POST(self):
            n = int(self.headers.get("Content-Length", "0"))
            self._run(self.rfile.read(n))

    HTTPServer(("127.0.0.1", PORT), H).serve_forever()
  '';

  gitRoot = "/var/lib/gitremote";
in
{
  name = "mcp-js-isomorphic-git";

  nodes = {
    machine = { ... }: {
      imports = [ ../../nix/module.nix ];

      # mcp-v8: stateless, fs + modules + fetch policies, external modules on.
      services.mcp-js = {
        enable = true;
        package = mcp-js;
        nodeId = "test";
        stateless = true;
        httpPort = 3000;
        allowExternalModules = true;
        policiesJson = builtins.toJSON {
          filesystem.policies = [{ url = "file://${fsPolicy}"; }];
          modules.policies = [{ url = "file://${modPolicy}"; }];
          fetch.policies = [{ url = "file://${fetchPolicy}"; }];
        };
      };
      # isomorphic-git + a git checkout need more than the 8 MB default heap.
      systemd.services.mcp-js.environment.MCP_V8_HEAP_MEMORY_MAX = "512";

      # Static server for the isomorphic-git bundle (imported as a URL module).
      systemd.services.iso-bundle = {
        description = "isomorphic-git bundle static server";
        wantedBy = [ "multi-user.target" ];
        before = [ "mcp-js.service" ];
        serviceConfig = {
          ExecStart = "${pkgs.python3}/bin/python3 -m http.server 8731 --bind 127.0.0.1 --directory ${bundleDir}";
          Restart = "on-failure";
          DynamicUser = true;
        };
      };

      # git smart-HTTP server with a push-enabled bare repo.
      systemd.services.git-http = {
        description = "git smart-HTTP server (push target)";
        wantedBy = [ "multi-user.target" ];
        path = [ pkgs.git ];
        preStart = ''
          if [ ! -d ${gitRoot}/repo.git ]; then
            mkdir -p ${gitRoot}
            ${pkgs.git}/bin/git init --bare -b main ${gitRoot}/repo.git
            ${pkgs.git}/bin/git -C ${gitRoot}/repo.git config http.receivepack true
          fi
        '';
        serviceConfig = {
          Environment = [
            "GIT_HTTP_BACKEND=${pkgs.git}/libexec/git-core/git-http-backend"
            "GIT_PROJECT_ROOT=${gitRoot}"
            "GIT_HTTP_PORT=8732"
          ];
          ExecStart = "${pkgs.python3}/bin/python3 ${gitHttp}";
          Restart = "on-failure";
        };
      };

      networking.firewall.allowedTCPPorts = [ 3000 ];

      # git on the machine PATH so the test script can inspect the bare remote.
      environment.systemPackages = [ pkgs.git ];

      # isomorphic-git runs with a 512 MB V8 heap; give the VM headroom on top
      # of that plus the OS and the two helper servers.
      virtualisation.memorySize = 2048;
    };
  };

  testScript = ''
    import json
    import shlex
    import time

    machine.start()
    machine.wait_for_unit("iso-bundle.service")
    machine.wait_for_unit("git-http.service")
    machine.wait_for_unit("mcp-js.service")
    machine.wait_for_open_port(8731)
    machine.wait_for_open_port(8732)
    machine.wait_for_open_port(3000)

    def exec_js(code):
        body = json.dumps({"code": code})
        raw = machine.succeed(
            "curl -s -X POST http://localhost:3000/api/exec "
            "-H 'Content-Type: application/json' "
            "-d " + shlex.quote(body)
        )
        exec_id = json.loads(raw)["execution_id"]
        for _ in range(120):
            status_raw = machine.succeed(
                "curl -s http://localhost:3000/api/executions/" + exec_id
            )
            resp = json.loads(status_raw)
            status = resp.get("status", "")
            if status.lower() == "completed":
                out = machine.succeed(
                    "curl -s http://localhost:3000/api/executions/" + exec_id + "/output"
                )
                return json.loads(out).get("data", "").strip()
            if status.lower() in ("failed", "timedout", "cancelled"):
                return "ERROR: " + str(resp.get("error", status))
            time.sleep(0.5)
        raise Exception("execution did not complete: " + exec_id)

    BUNDLE = "http://127.0.0.1:8731/isomorphic-git-bundle.mjs"
    REMOTE = "http://127.0.0.1:8732/repo.git"

    with subtest("init, write, add and commit a repo via isomorphic-git"):
        out = exec_js(f"""
            import {{ git }} from '{BUNDLE}';
            const dir = '/tmp/work/repo';
            await fs.mkdir(dir, {{ recursive: true }});
            await git.init({{ fs, dir, defaultBranch: 'main' }});
            await fs.writeFile(dir + '/README.md', '# hello from isomorphic-git');
            await git.add({{ fs, dir, filepath: 'README.md' }});
            const oid = await git.commit({{
                fs, dir, message: 'initial commit',
                author: {{ name: 'Test', email: 'test@example.com' }},
            }});
            const log = await git.log({{ fs, dir }});
            console.log(JSON.stringify({{ oid: oid.slice(0, 8), count: log.length, msg: log[0].commit.message.trim() }}));
        """)
        print("commit result: " + out)
        body = json.loads(out)
        assert body["count"] == 1, "expected exactly one commit, got: " + out
        assert body["msg"] == "initial commit", "unexpected commit message: " + out
        commit_oid = body["oid"]

    with subtest("push the commit to a git-over-HTTP remote"):
        out = exec_js(f"""
            import {{ git, http }} from '{BUNDLE}';
            const dir = '/tmp/work/repo';
            const res = await git.push({{
                fs, http, dir, url: '{REMOTE}', ref: 'main', remoteRef: 'main',
            }});
            console.log(JSON.stringify({{ ok: res.ok, errors: res.errors || null }}));
        """)
        print("push result: " + out)
        body = json.loads(out)
        assert body["ok"] == True, "push did not succeed: " + out
        assert body["errors"] is None, "push reported errors: " + out

    with subtest("the bare remote received the commit"):
        remote_log = machine.succeed("git -C " + shlex.quote("${gitRoot}/repo.git") + " log --oneline -1").strip()
        print("remote log: " + remote_log)
        assert commit_oid in remote_log, f"remote missing commit {commit_oid}: {remote_log}"
        assert "initial commit" in remote_log, "remote missing commit message: " + remote_log
  '';
}
