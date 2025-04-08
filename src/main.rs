use log::info;

use mcpr::{
    error::MCPError,
    schema::common::{Tool, ToolInputSchema},
    server::{Server, ServerConfig},
    transport::stdio::StdioTransport,
};
use serde_json::{json, Value};
use v8::OwnedIsolate;
use std::{collections::HashMap, io::Write};

fn main() -> Result<(), MCPError> {
    // Initialize logging
    env_logger::init_from_env(
        env_logger::Env::default().filter_or(env_logger::DEFAULT_FILTER_ENV, "info"),
    );

    // Create a transport
    let transport = StdioTransport::new();

    // Create a hello tool
    let hello_tool = Tool {
        name: "javascript".to_string(),
        description: Some("runs javascript".to_string()),
        input_schema: ToolInputSchema {
            r#type: "object".to_string(),
            properties: Some(
                [(
                    "code".to_string(),
                    json!({
                        "type": "string",
                        "description": "code to run"
                    }),
                )]
                .into_iter()
                .collect::<HashMap<_, _>>(),
            ),
            required: Some(vec!["code".to_string()]),
        },
    };

    // Configure the server
    let server_config = ServerConfig::new()
        .with_name("Echo Server")
        .with_version("1.0.0")
        .with_tool(hello_tool);

    // Create the server
    let mut server = Server::new(server_config);

    // Initialize V8 once at the start
    let platform = v8::new_default_platform(0, false).make_shared();
    v8::V8::initialize_platform(platform);
    v8::V8::initialize();

    server
        .register_tool_handler("javascript", |params| {
            let code = params.get("code").unwrap().as_str().unwrap();

            let mut isolate : OwnedIsolate;

            // Load snapshot if it exists
            if let Ok(snapshot) = std::fs::read("snapshot.bin") {
                isolate = v8::Isolate::snapshot_creator_from_existing_snapshot(snapshot, None, None);

            } else {
                isolate = v8::Isolate::new(Default::default());

            }
            
            // Create a new isolate for each execution
            let mut string_result = String::new();
            
            {
                // Create a scope within a new block to ensure it's dropped before we return
                let handle_scope = &mut v8::HandleScope::new(&mut isolate);
                let context = v8::Context::new(handle_scope, Default::default());
                let scope = &mut v8::ContextScope::new(handle_scope, context);

                let v8_string_code = v8::String::new(scope, code).unwrap();
                let script = match v8::Script::compile(scope, v8_string_code, None) {
                    Some(script) => script,
                    None => {
                        return Ok(json!({
                            "error": "Failed to compile JavaScript code"
                        }))
                    }
                };

                let result = match script.run(scope) {
                    Some(result) => result,
                    None => {
                        return Ok(json!({
                            "error": "Failed to run JavaScript code"
                        }))
                    }
                };

                let result_string = result.to_string(scope).unwrap();
                string_result = result_string.to_rust_string_lossy(scope);
            }
            
            // All scopes are dropped by here, isolate is no longer borrowed
            // cleanup
            let snapshot = isolate.create_blob(v8::FunctionCodeHandling::Keep).unwrap();
            let mut file = std::fs::File::create("snapshot.bin").unwrap();
            file.write_all(&snapshot).unwrap();

            // save snapshot to file
            // We could create a snapshot here if needed, but it's not necessary for each execution
            // Let's just return the result
            Ok(json!({
                "result": string_result
            }))
        })
        .unwrap();

    // Start the server
    info!("Starting server...");
    server.start(transport).unwrap();

    Ok(())
}