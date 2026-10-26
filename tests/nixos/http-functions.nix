{ pkgs, mcp-js, ... }:
let
  fixture = pkgs.writeText "http-functions-fixture.py"
    (builtins.readFile ./http-functions-fixture.py);
  fetchPolicy = pkgs.writeText "http-functions-fetch.rego" ''
    package mcp.fetch
    default allow = false
    allow if {
      input.method == "GET"
      input.url_parsed.host == "127.0.0.1"
      input.url_parsed.port == 8081
      input.url_parsed.path == "/allowed"
    }
  '';
  workerConfig = pkgs.writeText "http-functions-scheduled-worker.json" (builtins.toJSON {
    namespace = "nixos-test";
    worker_id = "scheduled";
    data_dir = "/var/lib/mcp-js-scheduled/worker";
    bearer_env = "NIXOS_TEST_WORKER_TOKEN";
    admission_bearer_env = "NIXOS_TEST_ADMISSION_TOKEN";
    admission_urls = [ "http://127.0.0.1:8081" ];
  });
  auxiliaryService = name: port: {
    wantedBy = [ "multi-user.target" ];
    serviceConfig = {
      ExecStart = "${mcp-js}/bin/server --heap-store none --http-port ${toString port} --session-db-path /var/lib/${name}/sessions";
      StateDirectory = name;
      WorkingDirectory = "/var/lib/${name}";
      DynamicUser = true;
      Restart = "on-failure";
    };
  };
in {
  name = "mcp-js-http-functions";
  globalTimeout = 300;
  nodes.machine = { ... }: {
    imports = [ ../../nix/module.nix ];
    virtualisation.memorySize = 1536;
    environment.systemPackages = [ pkgs.python3 ];
    services.mcp-js = {
      enable = true;
      package = mcp-js;
      settings = {
        http_port = 3000;
        heap_store = "none";
        heap_memory_max = 64;
        execution_timeout = 3;
        session_db_path = "/var/lib/mcp-js/sessions";
        jwks_url = "http://127.0.0.1:8081/jwks.json";
        policies.fetch.policies = [{ url = "file://${fetchPolicy}"; }];
        fetch_headers = [{
          host = "127.0.0.1";
          headers."X-Upstream-Token" = "server-injected-test-value";
        }];
      };
    };
    systemd.services.mcp-js = {
      after = [ "http-functions-fixture.service" ];
      requires = [ "http-functions-fixture.service" ];
      environment.MCP_V8_HTTP_FUNCTIONS_DIR = "/var/lib/mcp-js/http-functions";
    };
    systemd.services.http-functions-fixture = {
      wantedBy = [ "multi-user.target" ];
      serviceConfig = {
        ExecStart = "${pkgs.python3}/bin/python3 ${fixture}";
        ExecStartPost = "${pkgs.runtimeShell} -c 'until ${pkgs.curl}/bin/curl -sf http://127.0.0.1:8081/jwks.json; do sleep 0.1; done'";
        DynamicUser = true;
      };
    };
    systemd.services.mcp-js-disabled = auxiliaryService "mcp-js-disabled" 3001;
    systemd.services.mcp-js-scheduled = (auxiliaryService "mcp-js-scheduled" 3002) // {
      environment = {
        MCP_SCHEDULED_WORKER_CONFIG = toString workerConfig;
        MCP_V8_HTTP_FUNCTIONS_DIR = "/var/lib/mcp-js-scheduled/http-functions";
        NIXOS_TEST_WORKER_TOKEN = "public-worker-test-token";
        NIXOS_TEST_ADMISSION_TOKEN = "public-admission-test-token";
      };
    };
  };
  testScript = builtins.readFile ./http-functions-common.py
    + builtins.readFile ./http-functions.py;
}
