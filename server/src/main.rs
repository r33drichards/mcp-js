//! MCP Server for mcp-v8 project with stdio transport

use clap::Parser;
use mcpr::{
    error::MCPError,
    schema::common::{Tool, ToolInputSchema},
    transport::{stdio::StdioTransport, Transport},
};
use serde_json::{json, Value};

use std::collections::HashMap;
use std::error::Error;

use std::io::Write;

fn eval<'s>(scope: &mut v8::HandleScope<'s>, code: &str) -> Option<v8::Local<'s, v8::Value>> {
    let scope = &mut v8::EscapableHandleScope::new(scope);
    let source = v8::String::new(scope, code).unwrap();
    let script = v8::Script::compile(scope, source, None).unwrap();
    let r = script.run(scope);
    r.map(|v| scope.escape(v))
}

/// CLI arguments
#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Enable debug output
    #[arg(short, long)]
    debug: bool,
}

/// Server configuration
struct ServerConfig {
    /// Server name
    name: String,
    /// Server version
    version: String,
    /// Available tools
    tools: Vec<Tool>,
}

impl ServerConfig {
    /// Create a new server configuration
    fn new() -> Self {
        Self {
            name: "MCP Server".to_string(),
            version: "1.0.0".to_string(),
            tools: Vec::new(),
        }
    }

    /// Set the server name
    fn with_name(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }

    /// Set the server version
    fn with_version(mut self, version: &str) -> Self {
        self.version = version.to_string();
        self
    }

    /// Add a tool to the server
    fn with_tool(mut self, tool: Tool) -> Self {
        self.tools.push(tool);
        self
    }
}

/// Tool handler function type
type ToolHandler = Box<dyn Fn(Value) -> Result<Value, MCPError> + Send + Sync>;

/// High-level MCP server
struct Server<T> {
    config: ServerConfig,
    tool_handlers: HashMap<String, ToolHandler>,
    transport: Option<T>,
}

impl<T> Server<T>
where
    T: Transport,
{
    /// Create a new MCP server with the given configuration
    fn new(config: ServerConfig) -> Self {
        Self {
            config,
            tool_handlers: HashMap::new(),
            transport: None,
        }
    }

    /// Register a tool handler
    fn register_tool_handler<F>(&mut self, tool_name: &str, handler: F) -> Result<(), MCPError>
    where
        F: Fn(Value) -> Result<Value, MCPError> + Send + Sync + 'static,
    {
        // Check if the tool exists in the configuration
        if !self.config.tools.iter().any(|t| t.name == tool_name) {
            return Err(MCPError::Protocol(format!(
                "Tool '{}' not found in server configuration",
                tool_name
            )));
        }

        // Register the handler
        self.tool_handlers
            .insert(tool_name.to_string(), Box::new(handler));

        eprintln!("Registered handler for tool '{}'", tool_name);
        Ok(())
    }

    /// Start the server with the given transport
    fn start(&mut self, mut transport: T) -> Result<(), MCPError> {
        // Start the transport
        eprintln!("Starting transport...");
        transport.start()?;

        // Store the transport
        self.transport = Some(transport);

        // Process messages
        eprintln!("Processing messages...");
        self.process_messages()
    }

    /// Process incoming messages
    fn process_messages(&mut self) -> Result<(), MCPError> {
        eprintln!("Server is running and waiting for client connections...");

        loop {
            let message = {
                let transport = self
                    .transport
                    .as_mut()
                    .ok_or_else(|| MCPError::Protocol("Transport not initialized".to_string()))?;

                // Receive a message
                match transport.receive() {
                    Ok(msg) => msg,
                    Err(e) => {
                        // For transport errors, log them but continue waiting
                        // This allows the server to keep running even if there are temporary connection issues
                        eprintln!("Transport error: {}", e);
                        std::thread::sleep(std::time::Duration::from_millis(1000));
                        continue;
                    }
                }
            };

            // Handle the message
            match message {
                mcpr::schema::json_rpc::JSONRPCMessage::Request(request) => {
                    let id = request.id.clone();
                    let method = request.method.clone();
                    let params = request.params.clone();

                    match method.as_str() {
                        "initialize" => {
                            eprintln!("Received initialization request");
                            let res = self.handle_initialize(id, params);
                            if let Err(e) = res {
                                eprintln!("Error sending initialization response: {}", e);
                            }
                        }
                        "tool_call" => {
                            eprintln!("Received tool call request");
                            self.handle_tool_call(id, params)?;
                        }
                        "shutdown" => {
                            eprintln!("Received shutdown request");
                            self.handle_shutdown(id)?;
                            break;
                        }
                        _ => {
                            eprintln!("Unknown method: {}", method);
                            self.send_error(
                                id,
                                -32601,
                                format!("Method not found: {}", method),
                                None,
                            )?;
                        }
                    }
                }
                _ => {
                    eprintln!("Unexpected message type");
                    continue;
                }
            }
        }

        Ok(())
    }

    /// Handle initialization request
    fn handle_initialize(
        &mut self,
        id: mcpr::schema::json_rpc::RequestId,
        _params: Option<Value>,
    ) -> Result<(), MCPError> {
        let transport = self
            .transport
            .as_mut()
            .ok_or_else(|| MCPError::Protocol("Transport not initialized".to_string()))?;

        // Create initialization response
        let response = mcpr::schema::json_rpc::JSONRPCResponse::new(
            id,
            serde_json::json!({
                "protocol_version": mcpr::constants::LATEST_PROTOCOL_VERSION,
                "server_info": {
                    "name": self.config.name,
                    "version": self.config.version
                },
                "capabilities": {
                    "tools": self.config.tools
                }
            }),
        );

        // Send the response
        eprintln!("Sending initialization response {:?}", response);
        transport.send(&mcpr::schema::json_rpc::JSONRPCMessage::Response(response))
    }

    /// Handle tool call request
    fn handle_tool_call(
        &mut self,
        id: mcpr::schema::json_rpc::RequestId,
        params: Option<Value>,
    ) -> Result<(), MCPError> {
        let transport = self
            .transport
            .as_mut()
            .ok_or_else(|| MCPError::Protocol("Transport not initialized".to_string()))?;

        // Extract tool name and parameters
        let params = params.ok_or_else(|| {
            MCPError::Protocol("Missing parameters in tool call request".to_string())
        })?;

        let tool_name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| MCPError::Protocol("Missing tool name in parameters".to_string()))?;

        let tool_params = params.get("parameters").cloned().unwrap_or(Value::Null);
        eprintln!(
            "Tool call: {} with parameters: {:?}",
            tool_name, tool_params
        );

        // Find the tool handler
        let handler = self.tool_handlers.get(tool_name).ok_or_else(|| {
            MCPError::Protocol(format!("No handler registered for tool '{}'", tool_name))
        })?;

        // Call the handler
        match handler(tool_params) {
            Ok(result) => {
                // Create tool result response
                eprintln!("creating tool result response");
                let response =
                    mcpr::schema::json_rpc::JSONRPCResponse::new(id, serde_json::json!(result));
                eprintln!("sending tool result response");
                transport.send(&mcpr::schema::json_rpc::JSONRPCMessage::Response(response))?;
            }
            Err(e) => {
                // Send error response
                eprintln!("tool execution failed: {}", e);
                self.send_error(id, -32000, format!("Tool execution failed: {}", e), None)?;
            }
        }

        Ok(())
    }

    /// Handle shutdown request
    fn handle_shutdown(&mut self, id: mcpr::schema::json_rpc::RequestId) -> Result<(), MCPError> {
        let transport = self
            .transport
            .as_mut()
            .ok_or_else(|| MCPError::Protocol("Transport not initialized".to_string()))?;

        // Create shutdown response
        let response = mcpr::schema::json_rpc::JSONRPCResponse::new(id, serde_json::json!({}));

        // Send the response
        eprintln!("Sending shutdown response");
        transport.send(&mcpr::schema::json_rpc::JSONRPCMessage::Response(response))?;

        // Close the transport
        eprintln!("Closing transport");
        transport.close()?;

        Ok(())
    }

    /// Send an error response
    fn send_error(
        &mut self,
        id: mcpr::schema::json_rpc::RequestId,
        code: i32,
        message: String,
        data: Option<Value>,
    ) -> Result<(), MCPError> {
        let transport = self
            .transport
            .as_mut()
            .ok_or_else(|| MCPError::Protocol("Transport not initialized".to_string()))?;

        // Create error response
        let error = mcpr::schema::json_rpc::JSONRPCMessage::Error(
            mcpr::schema::json_rpc::JSONRPCError::new(id, code, message.clone(), data),
        );

        // Send the error
        eprintln!("Sending error response: {}", message);
        transport.send(&error)?;

        Ok(())
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    // Initialize logging

    // Configure the server
    let server_config = ServerConfig::new()
        .with_name("mcp-v8-server")
        .with_version("1.0.0")
        .with_tool(Tool {
            name: "javascript".to_string(),
            description: Some("execute javascript".to_string()),
            input_schema: ToolInputSchema {
                r#type: "object".to_string(),
                properties: Some(
                    [
                        (
                            "code".to_string(),
                            serde_json::json!({
                                "type": "string",
                                "description": "code to executet"
                            }),
                        ),
                        (
                            "session_id".to_string(),
                            serde_json::json!({
                                "type": "string",
                                "description": "session id"
                            }),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                ),
                required: Some(vec!["code".to_string()]),
            },
        });

    // Create the server
    let mut server = Server::new(server_config);

    // Initialize V8 once at the start
    let platform = v8::new_default_platform(0, false).make_shared();
    v8::V8::initialize_platform(platform);
    v8::V8::initialize();

    // Register tool handlers
    server.register_tool_handler("javascript", |params: Value| {
        eprintln!(
            "calling tool javascript with params: {}",
            params.to_string()
        );
        let code = params.get("code").unwrap().as_str().unwrap();

        // Create a new isolate for each execution
        let mut string_result = String::new();

        let startup_data = {
            eprintln!("Creating isolate...");
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
                        return Err(MCPError::Protocol("error creating isolate".to_string()));
                    }
                }
            };

            eprintln!("Isolate created");

            {
                let scope = &mut v8::HandleScope::new(&mut snapshot_creator);
                let context = v8::Context::new(scope, Default::default());
                let scope = &mut v8::ContextScope::new(scope, context);
                let ouptut = eval(scope, code).unwrap();
                string_result = ouptut.to_string(scope).unwrap().to_rust_string_lossy(scope);
                eprintln!("code executed: {}", string_result);
                scope.set_default_context(context);
            }

            snapshot_creator
                .create_blob(v8::FunctionCodeHandling::Clear)
                .unwrap()
        };


        eprintln!("snapshot created");
        eprintln!("writing snapshot to file snapshot.bin in current directory");
        let mut file = std::fs::File::create("snapshot.bin").unwrap();
        file.write_all(&startup_data).unwrap();

        // save snapshot to file
        // We could create a snapshot here if needed, but it's not necessary for each execution
        // Let's just return the result
        Ok(json!({
            "output": string_result
        }))
    })?;

    // Create transport and start the server
    eprintln!("Starting stdio server");
    let transport = StdioTransport::new();

    eprintln!("Starting mcp-v8-server...");

    let result = server.start(transport);

    if let Err(e) = result {
        eprintln!("Server failed to start: {}", e);
    }

    Ok(())
}
