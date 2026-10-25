

machine.start()
for unit in ["http-functions-fixture", "mcp-js", "mcp-js-disabled", "mcp-js-scheduled"]:
    machine.wait_for_unit(unit + ".service")
for port in [3000, 3001, 3002]:
    machine.wait_for_open_port(port)


def call(method, path, body=None, headers=None):
    return http(machine, method, path, body, headers, token=bearer_token())


management = "/api/http-functions/v1/definitions"

with subtest("feature is opt-in and absent on scheduled-only workers"):
    for port in [3001, 3002]:
        for method, path in [("GET", management), ("PUT", management + "/echo"),
                             ("POST", "/functions/v1/echo")]:
            assert_status(http(machine, method, path, port=port), 404)
    for path in ["/api/exec", "/mcp"]:
        assert_status(http(machine, "POST", path, port=3002), 404)

with subtest("management and invocation require the transport's verified token"):
    for token in [None, "invalid.jwt.token"]:
        for method, path in [("GET", management), ("PUT", management + "/echo"),
                             ("DELETE", management + "/echo"), ("POST", "/functions/v1/echo")]:
            assert_status(http(machine, method, path, token=token), 401)
    assert json.loads(assert_status(call("GET", management), 200)["body"]) == []

with subtest("live registration maps requests without leaking credentials or console output"):
    source = """
        import { basename } from 'node:path';
        async function handler(request) {
            await Promise.resolve(); console.log('console-only-marker');
            return {status:201, headers:{'content-type':'application/json'},
                body:JSON.stringify({request, imported:basename('/a/module.txt')})};
        }
    """
    echo = definition(source, ["POST", "HEAD"])
    assert_status(call("POST", "/functions/v1/echo"), 404)
    assert_status(call("PUT", management + "/echo", echo), 428)
    created = assert_status(call("PUT", management + "/echo", echo,
                                 headers={"If-None-Match": "*"}), 200)
    revision = created["headers"]["etag"]
    assert "source" not in json.loads(created["body"])
    assert_status(call("PUT", management + "/echo", echo,
                       headers={"If-None-Match": "*"}), 412)
    payload = b"\x00\xff\"}); throw new Error('input injection'); //"
    reply = assert_status(call("POST", "/functions/v1/echo?a=1&a=2", payload,
                               headers={"Cookie": "private-cookie", "Content-Type": "application/octet-stream"}), 201)
    decoded = json.loads(reply["body"])
    assert decoded["imported"] == "module.txt"
    request = decoded["request"]
    assert request["method"] == "POST" and request["path"] == "/functions/v1/echo"
    assert request["query"] == [["a", "1"], ["a", "2"]]
    assert base64.b64decode(request["body_base64"]) == payload
    assert "authorization" not in request["headers"] and "cookie" not in request["headers"]
    assert "console-only-marker" not in reply["body"].decode()
    assert reply["headers"]["x-request-id"]
    wrong_method = assert_status(call("GET", "/functions/v1/echo"), 405)
    assert wrong_method["headers"]["allow"] == "POST, HEAD"
    head = assert_status(call("HEAD", "/functions/v1/echo"), 201)
    assert head["body"] == b"" and int(head["headers"]["content-length"]) > 0
    assert_status(call("POST", "/functions/v1/echo", b"x" * (1024 * 1024 + 1)), 413)

with subtest("handler results obey binary, status, header, and timeout contracts"):
    cases = [
        ("return {status:200,body_base64:'AAH/'}", 200, b"\x00\x01\xff"),
        ("return {status:204,body:'suppressed'}", 204, b""),
        ("return {status:200,body_base64:'!'}", 500, b""),
        ("return {status:99,body:''}", 500, b""),
        ("return {status:200,body:'',body_base64:''}", 500, b""),
        ("return {status:200,headers:{connection:'close'},body:''}", 500, b""),
        ("return {status:200,headers:{x:'bad\\r\\nheader'},body:''}", 500, b""),
        ("return {status:200,body:'x'.repeat(1048577)}", 500, b""),
        ("throw new Error('private-error-marker')", 500, b""),
        ("while (true) {}", 504, b""),
    ]
    for index, (code, status, body) in enumerate(cases):
        name = "result" + str(index)
        assert_status(call("PUT", management + "/" + name,
                           definition("async function handler() { " + code + " }"),
                           headers={"If-None-Match": "*"}), 200)
        result = assert_status(call("POST", "/functions/v1/" + name), status)
        assert result["body"] == body, result
    invalid = definition("function handler( {")
    assert_status(call("PUT", management + "/invalid", invalid,
                       headers={"If-None-Match": "*"}), 400)

with subtest("fresh invocations retain the ordinary Engine's fetch policy and header injection"):
    policy_source = """
        async function handler() {
            const allowed = await (await fetch('http://127.0.0.1:8081/allowed')).json();
            let denied = false;
            try { await fetch('http://127.0.0.1:8081/denied'); }
            catch (error) { denied = error.message.includes('denied'); }
            globalThis.counter = (globalThis.counter || 0) + 1;
            return {status:200,body:JSON.stringify({allowed,denied,counter:globalThis.counter})};
        }
    """
    assert_status(call("PUT", management + "/policy", definition(policy_source),
                       headers={"If-None-Match": "*"}), 200)
    for _ in range(2):
        result = json.loads(assert_status(call("POST", "/functions/v1/policy"), 200)["body"])
        assert result == {"allowed": {"path": "/allowed", "injected": "server-injected-test-value"},
                          "denied": True, "counter": 1}, result

with subtest("conditional replacements and tombstones survive service restart and VM reboot"):
    machine.succeed("systemctl restart mcp-js.service")
    machine.wait_for_unit("mcp-js.service")
    machine.wait_for_open_port(3000)
    assert assert_status(call("GET", management + "/echo"), 200)["headers"]["etag"] == revision
    replacement = definition("function handler() { return {status:200,body:'replacement'}; }")
    assert_status(call("PUT", management + "/echo", replacement,
                       headers={"If-Match": '"stale"'}), 412)
    updated = assert_status(call("PUT", management + "/echo", replacement,
                                 headers={"If-Match": revision}), 200)
    new_revision = updated["headers"]["etag"]
    assert new_revision != revision
    assert assert_status(call("POST", "/functions/v1/echo"), 200)["body"] == b"replacement"
    assert_status(call("DELETE", management + "/echo", headers={"If-Match": new_revision}), 200)
    machine.reboot()
    machine.wait_for_unit("mcp-js.service")
    machine.wait_for_open_port(3000)
    assert_status(call("POST", "/functions/v1/echo"), 404)
    assert_status(call("PUT", management + "/echo", replacement,
                       headers={"If-Match": new_revision}), 412)
    recreated = assert_status(call("PUT", management + "/echo", replacement,
                                   headers={"If-None-Match": "*"}), 200)
    assert recreated["headers"]["etag"] not in [revision, new_revision]
    assert_status(call("DELETE", management + "/echo", headers={"If-Match": new_revision}), 412)
