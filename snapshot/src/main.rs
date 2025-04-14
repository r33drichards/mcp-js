


use log::info;


use std::io::Write;
use v8::OwnedIsolate;

fn main() {

    let code = "x=1;x+=1;x"

    let mut isolate: OwnedIsolate;

    info!("Creating isolate...");

    // Load snapshot if it exists
    if let Ok(snapshot) = std::fs::read("snapshot.bin") {
        info!("creating isolate from snapshot...");
        isolate = v8::Isolate::snapshot_creator_from_existing_snapshot(snapshot, None, None);
    } else {
        info!("creating isolate from scratch...");
        isolate = v8::Isolate::snapshot_creator(Default::default(), Default::default());
    }

    info!("Isolate created");

    // Create a new isolate for each execution
    let mut string_result = String::new();

    info!("Creating scope...");
    let handle_scope = &mut v8::HandleScope::new(&mut isolate);
    let context = v8::Context::new(handle_scope, Default::default());
    let scope = &mut v8::ContextScope::new(handle_scope, context);

    info!("Scope created");
    info!("creating v8 string from code...");

    let v8_string_code = v8::String::new(scope, code).unwrap();
    let script = match v8::Script::compile(scope, v8_string_code, None) {
        Some(script) => script,
        None => {
            info!("Failed to compile JavaScript code");
            return;
        }
    };
    info!("v8 string created");
    info!("running script...");
    let result = match script.run(scope) {
        Some(result) => result,
        None => {
            info!("Failed to run JavaScript code");
            return;
        }
    };
    info!("script ran");

    let result_string = result.to_string(scope).unwrap();
    string_result = result_string.to_rust_string_lossy(scope);
    info!("result: {}", string_result);

    // All scopes are dropped by here, isolate is no longer borrowed
    // cleanup
    info!("creating snapshot");

    let snapshot = match isolate.create_blob(v8::FunctionCodeHandling::Keep) {
        Some(snapshot) => snapshot,
        None => {
            info!("Failed to create snapshot");
            return;
        }
    };

    info!("snapshot created");
    info!("writing snapshot to file snapshot.bin in current directory");
    let mut file = std::fs::File::create("snapshot.bin").unwrap();
    file.write_all(&snapshot).unwrap();

}
