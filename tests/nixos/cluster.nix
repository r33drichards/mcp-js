{ pkgs, mcp-js, ... }:

let
  # ── TLS certificate generation ────────────────────────────────────────
  # Following the etcd NixOS test pattern: generate a CA + per-node certs
  # with openssl so that cluster traffic is authenticated.

  runWithOpenSSL =
    file: cmd:
    pkgs.runCommand file {
      buildInputs = [ pkgs.openssl ];
    } cmd;

  ca_key = runWithOpenSSL "ca-key.pem" "openssl genrsa -out $out 2048";
  ca_pem = runWithOpenSSL "ca.pem" ''
    openssl req \
      -x509 -new -nodes -key ${ca_key} \
      -days 10000 -out $out -subj "/CN=mcp-js-ca"
  '';

  node_key = runWithOpenSSL "node-key.pem" "openssl genrsa -out $out 2048";
  node_csr = runWithOpenSSL "node.csr" ''
    openssl req \
      -new -key ${node_key} \
      -out $out -subj "/CN=mcp-js-node" \
      -config ${openssl_cnf}
  '';
  node_cert = runWithOpenSSL "node.pem" ''
    openssl x509 \
      -req -in ${node_csr} \
      -CA ${ca_pem} -CAkey ${ca_key} \
      -CAcreateserial -out $out \
      -days 365 -extensions v3_req \
      -extfile ${openssl_cnf}
  '';

  client_key = runWithOpenSSL "client-key.pem" "openssl genrsa -out $out 2048";
  client_csr = runWithOpenSSL "client.csr" ''
    openssl req \
      -new -key ${client_key} \
      -out $out -subj "/CN=mcp-js-client" \
      -config ${client_openssl_cnf}
  '';
  client_cert = runWithOpenSSL "client.pem" ''
    openssl x509 \
      -req -in ${client_csr} \
      -CA ${ca_pem} -CAkey ${ca_key} \
      -CAcreateserial -out $out \
      -days 365 -extensions v3_req \
      -extfile ${client_openssl_cnf}
  '';

  openssl_cnf = pkgs.writeText "openssl.cnf" ''
    [req]
    req_extensions = v3_req
    distinguished_name = req_distinguished_name
    [req_distinguished_name]
    [v3_req]
    basicConstraints = CA:FALSE
    keyUsage = digitalSignature, keyEncipherment
    extendedKeyUsage = serverAuth, clientAuth
    subjectAltName = @alt_names
    [alt_names]
    DNS.1 = node1
    DNS.2 = node2
    DNS.3 = node3
    DNS.4 = node4
    DNS.5 = node5
    IP.1 = 127.0.0.1
  '';

  client_openssl_cnf = pkgs.writeText "client-openssl.cnf" ''
    [req]
    req_extensions = v3_req
    distinguished_name = req_distinguished_name
    [req_distinguished_name]
    [v3_req]
    basicConstraints = CA:FALSE
    keyUsage = digitalSignature, keyEncipherment
    extendedKeyUsage = clientAuth
  '';

  # ── Peer list ─────────────────────────────────────────────────────────

  allPeerAddrs = [
    "node1:4000"
    "node2:4000"
    "node3:4000"
    "node4:4000"
    "node5:4000"
  ];

  peersFor =
    nodeId:
    builtins.filter (p: !(pkgs.lib.hasPrefix "${nodeId}:" p)) allPeerAddrs;

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
        heartbeatInterval = 200;
        electionTimeoutMin = 1000;
        electionTimeoutMax = 2000;
        certFile = node_cert;
        keyFile = node_key;
        caFile = ca_pem;
      };

      # Certificates available in the environment for CLI tools
      environment.variables = {
        MCP_JS_CERT_FILE = "${client_cert}";
        MCP_JS_KEY_FILE = "${client_key}";
        MCP_JS_CA_FILE = "${ca_pem}";
      };

      networking.firewall.allowedTCPPorts = [ 4000 ];
    };
in

{
  name = "mcp-js-cluster";

  nodes = {
    node1 = { ... }: nodeConfig "node1";
    node2 = { ... }: nodeConfig "node2";
    node3 = { ... }: nodeConfig "node3";
    node4 = { ... }: nodeConfig "node4";
    node5 = { ... }: nodeConfig "node5";
  };

  testScript = ''
    import json
    import time

    all_nodes = [node1, node2, node3, node4, node5]
    node_names = ["node1", "node2", "node3", "node4", "node5"]

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

    # ── Start the 5-node cluster ────────────────────────────────────────

    with subtest("should start all cluster nodes"):
        for node in all_nodes:
            node.start()
        for node in all_nodes:
            node.wait_for_unit("mcp-js.service")

    # ── Verify leader election ──────────────────────────────────────────

    with subtest("should elect a leader"):
        leader, status = wait_for_leader(all_nodes)
        leader_id = status["node_id"]
        print(f"Leader elected: {leader_id} (term {status['term']})")

    # ── Write data and verify replication ────────────────────────────────

    with subtest("should replicate data to all nodes"):
        leader.succeed(
            "curl -sf -X POST http://localhost:4000/data/put "
            "-H 'Content-Type: application/json' "
            "-d '{\"key\":\"foo\",\"value\":\"Hello cluster\"}'"
        )
        leader.succeed(
            "curl -sf -X POST http://localhost:4000/data/put "
            "-H 'Content-Type: application/json' "
            "-d '{\"key\":\"bar\",\"value\":\"42\"}'"
        )

        # Allow time for replication
        time.sleep(3)

        for i, node in enumerate(all_nodes):
            result = node.succeed("curl -sf http://localhost:4000/data/get/foo")
            data = json.loads(result)
            assert data["value"] == "Hello cluster", \
                f"{node_names[i]}: expected 'Hello cluster', got {data}"

            result2 = node.succeed("curl -sf http://localhost:4000/data/get/bar")
            data2 = json.loads(result2)
            assert data2["value"] == "42", \
                f"{node_names[i]}: expected '42', got {data2}"

    # ── Crash the leader and one random follower ─────────────────────────

    with subtest("should survive leader + follower crash"):
        # Identify the leader index
        leader_idx = None
        for i, node in enumerate(all_nodes):
            st = get_status(node)
            if st["role"] == "Leader":
                leader_idx = i
                break
        assert leader_idx is not None

        # Pick a non-leader node to also crash
        random_idx = (leader_idx + 2) % 5
        print(f"Crashing leader {node_names[leader_idx]} and follower {node_names[random_idx]}")

        all_nodes[leader_idx].crash()
        all_nodes[random_idx].crash()

        surviving = [
            n for i, n in enumerate(all_nodes)
            if i != leader_idx and i != random_idx
        ]
        surviving_names = [
            name for i, name in enumerate(node_names)
            if i != leader_idx and i != random_idx
        ]

        # Wait for a new leader among the 3 surviving nodes
        new_leader, new_status = wait_for_leader(surviving, timeout=30)
        print(f"New leader: {new_status['node_id']} (term {new_status['term']})")

        # Verify NO data was lost
        for i, node in enumerate(surviving):
            result = node.succeed("curl -sf http://localhost:4000/data/get/foo")
            data = json.loads(result)
            assert data["value"] == "Hello cluster", \
                f"{surviving_names[i]}: data lost for key 'foo': {data}"

            result2 = node.succeed("curl -sf http://localhost:4000/data/get/bar")
            data2 = json.loads(result2)
            assert data2["value"] == "42", \
                f"{surviving_names[i]}: data lost for key 'bar': {data2}"

        # Write new data to verify the degraded cluster still works
        new_leader.succeed(
            "curl -sf -X POST http://localhost:4000/data/put "
            "-H 'Content-Type: application/json' "
            "-d '{\"key\":\"after-crash\",\"value\":\"still-works\"}'"
        )
        time.sleep(2)

        for i, node in enumerate(surviving):
            result = node.succeed("curl -sf http://localhost:4000/data/get/after-crash")
            data = json.loads(result)
            assert data["value"] == "still-works", \
                f"{surviving_names[i]}: post-crash write not replicated: {data}"

    # ── Bring crashed nodes back and verify recovery ─────────────────────

    with subtest("should recover when crashed nodes rejoin"):
        all_nodes[leader_idx].start()
        all_nodes[random_idx].start()
        all_nodes[leader_idx].wait_for_unit("mcp-js.service")
        all_nodes[random_idx].wait_for_unit("mcp-js.service")

        # Allow time for the rejoined nodes to catch up
        time.sleep(5)

        # Verify all 5 nodes have consistent data (no deadlock)
        for i, node in enumerate(all_nodes):
            st = get_status(node)
            print(f"{node_names[i]}: role={st['role']}, term={st['term']}, commit={st['commit_index']}")

            result = node.succeed("curl -sf http://localhost:4000/data/get/foo")
            data = json.loads(result)
            assert data["value"] == "Hello cluster", \
                f"{node_names[i]}: data inconsistent after rejoin: {data}"

            result2 = node.succeed("curl -sf http://localhost:4000/data/get/after-crash")
            data2 = json.loads(result2)
            assert data2["value"] == "still-works", \
                f"{node_names[i]}: post-crash data missing after rejoin: {data2}"

        # Verify exactly one leader exists
        leaders = []
        for i, node in enumerate(all_nodes):
            st = get_status(node)
            if st["role"] == "Leader":
                leaders.append(node_names[i])
        assert len(leaders) == 1, f"Expected 1 leader, found {len(leaders)}: {leaders}"
  '';
}
