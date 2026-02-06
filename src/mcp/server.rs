use crate::error::{NexusError, Result};
use crate::mcp::{ServerCapabilities, Tool, ToolResult, MCP_VERSION};
use crate::mcp::tools::{execute_nexus_tool, get_nexus_tools};
use crate::mcp::resources::ResourceHandler;
use axum::{
    routing::{get, post},
    Router,
    Json,
    extract::State,
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

/// MCP Server for exposing Nexus capabilities to other clients
pub struct McpServer {
    running: Arc<RwLock<bool>>,
    tools: Arc<RwLock<Vec<Tool>>>,
    resource_handler: Arc<ResourceHandler>,
    shutdown_tx: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
}

/// Server state shared across handlers
#[derive(Clone)]
struct ServerState {
    tools: Arc<RwLock<Vec<Tool>>>,
    resource_handler: Arc<ResourceHandler>,
}

/// JSON-RPC request structure
#[derive(Debug, Clone, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: Option<u64>,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
}

/// JSON-RPC response structure
#[derive(Debug, Clone, Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

/// Initialize request parameters
#[derive(Debug, Clone, Deserialize)]
struct InitializeParams {
    protocol_version: String,
    capabilities: serde_json::Value,
    client_info: ClientInfo,
}

#[derive(Debug, Clone, Deserialize)]
struct ClientInfo {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
}

/// Call tool request parameters
#[derive(Debug, Clone, Deserialize)]
struct CallToolParams {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    arguments: Option<serde_json::Value>,
}

impl McpServer {
    /// Create a new MCP server
    pub fn new() -> Result<Self> {
        let tools = get_nexus_tools();
        
        Ok(Self {
            running: Arc::new(RwLock::new(false)),
            tools: Arc::new(RwLock::new(tools)),
            resource_handler: Arc::new(ResourceHandler::new()),
            shutdown_tx: Arc::new(Mutex::new(None)),
        })
    }

    /// Start the MCP server on the given port
    pub async fn start(&self, port: u16) -> Result<()> {
        let addr: SocketAddr = format!("0.0.0.0:{}", port).parse()
            .map_err(|e| NexusError::Configuration(format!("Invalid address: {}", e)))?;

        // Create server state
        let state = ServerState {
            tools: self.tools.clone(),
            resource_handler: self.resource_handler.clone(),
        };

        // Build router
        let app = Router::new()
            .route("/mcp", post(handle_mcp_request))
            .route("/health", get(health_check))
            .route("/", get(server_info))
            .with_state(state);

        // Create shutdown channel
        let (tx, rx) = tokio::sync::oneshot::channel();
        *self.shutdown_tx.lock().await = Some(tx);

        // Mark as running
        *self.running.write().await = true;

        println!("[MCP Server] Starting on port {}", port);

        // Start server with graceful shutdown
        let listener = tokio::net::TcpListener::bind(addr).await
            .map_err(|e| NexusError::Io(e))?;

        let server = axum::serve(listener, app);

        // Handle graceful shutdown
        let running = self.running.clone();
        tokio::spawn(async move {
            let _ = rx.await;
            println!("[MCP Server] Received shutdown signal");
            *running.write().await = false;
        });

        println!("[MCP Server] Listening on http://{}", addr);

        // Run server
        if let Err(e) = server.await {
            eprintln!("[MCP Server] Error: {}", e);
            *self.running.write().await = false;
            return Err(NexusError::Configuration(format!("Server error: {}", e)));
        }

        Ok(())
    }

    /// Stop the MCP server
    pub async fn stop(&self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.lock().await.take() {
            let _ = tx.send(());
        }
        
        *self.running.write().await = false;
        println!("[MCP Server] Stopped");
        Ok(())
    }

    /// Check if server is running
    pub fn is_running(&self) -> bool {
        // Use try_read to avoid blocking
        match self.running.try_read() {
            Ok(running) => *running,
            Err(_) => false, // If lock is held, assume running
        }
    }

    /// Register a new tool
    pub async fn register_tool(&self, tool: Tool) -> Result<()> {
        let mut tools = self.tools.write().await;
        
        // Check if tool already exists
        if tools.iter().any(|t| t.name == tool.name) {
            return Err(NexusError::Configuration(
                format!("Tool '{}' already registered", tool.name)
            ));
        }
        
        let tool_name = tool.name.clone();
        tools.push(tool);
        println!("[MCP Server] Registered tool: {}", tool_name);
        Ok(())
    }

    /// Unregister a tool
    pub async fn unregister_tool(&self, tool_name: &str) -> Result<()> {
        let mut tools = self.tools.write().await;
        let initial_len = tools.len();
        tools.retain(|t| t.name != tool_name);
        
        if tools.len() == initial_len {
            return Err(NexusError::Configuration(
                format!("Tool '{}' not found", tool_name)
            ));
        }
        
        println!("[MCP Server] Unregistered tool: {}", tool_name);
        Ok(())
    }

    /// Get list of registered tools
    pub async fn list_tools(&self) -> Vec<Tool> {
        self.tools.read().await.clone()
    }
}

/// Health check endpoint
async fn health_check() -> StatusCode {
    StatusCode::OK
}

/// Server info endpoint
async fn server_info() -> Json<serde_json::Value> {
    Json(json!({
        "name": "nexus-mcp-server",
        "version": "0.1.0",
        "protocol_version": MCP_VERSION,
        "capabilities": {
            "tools": { "list_changed": true },
            "resources": { "subscribe": false, "list_changed": true },
        }
    }))
}

/// Handle MCP JSON-RPC requests
async fn handle_mcp_request(
    State(state): State<ServerState>,
    Json(request): Json<JsonRpcRequest>,
) -> Json<JsonRpcResponse> {
    let response = match request.method.as_str() {
        "initialize" => handle_initialize(request.params, request.id).await,
        "notifications/initialized" => {
            // Notification, no response needed
            return Json(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: None,
                result: None,
                error: None,
            });
        }
        "tools/list" => handle_list_tools(&state, request.id).await,
        "tools/call" => handle_call_tool(&state, request.params, request.id).await,
        "resources/list" => handle_list_resources(&state, request.id).await,
        "resources/read" => handle_read_resource(&state, request.params, request.id).await,
        _ => JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: request.id,
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: format!("Method not found: {}", request.method),
                data: None,
            }),
        },
    };

    Json(response)
}

/// Handle initialize request
async fn handle_initialize(
    params: Option<serde_json::Value>,
    id: Option<u64>,
) -> JsonRpcResponse {
    let client_info = params.as_ref()
        .and_then(|p| p.get("clientInfo"))
        .and_then(|c| c.get("name"))
        .and_then(|n| n.as_str())
        .unwrap_or("unknown");

    println!("[MCP Server] Client connected: {}", client_info);

    JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id,
        result: Some(json!({
            "protocolVersion": MCP_VERSION,
            "serverInfo": {
                "name": "nexus",
                "version": "0.1.0"
            },
            "capabilities": ServerCapabilities {
                tools: Some(crate::mcp::ToolsCapability { list_changed: true }),
                resources: Some(crate::mcp::ResourcesCapability { 
                    subscribe: false, 
                    list_changed: true 
                }),
                prompts: None,
                logging: None,
            }
        })),
        error: None,
    }
}

/// Handle tools/list request
async fn handle_list_tools(
    state: &ServerState,
    id: Option<u64>,
) -> JsonRpcResponse {
    let tools = state.tools.read().await.clone();

    JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id,
        result: Some(json!({
            "tools": tools
        })),
        error: None,
    }
}

/// Handle tools/call request
async fn handle_call_tool(
    state: &ServerState,
    params: Option<serde_json::Value>,
    id: Option<u64>,
) -> JsonRpcResponse {
    let params: CallToolParams = match params {
        Some(p) => match serde_json::from_value(p) {
            Ok(params) => params,
            Err(e) => {
                return JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: format!("Invalid params: {}", e),
                        data: None,
                    }),
                };
            }
        },
        None => {
            return JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: "Missing params".to_string(),
                    data: None,
                }),
            };
        }
    };

    let arguments = params.arguments.unwrap_or(json!({}));

    println!("[MCP Server] Tool call: {} with args: {:?}", params.name, arguments);

    // Execute the tool
    match execute_nexus_tool(&params.name, arguments).await {
        Ok(result) => JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(json!(result)),
            error: None,
        },
        Err(e) => JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(json!(ToolResult {
                content: vec![crate::mcp::ToolContent::Text { 
                    text: format!("Error: {}", e) 
                }],
                is_error: Some(true),
            })),
            error: None,
        },
    }
}

/// Handle resources/list request
async fn handle_list_resources(
    state: &ServerState,
    id: Option<u64>,
) -> JsonRpcResponse {
    let resources = state.resource_handler.list_resources().await;

    JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id,
        result: Some(json!({
            "resources": resources
        })),
        error: None,
    }
}

/// Handle resources/read request
async fn handle_read_resource(
    state: &ServerState,
    params: Option<serde_json::Value>,
    id: Option<u64>,
) -> JsonRpcResponse {
    let uri = params.as_ref()
        .and_then(|p| p.get("uri"))
        .and_then(|u| u.as_str());

    match uri {
        Some(uri) => {
            match state.resource_handler.read_resource(uri).await {
                Ok(content) => JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id,
                    result: Some(json!({
                        "contents": [content]
                    })),
                    error: None,
                },
                Err(e) => JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32002,
                        message: format!("Failed to read resource: {}", e),
                        data: None,
                    }),
                },
            }
        }
        None => JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message: "Missing uri parameter".to_string(),
                data: None,
            }),
        },
    }
}
