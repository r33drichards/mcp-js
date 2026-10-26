{ pkgs, mcp-js, scheduler-runner, scenario ? "cancellation", ... }:

{
  name = "mcp-js-scheduler-${scenario}";
  nodes.machine = { ... }: {
    virtualisation.memorySize = 4096;
    virtualisation.cores = 4;
    services.timesyncd.enable = false;
    environment.systemPackages = [ pkgs.python3 pkgs.curl mcp-js scheduler-runner ];
    environment.etc."scheduler-integration.py".source = ./scheduler/integration.py;
  };
  testScript = ''
    machine.start()
    machine.wait_for_unit("multi-user.target")
    with subtest("scheduler ${scenario} with real Raft and JavaScript worker"):
        try:
            machine.succeed(
                "python3 /etc/scheduler-integration.py"
                " --server ${mcp-js}/bin/server"
                " --runner ${scheduler-runner}/bin/mcp-js-scheduler-runner"
                " --scenario ${scenario}",
                timeout=600,
            )
        finally:
            machine.copy_from_vm("/tmp/scheduler-integration", "scheduler-${scenario}")
  '';
}
