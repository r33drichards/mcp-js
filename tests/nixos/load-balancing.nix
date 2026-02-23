{ pkgs, mcp-js, ... }:

let
  # ── Peer list (3-node cluster) ───────────────────────────────────────

  allPeerAddrs = [
    "node1@node1:4000"
    "node2@node2:4000"
    "node3@node3:4000"
  ];

  peersFor =
    nodeId:
    builtins.filter (p: !(pkgs.lib.hasPrefix "${nodeId}@" p)) allPeerAddrs;

  # ── Shared node configuration ─────────────────────────────────────────

  nodeConfig =
    nodeId:
    {
      imports = [ ../../nix/module.nix ];

      services.mcp-js = {
        enable = true;
        package = mcp-js;
        inherit nodeId;
        peers = peersFor nodeId;
        clusterPort = 4000;
        httpPort = 3000;
        heartbeatInterval = 200;
        electionTimeoutMin = 1000;
        electionTimeoutMax = 2000;
      };

      networking.firewall.allowedTCPPorts = [ 3000 4000 ];
    };

  # ── Nginx load balancer configuration ──────────────────────────────────

  lbConfig = {
    services.nginx = {
      enable = true;
      upstreams.mcp_cluster = {
        servers = {
          "node1:3000" = {};
          "node2:3000" = {};
          "node3:3000" = {};
        };
      };
      virtualHosts.default = {
        default = true;
        locations."/mcp" = {
          proxyPass = "http://mcp_cluster";
          extraConfig = ''
            proxy_http_version 1.1;
            proxy_set_header Host $host;
            proxy_set_header Connection '';
            proxy_buffering off;
            proxy_cache off;
            proxy_read_timeout 3600s;
            proxy_send_timeout 3600s;
          '';
        };
        locations."/health" = {
          proxyPass = "http://mcp_cluster/raft/status";
        };
      };
    };

    networking.firewall.allowedTCPPorts = [ 80 ];
  };
in

{
  name = "mcp-js-load-balancing";

  nodes = {
    node1 = { ... }: nodeConfig "node1";
    node2 = { ... }: nodeConfig "node2";
    node3 = { ... }: nodeConfig "node3";
    lb = { ... }: lbConfig;
  };

  testScript = ''
    import json
    import time

    all_nodes = [node1, node2, node3]
    node_names = ["node1", "node2", "node3"]

    def get_status(node):
        """Query the Raft status endpoint on a node."""
        raw = node.succeed("curl -sf http://localhost:4000/raft/status")
        return json.loads(raw)

    def find_leader(nodes):
        """Return (node, status) of the current leader, or (None, None)."""
        for n in nodes:
            try:
                st = get_status(n)
                if st["role"] == "Leader":
                    return n, st
            except Exception:
                pass
        return None, None

    def wait_for_leader(nodes, timeout=30):
        """Poll until a leader is elected among the given nodes."""
        for _ in range(timeout):
            leader, status = find_leader(nodes)
            if leader is not None:
                return leader, status
            time.sleep(1)
        raise AssertionError("No leader elected within {} seconds".format(timeout))

    # ── Start the cluster and load balancer ─────────────────────────────

    with subtest("should start all nodes and load balancer"):
        for node in all_nodes:
            node.start()
        lb.start()
        for node in all_nodes:
            node.wait_for_unit("mcp-js.service")
        lb.wait_for_unit("nginx.service")

    # ── Verify leader election ──────────────────────────────────────────

    with subtest("should elect a leader"):
        leader, status = wait_for_leader(all_nodes)
        leader_id = status["node_id"]
        print(f"Leader elected: {leader_id} (term {status['term']})")

    # ── Write data through leader and verify replication ────────────────

    with subtest("should replicate data through Raft"):
        leader.succeed(
            "curl -sf -X POST http://localhost:4000/data/put "
            "-H 'Content-Type: application/json' "
            "-d '{\"key\":\"lb-test\",\"value\":\"hello-cluster\"}'"
        )
        time.sleep(3)

        for i, node in enumerate(all_nodes):
            result = node.succeed("curl -sf http://localhost:4000/data/get/lb-test")
            data = json.loads(result)
            assert data["value"] == "hello-cluster", \
                f"{node_names[i]}: expected 'hello-cluster', got {data}"

    # ── Test write forwarding from followers ─────────────────────────────

    with subtest("should forward writes from followers to leader"):
        # Find a follower
        for i, node in enumerate(all_nodes):
            st = get_status(node)
            if st["role"] != "Leader":
                follower = node
                follower_name = node_names[i]
                break

        # Write through the follower's data API – this should forward to leader
        follower.succeed(
            "curl -sf -X POST http://localhost:4000/data/put "
            "-H 'Content-Type: application/json' "
            "-d '{\"key\":\"forwarded\",\"value\":\"from-follower\"}'"
        )
        time.sleep(3)

        for i, node in enumerate(all_nodes):
            result = node.succeed("curl -sf http://localhost:4000/data/get/forwarded")
            data = json.loads(result)
            assert data["value"] == "from-follower", \
                f"{node_names[i]}: forwarded write not replicated: {data}"

    # ── Test load balancer routing ───────────────────────────────────────

    with subtest("should route requests through load balancer"):
        # Health check through LB
        result = lb.succeed("curl -sf http://localhost:80/health")
        data = json.loads(result)
        assert "node_id" in data, f"Health check failed: {data}"
        print(f"LB health check: routed to {data['node_id']}")

    # ── Test leader crash and recovery ──────────────────────────────────

    with subtest("should survive leader crash with write forwarding"):
        leader_idx = None
        for i, node in enumerate(all_nodes):
            st = get_status(node)
            if st["role"] == "Leader":
                leader_idx = i
                break
        assert leader_idx is not None

        print(f"Crashing leader {node_names[leader_idx]}")
        all_nodes[leader_idx].crash()

        surviving = [
            n for i, n in enumerate(all_nodes)
            if i != leader_idx
        ]
        surviving_names = [
            name for i, name in enumerate(node_names)
            if i != leader_idx
        ]

        # Wait for new leader among the 2 surviving nodes
        new_leader, new_status = wait_for_leader(surviving, timeout=30)
        print(f"New leader: {new_status['node_id']} (term {new_status['term']})")

        # Write through the new leader
        new_leader.succeed(
            "curl -sf -X POST http://localhost:4000/data/put "
            "-H 'Content-Type: application/json' "
            "-d '{\"key\":\"after-leader-crash\",\"value\":\"still-alive\"}'"
        )
        time.sleep(2)

        for i, node in enumerate(surviving):
            result = node.succeed("curl -sf http://localhost:4000/data/get/after-leader-crash")
            data = json.loads(result)
            assert data["value"] == "still-alive", \
                f"{surviving_names[i]}: post-crash write not replicated: {data}"

        # Bring crashed node back
        all_nodes[leader_idx].start()
        all_nodes[leader_idx].wait_for_unit("mcp-js.service")
        time.sleep(5)

        # Verify all data is consistent
        for i, node in enumerate(all_nodes):
            result = node.succeed("curl -sf http://localhost:4000/data/get/lb-test")
            data = json.loads(result)
            assert data["value"] == "hello-cluster", \
                f"{node_names[i]}: data inconsistent after rejoin: {data}"
  '';
}
