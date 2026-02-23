{ config, lib, pkgs, ... }:

let
  cfg = config.services.mcp-js;
in
{
  options.services.mcp-js = {
    enable = lib.mkEnableOption "MCP JS server";

    package = lib.mkOption {
      type = lib.types.package;
      description = "The mcp-js server package to use.";
    };

    nodeId = lib.mkOption {
      type = lib.types.str;
      description = "Unique identifier for this cluster node.";
    };

    peers = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      description = "List of peer addresses in host:port format.";
    };

    clusterPort = lib.mkOption {
      type = lib.types.port;
      default = 4000;
      description = "Port for the Raft cluster HTTP server.";
    };

    ssePort = lib.mkOption {
      type = lib.types.nullOr lib.types.port;
      default = null;
      description = "Port for the SSE MCP transport. Null to disable.";
    };

    httpPort = lib.mkOption {
      type = lib.types.nullOr lib.types.port;
      default = null;
      description = "Port for the HTTP MCP transport. Null to disable.";
    };

    dataDir = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/mcp-js";
      description = "Directory for heap storage and session database.";
    };

    stateless = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Run in stateless mode (no heap persistence).";
    };

    heartbeatInterval = lib.mkOption {
      type = lib.types.int;
      default = 200;
      description = "Raft heartbeat interval in milliseconds.";
    };

    electionTimeoutMin = lib.mkOption {
      type = lib.types.int;
      default = 1000;
      description = "Minimum Raft election timeout in milliseconds.";
    };

    electionTimeoutMax = lib.mkOption {
      type = lib.types.int;
      default = 2000;
      description = "Maximum Raft election timeout in milliseconds.";
    };

    certFile = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = null;
      description = "Path to TLS certificate file for cluster communication.";
    };

    keyFile = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = null;
      description = "Path to TLS key file for cluster communication.";
    };

    caFile = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = null;
      description = "Path to CA certificate file for cluster communication.";
    };
  };

  config = lib.mkIf cfg.enable {
    systemd.services.mcp-js = {
      description = "MCP JS Server (Raft Cluster Node)";
      after = [ "network.target" ];
      wantedBy = [ "multi-user.target" ];

      serviceConfig = {
        ExecStart =
          let
            baseArgs = [
              "${cfg.package}/bin/server"
              "--directory-path"
              cfg.dataDir
              "--session-db-path"
              "${cfg.dataDir}/sessions"
              "--cluster-port"
              (toString cfg.clusterPort)
              "--node-id"
              cfg.nodeId
              "--heartbeat-interval"
              (toString cfg.heartbeatInterval)
              "--election-timeout-min"
              (toString cfg.electionTimeoutMin)
              "--election-timeout-max"
              (toString cfg.electionTimeoutMax)
            ];

            peerArgs = lib.optionals (cfg.peers != [ ]) [
              "--peers"
              (lib.concatStringsSep "," cfg.peers)
            ];

            sseArgs = lib.optionals (cfg.ssePort != null) [
              "--sse-port"
              (toString cfg.ssePort)
            ];

            httpArgs = lib.optionals (cfg.httpPort != null) [
              "--http-port"
              (toString cfg.httpPort)
            ];

            statelessArgs = lib.optionals cfg.stateless [ "--stateless" ];
          in
          lib.concatStringsSep " " (baseArgs ++ peerArgs ++ sseArgs ++ httpArgs ++ statelessArgs);

        Restart = "on-failure";
        RestartSec = "2s";
        StateDirectory = "mcp-js";
        WorkingDirectory = cfg.dataDir;
        DynamicUser = true;
      };

      environment = lib.mkMerge [
        (lib.mkIf (cfg.certFile != null) {
          MCP_JS_CERT_FILE = toString cfg.certFile;
          MCP_JS_KEY_FILE = toString cfg.keyFile;
          MCP_JS_CA_FILE = toString cfg.caFile;
        })
      ];
    };

    networking.firewall.allowedTCPPorts =
      [ cfg.clusterPort ]
      ++ lib.optionals (cfg.ssePort != null) [ cfg.ssePort ]
      ++ lib.optionals (cfg.httpPort != null) [ cfg.httpPort ];
  };
}
