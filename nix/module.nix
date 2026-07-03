# NixOS module for the MCP JS server.
#
# All server configuration lives under `services.mcp-js.settings`, whose
# per-key option set is NOT written by hand: it is generated into
# ./options.nix from the server's own Clap definition and `--config` loader
# tables (see server/src/bin/generate-nix-options.rs), so every server flag
# is automatically configurable here and CI fails if the two drift. Non-null
# settings are rendered to a TOML file and passed via `--config`; the server
# validates every key at startup (unknown keys are fatal and list the
# accepted vocabulary), so the module cannot silently accept stale options.
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
  # The pre-`settings` options are deprecated: keys with a 1:1 config-file
  # twin are renamed onto it (they keep working, with a warning), the rest
  # are removed with a pointer to their replacement.
  imports = lib.mapAttrsToList
    (old: new: lib.mkRenamedOptionModule
      [ "services" "mcp-js" old ]
      [ "services" "mcp-js" "settings" new ])
    {
      nodeId = "node_id";
      httpPort = "http_port";
      clusterPort = "cluster_port";
      peers = "peers";
      advertiseAddr = "advertise_addr";
      join = "join";
      heartbeatInterval = "heartbeat_interval";
      electionTimeoutMin = "election_timeout_min";
      electionTimeoutMax = "election_timeout_max";
      allowExternalModules = "allow_external_modules";
      policiesJson = "policies_json";
    }
  ++ [
    (lib.mkRemovedOptionModule [ "services" "mcp-js" "stateless" ]
      "The server is stateless unless told otherwise; for the old stateful behaviour set services.mcp-js.settings.{heap_store = \"dir\", heap_dir, session_db_path, cluster_port} explicitly.")
    (lib.mkRemovedOptionModule [ "services" "mcp-js" "dataDir" ]
      "Set services.mcp-js.settings.heap_dir and services.mcp-js.settings.session_db_path instead (the service's state directory is /var/lib/mcp-js).")
    (lib.mkRemovedOptionModule [ "services" "mcp-js" "certFile" ]
      "Set systemd.services.mcp-js.environment.MCP_JS_CERT_FILE instead.")
    (lib.mkRemovedOptionModule [ "services" "mcp-js" "keyFile" ]
      "Set systemd.services.mcp-js.environment.MCP_JS_KEY_FILE instead.")
    (lib.mkRemovedOptionModule [ "services" "mcp-js" "caFile" ]
      "Set systemd.services.mcp-js.environment.MCP_JS_CA_FILE instead.")
  ];

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
  };

  config = lib.mkIf cfg.enable {
    systemd.services.mcp-js = {
      description = "MCP JS Server";
      after = [ "network.target" ];
      wantedBy = [ "multi-user.target" ];

      serviceConfig = {
        ExecStart = "${cfg.package}/bin/server --config ${configFile}";
        Restart = "on-failure";
        RestartSec = "2s";
        StateDirectory = "mcp-js";
        WorkingDirectory = "/var/lib/mcp-js";
        DynamicUser = true;
      };
    };

    # Open exactly the ports the merged settings bind.
    networking.firewall.allowedTCPPorts = lib.filter (port: port != null) [
      cfg.settings.cluster_port
      cfg.settings.http_port
      cfg.settings.sse_port
    ];
  };
}
