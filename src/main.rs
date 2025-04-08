use log::info;

use mcpr::{
    error::MCPError,
    schema::common::{Tool, ToolInputSchema},
    server::{Server, ServerConfig},
    transport::stdio::StdioTransport,
};
use serde_json::{json, Value};
use std::collections::HashMap;

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

    server.register_tool_handler("javascript", |params| {

        let code =Value::String(params.get("code").unwrap().to_string()).to_string();
        let platform = v8::new_default_platform(0, false).make_shared();
        v8::V8::initialize_platform(platform);
        v8::V8::initialize();
        
        let isolate = &mut v8::Isolate::snapshot_creator(
            None,
            None
        );
        
        let scope = &mut v8::HandleScope::new(isolate);
        let context = v8::Context::new(scope, Default::default());
        let scope = &mut v8::ContextScope::new(scope, context);
    

        let v8_string_code = v8::String::new(scope, &code).unwrap();


        
        
        let script = v8::Script::compile(scope, v8_string_code, None).unwrap();
        let result = script.run(scope).unwrap();
        let result = result.to_string(scope).unwrap();
        println!("result: {}", result.to_rust_string_lossy(scope));

        let string_result = result.to_rust_string_lossy(scope);

        // drop scope

        // cleanup 
        let snapshot = isolate.create_blob(
            v8::FunctionCodeHandling::Keep,
        ).unwrap();

        // save snapshot to file
        let mut file = std::fs::File::create("snapshot.bin").unwrap();




        Ok(json!({
            "result": string_result
        }))
    }).unwrap();
    
    // Start the server
    info!("Starting server...");
    server.start(transport).unwrap();

    Ok(())
}