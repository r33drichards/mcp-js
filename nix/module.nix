# NixOS module for the MCP JS server.
#
# The per-key option set (`services.mcp-js.settings.*`) is NOT written by
# hand: it is generated into ./options.nix from the server's own Clap
# definition and `--config` loader tables (see
# server/src/bin/generate-nix-options.rs), so every server flag is
# automatically configurable here and CI fails if the two drift. Non-null
# settings are rendered to a TOML file and passed via `--config`; the server
# validates every key at startup (unknown keys are fatal and list the
# accepted vocabulary), so the module cannot silently accept stale options.
#
# The remaining top-level options (nodeId, peers, stateless, ...) are a thin
# convenience layer with cluster-oriented defaults; each one just maps onto a
# `settings` key with `lib.mkDefault`, so an explicit `settings.*` value
# always wins.
{ config, lib, pkgs, ... }:

let
  cfg = config.services.mcp-js;

  settingsFormat = pkgs.formats.toml { };

  # The server picks the parser by file extension, so the name must end in
  # ".toml". Keys left at null mean "not set": the server's built-in default
  # and any MCP_V8_* environment variable apply unchanged.
  configFile = settingsFormat.generate "mcp-js-config.toml"
    (lib.filterAttrs (_: value: value != null) cfg.settings);
in
{
  options.services.mcp-js = {
    enable = lib.mkEnableOption "MCP JS server";

    package = lib.mkOption {
      type = lib.types.package;
      description = "The mcp-js server package to use.";
    };

    settings = lib.mkOption {
      type = lib.types.submodule {
        options = import ./options.nix { inherit lib; };
      };
      default = { };
      description = ''
        Server configuration, one option per config-file key (see the
        "Configuration file" reference page). The option set is generated
        from the server's CLI definition, so everything the CLI can
        configure is available here. Values are written to a TOML file
        passed via `--config`; `null` omits the key.
      '';
    };

    nodeId = lib.mkOption {
      type = lib.types.str;
      default = "node1";
      description = "Unique identifier for this cluster node. Maps to `settings.node_id`.";
    };

    peers = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      description = "List of peer addresses in host:port format. Maps to `settings.peers`.";
    };

    clusterPort = lib.mkOption {
      type = lib.types.port;
      default = 4000;
      description = "Port for the Raft cluster HTTP server (only used when `stateless` is false). Maps to `settings.cluster_port`.";
    };

    httpPort = lib.mkOption {
      type = lib.types.nullOr lib.types.port;
      default = 3000;
      description = "Port for the HTTP MCP transport (required in cluster mode). Maps to `settings.http_port`; set to null when configuring `settings.sse_port` instead.";
    };

    dataDir = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/mcp-js";
      description = "Directory for heap storage and session database (only used when `stateless` is false).";
    };

    stateless = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Run in stateless mode: no heap persistence, no session database, and no Raft cluster listener.";
    };

    allowExternalModules = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Allow external module imports (npm:/jsr:/URL). Maps to `settings.allow_external_modules`.";
    };

    heartbeatInterval = lib.mkOption {
      type = lib.types.int;
      default = 200;
      description = "Raft heartbeat interval in milliseconds. Maps to `settings.heartbeat_interval`.";
    };

    electionTimeoutMin = lib.mkOption {
      type = lib.types.int;
      default = 1000;
      description = "Minimum Raft election timeout in milliseconds. Maps to `settings.election_timeout_min`.";
    };

    electionTimeoutMax = lib.mkOption {
      type = lib.types.int;
      default = 2000;
      description = "Maximum Raft election timeout in milliseconds. Maps to `settings.election_timeout_max`.";
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
      description = "Externally-reachable address (host:port) for this node. Defaults to <nodeId>:<clusterPort>. Maps to `settings.advertise_addr`.";
    };

    join = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "Seed address (host:port) to join an existing cluster dynamically. Maps to `settings.join`.";
    };

    policiesJson = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "JSON policy configuration (inline JSON or path to a JSON file). Maps to `settings.policies_json`; prefer the structured `settings.policies` section for new configurations.";
    };
  };

  config = lib.mkIf cfg.enable {
    # The convenience options only ever contribute defaults, so explicit
    # `settings.*` definitions take precedence.
    services.mcp-js.settings = lib.mkMerge [
      {
        node_id = lib.mkDefault cfg.nodeId;
      }
      (lib.mkIf (cfg.httpPort != null) {
        http_port = lib.mkDefault cfg.httpPort;
      })
      (lib.mkIf (!cfg.stateless) {
        heap_store = lib.mkDefault "dir";
        heap_dir = lib.mkDefault cfg.dataDir;
        session_db_path = lib.mkDefault "${cfg.dataDir}/sessions";
        cluster_port = lib.mkDefault cfg.clusterPort;
        heartbeat_interval = lib.mkDefault cfg.heartbeatInterval;
        election_timeout_min = lib.mkDefault cfg.electionTimeoutMin;
        election_timeout_max = lib.mkDefault cfg.electionTimeoutMax;
      })
      (lib.mkIf (cfg.peers != [ ]) {
        peers = lib.mkDefault cfg.peers;
      })
      (lib.mkIf (cfg.advertiseAddr != null) {
        advertise_addr = lib.mkDefault cfg.advertiseAddr;
      })
      (lib.mkIf (cfg.join != null) {
        join = lib.mkDefault cfg.join;
      })
      (lib.mkIf cfg.allowExternalModules {
        allow_external_modules = lib.mkDefault true;
      })
      (lib.mkIf (cfg.policiesJson != null) {
        policies_json = lib.mkDefault cfg.policiesJson;
      })
    ];

    systemd.services.mcp-js = {
      description = "MCP JS Server";
      after = [ "network.target" ];
      wantedBy = [ "multi-user.target" ];

      serviceConfig = {
        ExecStart = "${cfg.package}/bin/server --config ${configFile}";
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

    # Open exactly the ports the merged settings bind.
    networking.firewall.allowedTCPPorts = lib.filter (port: port != null) [
      cfg.settings.cluster_port
      cfg.settings.http_port
      cfg.settings.sse_port
    ];
  };
}
