# Deploy and test scheduler and HTTP features on NixOS

Use a flake revision containing the features you need. The
[status page](../reference/scheduler-http-status.md) identifies this documentation's
integrated scheduler hardening boundary. The examples are configuration guidance,
not evidence of passing combined-stack VM validation.

## Enable HTTP functions in the server module

Import the flake module and choose its server package:

```nix
{ inputs, ... }: {
  imports = [ inputs.mcp-js.nixosModules.mcp-js ];
  services.mcp-js = {
    enable = true;
    package = inputs.mcp-js.packages.x86_64-linux.default;
    settings = {
      http_port = 8080;
      bind_host = "127.0.0.1";
      heap_store = "none";
      jwks_url = "https://identity.example.com/.well-known/jwks.json";
    };
  };
  systemd.services.mcp-js.environment.MCP_V8_HTTP_FUNCTIONS_DIR =
    "/var/lib/mcp-js/http-functions";
}
```

Replace the JWKS URL with your existing issuer and use your existing TLS ingress.
The module creates `/var/lib/mcp-js` through `StateDirectory` and runs as a dynamic
user. Its generated `settings` are server config keys. HTTP function enablement is
an environment variable, not an invented Nix module option. Definitions persist
in the service state directory in single-node mode.

The module opens configured HTTP/SSE/cluster ports in the NixOS firewall. Check
actual listener reachability and ingress policy for your deployment. The ordinary server
uses legacy dynamic cluster networking; do not infer the scheduler runner's
fixed-voter bearer protection applies to those ports. Keep them private.
Cluster HTTP functions use replicated registry state, accept admission on the
leader, and require quorum; setting the directory alone does not configure Raft.
See [clustering setup](clustering.md).

## Deploy a dedicated scheduled worker separately

The server module can also run the dedicated worker using
`MCP_SCHEDULED_WORKER_CONFIG`, HTTP mode, and `heap_store = "none"`. That mode
replaces ordinary MCP/REST/HTTP-function routes, so use a separate service if you
need both surfaces. Put nonsecret worker JSON in an operator-managed file and set
its namespace/worker ID to match the runner. Preserve its dedicated admission
store across restarts.

Provide the existing bearer credentials through a protected systemd
`EnvironmentFile`; do not embed secrets in Nix strings or the world-readable Nix
store. Keep only environment-variable **names** in JSON. Keep worker, callback,
and Raft network boundaries distinct; remote callback listeners need existing TLS
termination. The runner is packaged separately as
`packages.x86_64-linux.scheduler-runner`; there is no automatic scheduler runner
NixOS service or scheduling option in `services.mcp-js`. Operators must supply
runner units/configuration and a managed stdin/control arrangement themselves.
See [runner configuration](../reference/scheduler.md).

Do not clone active worker stores, reset identity-bound state, or restore stale
consensus snapshots as a fresh authoritative cluster. Retain stores on uncertainty.
There is no automatic schema migration or log compaction.

## Run the actual VM checks

On a Linux host with Nix and usable KVM, run checks separately to limit memory:

```sh
nix build .#checks.x86_64-linux.http-functions-test --no-link -L
nix build .#checks.x86_64-linux.http-functions-cluster-test --no-link -L
nix build .#checks.x86_64-linux.scheduler-disable-test --no-link -L
nix build .#checks.x86_64-linux.scheduler-cancellation-test --no-link -L
nix build .#checks.x86_64-linux.scheduler-disabled-cancellation-test --no-link -L
```

| Check | Observes |
|---|---|
| `http-functions-test` | NixOS service, JWKS, opt-in/isolation, CRUD/ETags, imports, policy header injection, bounds/timeouts, fresh globals, restart and VM reboot |
| `http-functions-cluster-test` | Three nodes, leader crash, revision recovery, quorum loss, restore/full restart, tombstone/ABA protection |
| `scheduler-disable-test` | Accepted work finishes; next scheduled occurrence is suppressed |
| `scheduler-cancellation-test` | Actual Engine heartbeat stops, durable Cancelled, schedule stays enabled and next run succeeds |
| `scheduler-disabled-cancellation-test` | Disable and cancellation remain independent |

The scheduler checks boot three actual runner processes and an actual Engine in
each VM; the HTTP probe observes effects rather than replacing the worker. The
harness advances wall time to real cron boundaries inside the guest while keeping
monotonic deadlines. The tests do not establish external-effect rollback, power-
loss durability, tenant isolation, or every cancellation/takeover race.

## Check native behavior and documentation

```sh
cargo test --locked -p server --lib http_functions -- --test-threads=1
cargo test --locked -p server --lib scheduled_runtime_tests
cargo test --locked -p server --test jwks_auth_e2e -- --test-threads=1
cargo test --locked --manifest-path scheduler-core/Cargo.toml
cargo test --locked --manifest-path scheduler-runner/Cargo.toml
nix build .#packages.x86_64-linux.docs --no-link -L
nix build .#checks.x86_64-linux.docs-generated-check --no-link -L
```

The docs package regenerates API/CLI/config/tool references before a strict
MkDocs build; the drift check compares generated artifacts to their checked-in
sources. Follow the repository's Docs Check workflow for OAuth contract,
compatibility, link, and browser checks. Keep generated reference pages generated;
feature explanations and recipes belong in the hand-written pages.
