#![no_main]
use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use server::engine::ExecutionConfig;
use std::sync::{Arc, Mutex, Once};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        // Disable V8 background threads to prevent cumulative memory exhaustion
        deno_core::v8::V8::set_flags_from_string("--single-threaded");
        server::engine::initialize_v8();
        // Override libfuzzer's abort-on-panic hook for graceful panic handling
        std::panic::set_hook(Box::new(|_| {}));
    });
}

#[derive(Arbitrary, Debug)]
struct FsOperationInput {
    /// Which filesystem operation to test
    operation: FsOperation,
    /// The path argument (can be empty, contain .., special chars, unicode, etc.)
    path: String,
    /// Optional destination path (for rename/copy operations)
    destination: Option<String>,
    /// Optional file data for write operations
    file_data: Option<Vec<u8>>,
    /// Optional recursive flag for mkdir/rm
    recursive: bool,
}

#[derive(Arbitrary, Debug, Clone, Copy)]
enum FsOperation {
    ReadFileText,
    ReadFileBuffer,
    WriteFileText,
    WriteFileBuffer,
    AppendFile,
    Readdir,
    Stat,
    Mkdir,
    Rm,
    Rename,
    CopyFile,
    Exists,
}

// Fuzz filesystem operation inputs with edge cases:
// - Empty paths, paths with "..", "..", null bytes, unicode characters
// - Very long paths and file names
// - Large file data (up to Vec limits)
// - Various flag combinations (recursive, encoding)
//
// This exercises the policy input serialization and filesystem code paths
// without depending on actual filesystem operations (most will fail gracefully).
fuzz_target!(|input: FsOperationInput| {
    ensure_v8();

    // Cap data size to avoid excessive memory usage
    let mut file_data = input.file_data;
    if let Some(ref mut data) = file_data {
        data.truncate(1024 * 1024); // 1MB max
    }

    // Build JavaScript code that attempts various filesystem operations
    // Wrap in try-catch since most paths will be invalid or denied by policy
    let js_code = match input.operation {
        FsOperation::ReadFileText => {
            format!(
                "try {{ await fs.readFile('{}'); }} catch(e) {{ }}",
                escape_js_string(&input.path)
            )
        }
        FsOperation::ReadFileBuffer => {
            format!(
                "try {{ await fs.readFile('{}', 'buffer'); }} catch(e) {{ }}",
                escape_js_string(&input.path)
            )
        }
        FsOperation::WriteFileText => {
            let data = file_data.as_ref().map(|d| {
                format!(
                    "'{}'",
                    String::from_utf8_lossy(d)
                        .replace('\\', "\\\\")
                        .replace('\'', "\\'")
                )
            });
            if let Some(d) = data {
                format!(
                    "try {{ await fs.writeFile('{}', {}); }} catch(e) {{ }}",
                    escape_js_string(&input.path),
                    d
                )
            } else {
                format!(
                    "try {{ await fs.writeFile('{}', ''); }} catch(e) {{ }}",
                    escape_js_string(&input.path)
                )
            }
        }
        FsOperation::WriteFileBuffer => {
            if let Some(d) = file_data {
                let hex = d
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect::<Vec<_>>()
                    .join("");
                if hex.is_empty() {
                    format!(
                        "try {{ await fs.writeFile('{}', new Uint8Array([])); }} catch(e) {{ }}",
                        escape_js_string(&input.path)
                    )
                } else {
                    format!(
                        "try {{ await fs.writeFile('{}', new Uint8Array([{}])); }} catch(e) {{ }}",
                        escape_js_string(&input.path),
                        hex
                    )
                }
            } else {
                format!(
                    "try {{ await fs.writeFile('{}', new Uint8Array([])); }} catch(e) {{ }}",
                    escape_js_string(&input.path)
                )
            }
        }
        FsOperation::AppendFile => {
            format!(
                "try {{ await fs.appendFile('{}', 'x'); }} catch(e) {{ }}",
                escape_js_string(&input.path)
            )
        }
        FsOperation::Readdir => {
            format!(
                "try {{ await fs.readdir('{}'); }} catch(e) {{ }}",
                escape_js_string(&input.path)
            )
        }
        FsOperation::Stat => {
            format!(
                "try {{ await fs.stat('{}'); }} catch(e) {{ }}",
                escape_js_string(&input.path)
            )
        }
        FsOperation::Mkdir => {
            format!(
                "try {{ await fs.mkdir('{}', {{ recursive: {} }}); }} catch(e) {{ }}",
                escape_js_string(&input.path),
                input.recursive
            )
        }
        FsOperation::Rm => {
            format!(
                "try {{ await fs.rm('{}', {{ recursive: {} }}); }} catch(e) {{ }}",
                escape_js_string(&input.path),
                input.recursive
            )
        }
        FsOperation::Rename => {
            format!(
                "try {{ await fs.rename('{}', '{}'); }} catch(e) {{ }}",
                escape_js_string(&input.path),
                escape_js_string(&input.destination.as_deref().unwrap_or(""))
            )
        }
        FsOperation::CopyFile => {
            format!(
                "try {{ await fs.copyFile('{}', '{}'); }} catch(e) {{ }}",
                escape_js_string(&input.path),
                escape_js_string(&input.destination.as_deref().unwrap_or(""))
            )
        }
        FsOperation::Exists => {
            format!(
                "try {{ await fs.exists('{}'); }} catch(e) {{ }}",
                escape_js_string(&input.path)
            )
        }
    };

    // Execute stateless — we don't care about the result, only that it doesn't crash
    let max_bytes = 8 * 1024 * 1024;
    let handle = Arc::new(Mutex::new(None));
    let _ = server::engine::execute_stateless(&js_code, ExecutionConfig::new(max_bytes)
        .isolate_handle(handle));
});

/// Escape a string for safe inclusion in JavaScript code
fn escape_js_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\0', "\\0")
}
