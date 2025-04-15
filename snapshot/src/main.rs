use std::io::Write;

fn eval<'s>(scope: &mut v8::HandleScope<'s>, code: &str) -> Option<v8::Local<'s, v8::Value>> {
    let scope = &mut v8::EscapableHandleScope::new(scope);
    let source = v8::String::new(scope, code).unwrap();
    let script = v8::Script::compile(scope, source, None).unwrap();
    let r = script.run(scope);
    r.map(|v| scope.escape(v))
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
            let out = eval(
                scope,
                "
try {
 x = x + 1
} catch (e) {
  x = 1
}
x;
            ",
            )
            .unwrap();
            eprintln!(
                "x = {}",
                out.to_string(scope).unwrap().to_rust_string_lossy(scope)
            );

            scope.set_default_context(context);
        }
        snapshot_creator
            .create_blob(v8::FunctionCodeHandling::Clear)
            .unwrap()
    };

    // Write snapshot to file
    eprintln!("snapshot created");
    eprintln!("writing snapshot to file snapshot.bin in current directory");
    let mut file = std::fs::File::create("snapshot.bin").unwrap();
    file.write_all(&startup_data).unwrap();
}
