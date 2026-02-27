use std::io::Write;
use deno_core::{JsRuntimeForSnapshot, RuntimeOptions};
use deno_core::v8;

fn main() {
    // Box::leak pattern: RuntimeOptions::startup_snapshot requires &'static [u8].
    // We leak the dynamically-loaded snapshot data, then reclaim after snapshot()
    // consumes the runtime.
    let leaked_snapshot: Option<(*mut [u8], &'static [u8])> =
        match std::fs::read("snapshot.bin") {
            Ok(snapshot) => {
                eprintln!("creating isolate from snapshot...");
                let ptr = Box::into_raw(snapshot.into_boxed_slice());
                let static_ref: &'static [u8] = unsafe { &*ptr };
                Some((ptr, static_ref))
            }
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    eprintln!("snapshot file not found, creating new isolate...");
                    None
                } else {
                    eprintln!("error: {}", e);
                    return;
                }
            }
        };

    let startup_snapshot = leaked_snapshot.as_ref().map(|(_, s)| *s);

    let mut runtime = JsRuntimeForSnapshot::new(RuntimeOptions {
        startup_snapshot,
        ..Default::default()
    });

    // Execute script and stringify result before consuming runtime for snapshot.
    let code = r#"
try {
  x = x + 1
} catch (e) {
  x = 1
}
x;
"#;

    match runtime.execute_script("<eval>", code.to_string()) {
        Ok(global_value) => {
            deno_core::scope!(scope, runtime);
            let local = v8::Local::new(scope, global_value);
            match local.to_string(scope) {
                Some(s) => eprintln!("x = {}", s.to_rust_string_lossy(scope)),
                None => {
                    eprintln!("Failed to convert result to string");
                    return;
                }
            }
        }
        Err(e) => {
            eprintln!("eval error: {}", e);
            return;
        }
    }

    // Consume runtime to create snapshot.
    let snapshot_data = runtime.snapshot();

    // Reclaim leaked input snapshot memory (safe: runtime is consumed).
    if let Some((ptr, _)) = leaked_snapshot {
        unsafe { let _ = Box::from_raw(ptr); }
    }

    // Write snapshot to file
    eprintln!("snapshot created");
    eprintln!("writing snapshot to file snapshot.bin in current directory");
    let mut file = match std::fs::File::create("snapshot.bin") {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Failed to create snapshot.bin: {}", e);
            return;
        }
    };
    if let Err(e) = file.write_all(&snapshot_data) {
        eprintln!("Failed to write snapshot data: {}", e);
    }
}
