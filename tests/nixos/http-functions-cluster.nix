{ pkgs, mcp-js, ... }:
let
  names = [ "node1" "node2" "node3" ];
  nodeConfig = name: { ... }: {
    imports = [ ../../nix/module.nix ];
    virtualisation.memorySize = 1024;
    environment.systemPackages = [ pkgs.python3 ];
    services.mcp-js = {
      enable = true;
      package = mcp-js;
      settings = {
        node_id = name;
        http_port = 3000;
        cluster_port = 4000;
        advertise_addr = "${name}:4000";
        heap_store = "none";
        heap_memory_max = 64;
        execution_timeout = 3;
        session_db_path = "/var/lib/mcp-js/sessions";
        peers = map (peer: "${peer}@${peer}:4000")
          (builtins.filter (peer: peer != name) names);
        heartbeat_interval = 200;
        election_timeout_min = 1000;
        election_timeout_max = 2000;
      };
    };
    systemd.services.mcp-js.environment.MCP_V8_HTTP_FUNCTIONS_DIR = "/var/lib/mcp-js/http-functions";
  };
in {
  name = "mcp-js-http-functions-cluster";
  nodes = builtins.listToAttrs (map (name: { inherit name; value = nodeConfig name; }) names);
  testScript = builtins.readFile ./http-functions-common.py
    + builtins.readFile ./http-functions-cluster.py;
}
