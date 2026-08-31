//! Loopback-only TCP capability: node:net / node:http servers and clients.
//!
//! Off by default — without a NetTcpConfig the shims keep their inert,
//! capability-model-explaining behavior. With it, traffic works but only
//! over the loopback interface.

use std::sync::Once;

use server::engine::{
    ExecutionConfig, execute_stateless, initialize_v8, net_tcp::NetTcpConfig,
};

static INIT: Once = Once::new();

fn run_with_net(code: &str) -> Result<String, String> {
    INIT.call_once(initialize_v8);
    let (result, _) = execute_stateless(
        code,
        ExecutionConfig::new(64 * 1024 * 1024)
            .maybe_net_tcp_config(Some(NetTcpConfig::default())),
    );
    result
}

fn run_without_net(code: &str) -> Result<String, String> {
    INIT.call_once(initialize_v8);
    let (result, _) = execute_stateless(code, ExecutionConfig::new(64 * 1024 * 1024));
    result
}

#[test]
fn net_disabled_by_default() {
    let result = run_without_net(
        r#"
        const net = await import('node:net');
        const http = await import('node:http');
        let threw = 0;
        try { net.createServer(); } catch (e) {
            if (/not supported/.test(e.message)) threw++;
        }
        try { http.createServer(); } catch (e) {
            if (/not supported/.test(e.message)) threw++;
        }
        if (threw !== 2) throw new Error('expected capability errors, got ' + threw);
        "#,
    );
    assert!(result.is_ok(), "disabled-mode contract failed: {result:?}");
}

#[test]
fn net_echo_round_trip() {
    let result = run_with_net(
        r#"
        const net = await import('node:net');
        const echoed = await new Promise((resolve, reject) => {
            const server = net.createServer((socket) => {
                socket.on('data', (data) => socket.end(data));
            });
            server.listen(0, () => {
                const port = server.address().port;
                if (!(port > 0)) return reject(new Error('bad port'));
                const client = net.connect(port, '127.0.0.1', () => {
                    client.write('ping');
                });
                let received = '';
                client.on('data', (data) => { received += data; });
                client.on('close', () => server.close(() => resolve(received)));
                client.on('error', reject);
            });
        });
        if (echoed !== 'ping') throw new Error('echo mismatch: ' + echoed);
        "#,
    );
    assert!(result.is_ok(), "echo round trip failed: {result:?}");
}

#[test]
fn http_server_and_client_round_trip() {
    let result = run_with_net(
        r#"
        const http = await import('node:http');
        const outcome = await new Promise((resolve, reject) => {
            const server = http.createServer((req, res) => {
                let body = '';
                req.on('data', (chunk) => { body += chunk; });
                req.on('end', () => {
                    res.writeHead(201, { 'X-Echo': req.headers['x-probe'] });
                    res.end('got:' + req.method + ':' + req.url + ':' + body);
                });
            });
            server.listen(0, () => {
                const request = http.request({
                    port: server.address().port,
                    method: 'POST',
                    path: '/target',
                    headers: { 'X-Probe': 'v1' },
                }, (res) => {
                    let body = '';
                    res.on('data', (chunk) => { body += chunk; });
                    res.on('end', () => server.close(() => resolve({
                        status: res.statusCode,
                        echo: res.headers['x-echo'],
                        body,
                    })));
                });
                request.on('error', reject);
                request.end('payload');
            });
        });
        if (outcome.status !== 201) throw new Error('status: ' + outcome.status);
        if (outcome.echo !== 'v1') throw new Error('header echo: ' + outcome.echo);
        if (outcome.body !== 'got:POST:/target:payload') {
            throw new Error('body: ' + outcome.body);
        }
        "#,
    );
    assert!(result.is_ok(), "http round trip failed: {result:?}");
}

#[test]
fn non_loopback_addresses_are_refused() {
    let result = run_with_net(
        r#"
        const net = await import('node:net');
        // Wildcard binds are rewritten to loopback rather than exposed.
        const server = net.createServer();
        const bound = await new Promise((resolve, reject) => {
            server.on('error', reject);
            server.listen(0, '0.0.0.0', () => resolve(server.address().address));
        });
        if (bound !== '127.0.0.1') throw new Error('wildcard bind not pinned: ' + bound);
        await new Promise((resolve) => server.close(resolve));

        // Outbound connections to non-loopback addresses are refused.
        const error = await new Promise((resolve) => {
            const socket = net.connect(80, '93.184.216.34');
            socket.on('error', resolve);
        });
        if (!/loopback/.test(String(error.message))) {
            throw new Error('expected loopback refusal, got: ' + error.message);
        }
        "#,
    );
    assert!(result.is_ok(), "loopback pinning contract failed: {result:?}");
}
