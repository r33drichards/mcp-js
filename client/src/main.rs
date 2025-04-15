//! MCP Client for mcp-v8 project with stdio transport
//!
//! This client demonstrates how to connect to an MCP server using stdio transport.
//! 
//! There are two ways to use this client:
//! 1. Connect to an already running server (recommended for production)
//! 2. Start a new server process and connect to it (convenient for development)
//!
//! The client supports both interactive and one-shot modes.

use clap::Parser;
use mcpr::{
    error::MCPError,
    schema::json_rpc::{JSONRPCMessage, JSONRPCRequest, RequestId},
    transport::{
        stdio::StdioTransport,
        Transport,
    },
};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use std::error::Error;
use std::io::{self, BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use log::{info, error, debug, warn};

/// CLI arguments
#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Enable debug output
    #[arg(short, long)]
    debug: bool,
    
    /// Server command to execute (if not connecting to an existing server)
    #[arg(short, long, default_value = "./server/target/debug/mcp-v8-server")]
    server_cmd: String,
    
    /// Connect to an already running server instead of starting a new one
    #[arg(short, long)]
    connect: bool,
    
    /// Run in interactive mode
    #[arg(short, long)]
    interactive: bool,
    
    /// Name to greet (for non-interactive mode)
    #[arg(short, long)]
    name: Option<String>,
    
    /// Timeout in seconds for operations
    #[arg(short, long, default_value = "30")]
    timeout: u64,
}

/// High-level MCP client
struct Client<T: Transport> {
    transport: T,
    next_request_id: i64,
}

impl<T: Transport> Client<T> {
    /// Create a new MCP client with the given transport
    fn new(transport: T) -> Self {
        Self {
            transport,
            next_request_id: 1,
        }
    }

    /// Initialize the client
    fn initialize(&mut self) -> Result<Value, MCPError> {
        // Start the transport
        debug!("Starting transport");
        self.transport.start()?;

        // Send initialization request
        let initialize_request = JSONRPCRequest::new(
            self.next_request_id(),
            "initialize".to_string(),
            Some(serde_json::json!({
                "protocol_version": mcpr::constants::LATEST_PROTOCOL_VERSION
            })),
        );

        let message = JSONRPCMessage::Request(initialize_request);
        debug!("Sending initialize request: {:?}", message);
        self.transport.send(&message)?;

        // Wait for response
        eprintln!("Waiting for initialization response");
        let response: JSONRPCMessage = self.transport.receive()?;
        debug!("Received response: {:?}", response);

        match response {
            JSONRPCMessage::Response(resp) => Ok(resp.result),
            JSONRPCMessage::Error(err) => {
                error!("Initialization failed: {:?}", err);
                Err(MCPError::Protocol(format!(
                    "Initialization failed: {:?}",
                    err
                )))
            }
            _ => {
                error!("Unexpected response type");
                Err(MCPError::Protocol("Unexpected response type".to_string()))
            }
        }
    }

    /// Call a tool on the server
    fn call_tool<P: Serialize + std::fmt::Debug, R: DeserializeOwned>(
        &mut self,
        tool_name: &str,
        params: &P,
    ) -> Result<R, MCPError> {
        // Create tool call request
        let tool_call_request = JSONRPCRequest::new(
            self.next_request_id(),
            "tool_call".to_string(),
            Some(serde_json::json!({
                "name": tool_name,
                "parameters": serde_json::to_value(params)?
            })),
        );

        let message = JSONRPCMessage::Request(tool_call_request);
        eprintln!("Calling tool '{}' with parameters: {:?}", tool_name, params);
        debug!("Sending tool call request: {:?}", message);
        self.transport.send(&message)?;

        // Wait for response
        eprintln!("Waiting for tool call response");
        let response: JSONRPCMessage = self.transport.receive()?;
        debug!("Received response: {:?}", response);

        match response {
            JSONRPCMessage::Response(resp) => {
                // Extract the tool result from the response
                let result_value = resp.result;
                let result = result_value.get("result").ok_or_else(|| {
                    error!("Missing 'result' field in response");
                    MCPError::Protocol("Missing 'result' field in response".to_string())
                })?;

                // Parse the result
                debug!("Parsing result: {:?}", result);
                serde_json::from_value(result.clone()).map_err(|e| {
                    error!("Failed to parse result: {}", e);
                    MCPError::Serialization(e)
                })
            }
            JSONRPCMessage::Error(err) => {
                error!("Tool call failed: {:?}", err);
                Err(MCPError::Protocol(format!("Tool call failed: {:?}", err)))
            }
            _ => {
                error!("Unexpected response type");
                Err(MCPError::Protocol("Unexpected response type".to_string()))
            }
        }
    }

    /// Shutdown the client
    fn shutdown(&mut self) -> Result<(), MCPError> {
        // Send shutdown request
        let shutdown_request =
            JSONRPCRequest::new(self.next_request_id(), "shutdown".to_string(), None);

        let message = JSONRPCMessage::Request(shutdown_request);
        eprintln!("Sending shutdown request");
        debug!("Shutdown request: {:?}", message);
        self.transport.send(&message)?;

        // Wait for response
        eprintln!("Waiting for shutdown response");
        let response: JSONRPCMessage = self.transport.receive()?;
        debug!("Received response: {:?}", response);

        match response {
            JSONRPCMessage::Response(_) => {
                // Close the transport
                eprintln!("Closing transport");
                self.transport.close()?;
                Ok(())
            }
            JSONRPCMessage::Error(err) => {
                error!("Shutdown failed: {:?}", err);
                Err(MCPError::Protocol(format!("Shutdown failed: {:?}", err)))
            }
            _ => {
                error!("Unexpected response type");
                Err(MCPError::Protocol("Unexpected response type".to_string()))
            }
        }
    }

    /// Generate the next request ID
    fn next_request_id(&mut self) -> RequestId {
        let id = self.next_request_id;
        self.next_request_id += 1;
        RequestId::Number(id)
    }
}

/// Connect to an already running server
fn connect_to_running_server(command: &str, args: &[&str]) -> Result<(StdioTransport, Option<Child>), Box<dyn Error>> {
    eprintln!("Connecting to running server with command: {} {}", command, args.join(" "));
    
    // Start a new process that will connect to the server
    let mut process = Command::new(command)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    
    // Create a stderr reader to monitor server output
    if let Some(stderr) = process.stderr.take() {
        let stderr_reader = BufReader::new(stderr);
        thread::spawn(move || {
            for line in stderr_reader.lines().map_while(Result::ok) {
                debug!("Server stderr: {}", line);
            }
        });
    }
    
    // Give the server a moment to start up
    thread::sleep(Duration::from_millis(500));
    
    // Create a transport that communicates with the server process
    let transport = StdioTransport::with_reader_writer(
        Box::new(process.stdout.take().ok_or("Failed to get stdout")?),
        Box::new(process.stdin.take().ok_or("Failed to get stdin")?),
    );
    
    Ok((transport, Some(process)))
}

/// Start a new server and connect to it
fn start_and_connect_to_server(server_cmd: &str) -> Result<(StdioTransport, Option<Child>), Box<dyn Error>> {
    eprintln!("Starting server process: {}", server_cmd);
    
    // Start the server process
    let mut server_process = Command::new(server_cmd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    
    // Create a stderr reader to monitor server output
    if let Some(stderr) = server_process.stderr.take() {
        let stderr_reader = BufReader::new(stderr);
        thread::spawn(move || {
            for line in stderr_reader.lines().map_while(Result::ok) {
                debug!("Server stderr: {}", line);
            }
        });
    }
    
    // Give the server a moment to start up
    thread::sleep(Duration::from_millis(500));
    
    let server_stdin = server_process.stdin.take().ok_or("Failed to get stdin")?;
    let server_stdout = server_process.stdout.take().ok_or("Failed to get stdout")?;

    eprintln!("Using stdio transport");
    let transport = StdioTransport::with_reader_writer(
        Box::new(server_stdout),
        Box::new(server_stdin),
    );
    
    Ok((transport, Some(server_process)))
}

fn prompt_input(prompt: &str) -> Result<String, io::Error> {
    print!("{}: ", prompt);
    io::stdout().flush()?;
    
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    
    Ok(input.trim().to_string())
}

fn main() -> Result<(), Box<dyn Error>> {
    // Initialize logging
    env_logger::init_from_env(env_logger::Env::default().default_filter_or("info"));
    
    // Parse command line arguments
    let args = Args::parse();
    
    // Set log level based on debug flag
    if args.debug {
        log::set_max_level(log::LevelFilter::Debug);
        debug!("Debug logging enabled");
    }
    
    // Set timeout
    let timeout = Duration::from_secs(args.timeout);
    eprintln!("Operation timeout set to {} seconds", args.timeout);
    
    // Create transport and server process based on connection mode
    let (transport, server_process) = if args.connect {
        eprintln!("Connecting to already running server");
        connect_to_running_server(&args.server_cmd, &[])?
    } else {
        eprintln!("Starting new server process");
        start_and_connect_to_server(&args.server_cmd)?
    };
    
    let mut client = Client::new(transport);
    
    // Initialize the client with timeout
    eprintln!("Initializing client...");
    let start_time = Instant::now();
    let _init_result = loop {
        if start_time.elapsed() >= timeout {
            error!("Initialization timed out after {:?}", timeout);
            return Err(Box::new(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("Initialization timed out after {:?}", timeout),
            )));
        }
        
        match client.initialize() {
            Ok(result) => {
                eprintln!("Server info: {:?}", result);
                break result;
            },
            Err(e) => {
                warn!("Initialization attempt failed: {}", e);
                thread::sleep(Duration::from_millis(500));
                continue;
            }
        }
    };
    
    if args.interactive {
        // Interactive mode
        eprintln!("=== mcp-v8-client Interactive Mode ===");
        println!("=== mcp-v8-client Interactive Mode ===");
        println!("Type 'exit' or 'quit' to exit");
        
        loop {
            let name = prompt_input("Enter your name (or 'exit' to quit)")?;
            if name.to_lowercase() == "exit" || name.to_lowercase() == "quit" {
                eprintln!("User requested exit");
                break;
            }
            
            // Call the hello tool
            let request = serde_json::json!({
                "name": name
            });
            
            match client.call_tool::<Value, Value>("hello", &request) {
                Ok(response) => {
                    if let Some(message) = response.get("message") {
                        let msg = message.as_str().unwrap_or("");
                        eprintln!("Received message: {}", msg);
                        println!("{}", msg);
                    } else {
                        eprintln!("Received response without message field: {:?}", response);
                        println!("Response: {:?}", response);
                    }
                },
                Err(e) => {
                    error!("Error calling tool: {}", e);
                    eprintln!("Error: {}", e);
                }
            }
            
            println!();
        }
        
        eprintln!("Exiting interactive mode");
        println!("Exiting interactive mode");
    } else {
        // One-shot mode
        let name = args.name.ok_or_else(|| {
            error!("Name is required in non-interactive mode");
            "Name is required in non-interactive mode"
        })?;
        
        eprintln!("Running in one-shot mode with name: {}", name);
        
        // Call the hello tool
        let request = serde_json::json!({
            "name": name
        });
        
        let response: Value = match client.call_tool("hello", &request) {
            Ok(response) => response,
            Err(e) => {
                error!("Error calling tool: {}", e);
                return Err(Box::new(e));
            }
        };
        
        if let Some(message) = response.get("message") {
            let msg = message.as_str().unwrap_or("");
            eprintln!("Received message: {}", msg);
            println!("{}", msg);
        } else {
            eprintln!("Received response without message field: {:?}", response);
            println!("Response: {:?}", response);
        }
    }
    
    // Shutdown the client
    eprintln!("Shutting down client");
    if let Err(e) = client.shutdown() {
        error!("Error during shutdown: {}", e);
    }
    eprintln!("Client shutdown complete");
    
    // If we started the server, terminate it gracefully
    if let Some(mut process) = server_process {
        eprintln!("Terminating server process...");
        let _ = process.kill();
    }
    
    Ok(())
}