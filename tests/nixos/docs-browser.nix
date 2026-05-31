{ pkgs, docsSite, ... }:

{
  name = "mcp-js-docs-browser";

  nodes.machine =
    { ... }:
    {
      networking.firewall.allowedTCPPorts = [ 8000 ];

      environment.systemPackages = with pkgs; [
        chromium
        curl
        python3
      ];
    };

  testScript = ''
    import shlex

    def page_dom(path):
        url = "http://localhost:8000" + path
        return machine.succeed(
            "chromium "
            "--headless "
            "--disable-gpu "
            "--disable-dev-shm-usage "
            "--no-sandbox "
            "--user-data-dir=/tmp/chromium-profile "
            "--virtual-time-budget=5000 "
            "--dump-dom "
            + shlex.quote(url)
        )

    machine.start()

    machine.succeed(
        "python3 -m http.server 8000 --directory ${docsSite} >/tmp/docs-http.log 2>&1 &"
    )
    machine.wait_for_open_port(8000)
    machine.wait_until_succeeds("curl -sf http://localhost:8000/ >/dev/null")

    with subtest("home page renders"):
        home = page_dom("/")
        assert "mcp-v8" in home
        assert "Quick Start" in home

    with subtest("quick start renders"):
        quick_start = page_dom("/quick-start/overview/")
        assert "Quick Start Overview" in quick_start

    with subtest("http api reference renders"):
        http_api = page_dom("/reference/http-api/")
        assert "HTTP API" in http_api

    with subtest("mcp tools reference renders"):
        mcp_tools = page_dom("/reference/mcp-tools/")
        assert "MCP Tools" in mcp_tools
  '';
}
