/// Tests for atob/btoa base64 globals and FormData/Blob/File polyfills.

use std::sync::{Arc, Once};
use server::engine::{initialize_v8, Engine};
use server::engine::execution::ExecutionRegistry;
use server::mcp::StatelessMcpService;

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        initialize_v8();
    });
}

fn rand_id() -> u64 {
    use std::time::SystemTime;
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos() as u64
}

fn create_test_engine() -> Engine {
    let tmp = std::env::temp_dir().join(format!("mcp-base64-test-{}-{}", std::process::id(), rand_id()));
    let registry = ExecutionRegistry::new(tmp.to_str().unwrap()).expect("Failed to create test registry");
    Engine::new_stateless(8 * 1024 * 1024, 30, 4)
        .with_execution_registry(Arc::new(registry))
}

use server::mcp::StatelessRunJsResponse;

fn parse_response(resp: StatelessRunJsResponse) -> serde_json::Value {
    resp.value
}

fn assert_output_contains(value: &serde_json::Value, expected: &str) {
    assert!(value["error"].is_null(), "Should not have error: {:?}", value);
    let output = value["output"].as_str().expect("Should have output field");
    assert!(output.contains(expected), "Expected output to contain '{}', got: {}", expected, output);
}

// ── btoa tests ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_btoa_basic() {
    ensure_v8();
    let engine = create_test_engine();
    let service = StatelessMcpService::new(engine, None);

    let resp = service.run_js("console.log(btoa('hello'))".to_string(), None, None).await;
    let value = parse_response(resp);
    assert_output_contains(&value, "aGVsbG8=");
}

#[tokio::test]
async fn test_btoa_empty() {
    ensure_v8();
    let engine = create_test_engine();
    let service = StatelessMcpService::new(engine, None);

    let resp = service.run_js("console.log(btoa(''))".to_string(), None, None).await;
    let value = parse_response(resp);
    let output = value["output"].as_str().unwrap();
    assert!(output.trim().is_empty() || output.contains(""), "btoa('') should return empty string");
}

#[tokio::test]
async fn test_btoa_binary_chars() {
    ensure_v8();
    let engine = create_test_engine();
    let service = StatelessMcpService::new(engine, None);

    // Test with bytes 0-255 (Latin1 range)
    let resp = service.run_js(r#"console.log(btoa('\x00\x01\xff'))"#.to_string(), None, None).await;
    let value = parse_response(resp);
    assert_output_contains(&value, "AAH/");
}

#[tokio::test]
async fn test_btoa_rejects_non_latin1() {
    ensure_v8();
    let engine = create_test_engine();
    let service = StatelessMcpService::new(engine, None);

    let resp = service.run_js(r#"
        try { btoa('Ā'); console.log('NO ERROR'); }
        catch(e) { console.log('CAUGHT: ' + e.name + ': ' + e.message); }
    "#.to_string(), None, None).await;
    let value = parse_response(resp);
    assert_output_contains(&value, "CAUGHT:");
    assert_output_contains(&value, "Latin1");
}

// ── atob tests ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_atob_basic() {
    ensure_v8();
    let engine = create_test_engine();
    let service = StatelessMcpService::new(engine, None);

    let resp = service.run_js("console.log(atob('aGVsbG8='))".to_string(), None, None).await;
    let value = parse_response(resp);
    assert_output_contains(&value, "hello");
}

#[tokio::test]
async fn test_atob_no_padding() {
    ensure_v8();
    let engine = create_test_engine();
    let service = StatelessMcpService::new(engine, None);

    let resp = service.run_js("console.log(atob('aGVsbG8'))".to_string(), None, None).await;
    let value = parse_response(resp);
    assert_output_contains(&value, "hello");
}

#[tokio::test]
async fn test_atob_rejects_invalid() {
    ensure_v8();
    let engine = create_test_engine();
    let service = StatelessMcpService::new(engine, None);

    let resp = service.run_js(r#"
        try { atob('!!!!'); console.log('NO ERROR'); }
        catch(e) { console.log('CAUGHT: ' + e.message); }
    "#.to_string(), None, None).await;
    let value = parse_response(resp);
    assert_output_contains(&value, "CAUGHT:");
}

#[tokio::test]
async fn test_btoa_atob_roundtrip() {
    ensure_v8();
    let engine = create_test_engine();
    let service = StatelessMcpService::new(engine, None);

    let resp = service.run_js(r#"
        var original = 'The quick brown fox jumps over the lazy dog';
        var encoded = btoa(original);
        var decoded = atob(encoded);
        console.log(decoded === original ? 'ROUNDTRIP_OK' : 'MISMATCH');
    "#.to_string(), None, None).await;
    let value = parse_response(resp);
    assert_output_contains(&value, "ROUNDTRIP_OK");
}

// ── Blob tests ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_blob_basic() {
    ensure_v8();
    let engine = create_test_engine();
    let service = StatelessMcpService::new(engine, None);

    let resp = service.run_js(r#"
        var b = new Blob(['hello ', 'world'], { type: 'text/plain' });
        console.log(b.size + '|' + b.type);
    "#.to_string(), None, None).await;
    let value = parse_response(resp);
    assert_output_contains(&value, "11|text/plain");
}

#[tokio::test]
async fn test_blob_text() {
    ensure_v8();
    let engine = create_test_engine();
    let service = StatelessMcpService::new(engine, None);

    let resp = service.run_js(r#"
        (async () => {
            var b = new Blob(['abc', 'def']);
            console.log(await b.text());
        })();
    "#.to_string(), None, None).await;
    let value = parse_response(resp);
    assert_output_contains(&value, "abcdef");
}

#[tokio::test]
async fn test_blob_slice() {
    ensure_v8();
    let engine = create_test_engine();
    let service = StatelessMcpService::new(engine, None);

    let resp = service.run_js(r#"
        (async () => {
            var b = new Blob(['hello world']);
            var sliced = b.slice(0, 5);
            console.log(await sliced.text());
        })();
    "#.to_string(), None, None).await;
    let value = parse_response(resp);
    assert_output_contains(&value, "hello");
}

// ── File tests ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_file_basic() {
    ensure_v8();
    let engine = create_test_engine();
    let service = StatelessMcpService::new(engine, None);

    let resp = service.run_js(r#"
        var f = new File(['content'], 'test.txt', { type: 'text/plain' });
        console.log(f.name + '|' + f.size + '|' + f.type + '|' + (f instanceof Blob));
    "#.to_string(), None, None).await;
    let value = parse_response(resp);
    assert_output_contains(&value, "test.txt|7|text/plain|true");
}

// ── FormData tests ──────────────────────────────────────────────────────────

#[tokio::test]
async fn test_formdata_append_get() {
    ensure_v8();
    let engine = create_test_engine();
    let service = StatelessMcpService::new(engine, None);

    let resp = service.run_js(r#"
        var fd = new FormData();
        fd.append('name', 'alice');
        fd.append('name', 'bob');
        console.log(fd.get('name') + '|' + fd.getAll('name').join(','));
    "#.to_string(), None, None).await;
    let value = parse_response(resp);
    assert_output_contains(&value, "alice|alice,bob");
}

#[tokio::test]
async fn test_formdata_set_replaces() {
    ensure_v8();
    let engine = create_test_engine();
    let service = StatelessMcpService::new(engine, None);

    let resp = service.run_js(r#"
        var fd = new FormData();
        fd.append('x', '1');
        fd.append('x', '2');
        fd.set('x', '3');
        console.log(fd.getAll('x').join(','));
    "#.to_string(), None, None).await;
    let value = parse_response(resp);
    assert_output_contains(&value, "3");
}

#[tokio::test]
async fn test_formdata_has_delete() {
    ensure_v8();
    let engine = create_test_engine();
    let service = StatelessMcpService::new(engine, None);

    let resp = service.run_js(r#"
        var fd = new FormData();
        fd.append('key', 'val');
        var before = fd.has('key');
        fd.delete('key');
        var after = fd.has('key');
        console.log(before + '|' + after);
    "#.to_string(), None, None).await;
    let value = parse_response(resp);
    assert_output_contains(&value, "true|false");
}

#[tokio::test]
async fn test_formdata_serialize_text() {
    ensure_v8();
    let engine = create_test_engine();
    let service = StatelessMcpService::new(engine, None);

    let resp = service.run_js(r#"
        var fd = new FormData();
        fd.append('field', 'value');
        var s = fd._serialize();
        var ok = s.body.includes('Content-Disposition: form-data; name="field"')
              && s.body.includes('value')
              && s.body.includes('--' + s.boundary + '--');
        console.log(ok ? 'SERIALIZE_OK' : 'FAIL: ' + s.body);
    "#.to_string(), None, None).await;
    let value = parse_response(resp);
    assert_output_contains(&value, "SERIALIZE_OK");
}

#[tokio::test]
async fn test_formdata_serialize_blob_with_filename() {
    ensure_v8();
    let engine = create_test_engine();
    let service = StatelessMcpService::new(engine, None);

    let resp = service.run_js(r#"
        var fd = new FormData();
        fd.append('f', new Blob(['file data'], { type: 'text/plain' }), 'upload.txt');
        var s = fd._serialize();
        var ok = s.body.includes('filename="upload.txt"')
              && s.body.includes('Content-Type: text/plain')
              && s.body.includes('file data');
        console.log(ok ? 'BLOB_OK' : 'FAIL: ' + s.body);
    "#.to_string(), None, None).await;
    let value = parse_response(resp);
    assert_output_contains(&value, "BLOB_OK");
}

#[tokio::test]
async fn test_formdata_serialize_file() {
    ensure_v8();
    let engine = create_test_engine();
    let service = StatelessMcpService::new(engine, None);

    let resp = service.run_js(r#"
        var fd = new FormData();
        fd.append('doc', new File(['csv,data'], 'data.csv', { type: 'text/csv' }));
        var s = fd._serialize();
        var ok = s.body.includes('filename="data.csv"')
              && s.body.includes('Content-Type: text/csv')
              && s.body.includes('csv,data');
        console.log(ok ? 'FILE_OK' : 'FAIL: ' + s.body);
    "#.to_string(), None, None).await;
    let value = parse_response(resp);
    assert_output_contains(&value, "FILE_OK");
}
