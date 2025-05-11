use std::io::Write;

fn eval<'s>(scope: &mut v8::HandleScope<'s>, code: &str) -> Result<v8::Local<'s, v8::Value>, String> {
    let scope = &mut v8::EscapableHandleScope::new(scope);
    let source = v8::String::new(scope, code).ok_or("Failed to create V8 string")?;
    let script = v8::Script::compile(scope, source, None).ok_or("Failed to compile script")?;
    let r = script.run(scope).ok_or("Failed to run script")?;
    Ok(scope.escape(r))
}

fn main() {
    let platform = v8::new_default_platform(0, false).make_shared();
    v8::V8::initialize_platform(platform);
    v8::V8::initialize();

    // Create snapshot
    let startup_data = {
        let mut snapshot_creator = match std::fs::read("snapshot.bin") {
            Ok(snapshot) => {
                eprintln!("creating isolate from snapshot...");
                v8::Isolate::snapshot_creator_from_existing_snapshot(snapshot, None, None)
            }
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    eprintln!("snapshot file not found, creating new isolate...");
                    v8::Isolate::snapshot_creator(Default::default(), Default::default())
                } else {
                    eprintln!("error creating isolate: {}", e);
                    return;
                }
            }
        };
        {
            let scope = &mut v8::HandleScope::new(&mut snapshot_creator);
            let context = v8::Context::new(scope, Default::default());
            let scope = &mut v8::ContextScope::new(scope, context);
            let out = match eval(
                scope,
                "
try {
 x = x + 1
} catch (e) {
  x = 1
}
x;
                ",
            ) {
                Ok(val) => val,
                Err(e) => {
                    eprintln!("eval error: {}", e);
                    return;
                }
            };
            let out_str = match out.to_string(scope) {
                Some(s) => s.to_rust_string_lossy(scope),
                None => {
                    eprintln!("Failed to convert result to string");
                    return;
                }
            };
            eprintln!(
                "x = {}",
                out_str
            );
            scope.set_default_context(context);
        }
        match snapshot_creator
            .create_blob(v8::FunctionCodeHandling::Clear) {
            Some(blob) => blob,
            None => {
                eprintln!("Failed to create V8 snapshot blob");
                return;
            }
        }
    };

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
    if let Err(e) = file.write_all(&startup_data) {
        eprintln!("Failed to write snapshot data: {}", e);
    }
}
