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

    httpPort = lib.mkOption {
      type = lib.types.port;
      default = 3000;
      description = "Port for the HTTP MCP transport (required in cluster mode).";
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

    advertiseAddr = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "Externally-reachable address (host:port) for this node. Defaults to <nodeId>:<clusterPort>.";
    };

    join = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "Seed address (host:port) to join an existing cluster dynamically.";
    };

    opaUrl = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "OPA server URL for policy-gated fetch. When set, fetch() becomes available in the JS runtime.";
    };

    opaFetchPolicy = lib.mkOption {
      type = lib.types.str;
      default = "mcp/fetch";
      description = "OPA policy path for fetch requests (appended to /v1/data/).";
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
              "--node-id"
              cfg.nodeId
              "--http-port"
              (toString cfg.httpPort)
            ];

            storageArgs = lib.optionals (!cfg.stateless) [
              "--directory-path"
              cfg.dataDir
              "--session-db-path"
              "${cfg.dataDir}/sessions"
              "--cluster-port"
              (toString cfg.clusterPort)
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

            statelessArgs = lib.optionals cfg.stateless [ "--stateless" ];

            advertiseArgs = lib.optionals (cfg.advertiseAddr != null) [
              "--advertise-addr"
              cfg.advertiseAddr
            ];

            joinArgs = lib.optionals (cfg.join != null) [
              "--join"
              cfg.join
            ];

            opaArgs = lib.optionals (cfg.opaUrl != null) [
              "--opa-url"
              cfg.opaUrl
              "--opa-fetch-policy"
              cfg.opaFetchPolicy
            ];
          in
          lib.concatStringsSep " " (baseArgs ++ storageArgs ++ peerArgs ++ statelessArgs ++ advertiseArgs ++ joinArgs ++ opaArgs);

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

    networking.firewall.allowedTCPPorts = [ cfg.clusterPort cfg.httpPort ];
  };
}
