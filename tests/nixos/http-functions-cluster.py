

nodes = [node1, node2, node3]
management = "/api/http-functions/v1/definitions"


with subtest("three VM services elect a leader and register a durable handler"):
    start_all()
    for node in nodes:
        node.wait_for_unit("mcp-js.service")
        node.wait_for_open_port(3000)
        node.wait_for_open_port(4000)
    leader = committed_leader(nodes)
    followers = [node for node in nodes if node is not leader]
    source = definition("async function handler() { return {status:200,body:'revision-one'}; }")
    created = assert_status(http(leader, "PUT", management + "/durable", source,
                                 headers={"If-None-Match": "*"}), 200)
    first_revision = created["headers"]["etag"]
    assert assert_status(http(leader, "POST", "/functions/v1/durable"), 200)["body"] == b"revision-one"
    for follower in followers:
        assert_status(http(follower, "POST", "/functions/v1/durable"), 503)
        assert_status(http(follower, "PUT", management + "/follower-write", source,
                           headers={"If-None-Match": "*"}), 503)

with subtest("crashing the leader retains the revision and conditional update contract"):
    old_leader = leader
    old_leader.crash()
    leader = committed_leader(followers)
    metadata = assert_status(http(leader, "GET", management + "/durable"), 200)
    assert metadata["headers"]["etag"] == first_revision
    assert assert_status(http(leader, "POST", "/functions/v1/durable"), 200)["body"] == b"revision-one"
    replacement = definition("function handler() { return {status:200,body:'revision-two'}; }")
    updated = assert_status(http(leader, "PUT", management + "/durable", replacement,
                                 headers={"If-Match": first_revision}), 200)
    second_revision = updated["headers"]["etag"]
    assert second_revision != first_revision
    assert_status(http(leader, "DELETE", management + "/durable",
                       headers={"If-Match": first_revision}), 412)
    old_leader.start()
    old_leader.wait_for_unit("mcp-js.service")
    old_leader.wait_for_open_port(4000)
    leader = committed_leader(nodes)
    assert assert_status(http(leader, "POST", "/functions/v1/durable"), 200)["body"] == b"revision-two"

with subtest("quorum loss blocks reads, writes, and invocation instead of serving cached source"):
    stopped = [node for node in nodes if node is not leader]
    for node in stopped:
        node.succeed("systemctl stop mcp-js.service")
    assert_status(http(leader, "POST", "/functions/v1/durable"), 503)
    assert_status(http(leader, "GET", management + "/durable"), 503)
    # A timed-out proposal may commit after recovery. Do not replay it or assume rollback.
    assert_status(http(leader, "PUT", management + "/ambiguous", source,
                       headers={"If-None-Match": "*"}), 503)
    for node in stopped:
        node.succeed("systemctl start mcp-js.service")
        node.wait_for_unit("mcp-js.service")
        node.wait_for_open_port(4000)
    leader = committed_leader(nodes)
    assert assert_status(http(leader, "POST", "/functions/v1/durable"), 200)["body"] == b"revision-two"

with subtest("a committed deletion survives full cluster service restart and prevents ABA"):
    assert_status(http(leader, "DELETE", management + "/durable",
                       headers={"If-Match": second_revision}), 200)
    for node in nodes:
        node.succeed("systemctl stop mcp-js.service")
    for node in nodes:
        node.succeed("systemctl start mcp-js.service")
        node.wait_for_unit("mcp-js.service")
        node.wait_for_open_port(4000)
    leader = committed_leader(nodes)
    assert_status(http(leader, "POST", "/functions/v1/durable"), 404)
    assert_status(http(leader, "PUT", management + "/durable", source,
                       headers={"If-Match": second_revision}), 412)
    recreated = assert_status(http(leader, "PUT", management + "/durable", source,
                                   headers={"If-None-Match": "*"}), 200)
    assert recreated["headers"]["etag"] not in [first_revision, second_revision]
    assert_status(http(leader, "DELETE", management + "/durable",
                       headers={"If-Match": second_revision}), 412)
    assert assert_status(http(leader, "POST", "/functions/v1/durable"), 200)["body"] == b"revision-one"
