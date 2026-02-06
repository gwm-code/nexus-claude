use crate::error::{NexusError, Result};
use crate::mcp::{McpServerConfig, Tool, ToolResult, MCP_VERSION};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

/// JSON-RPC request for MCP protocol
#[derive(Debug, Clone, Serialize, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
}

/// JSON-RPC response for MCP protocol
#[derive(Debug, Clone, Serialize, Deserialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

/// MCP Client for connecting to external MCP servers
pub struct McpClient {
    next_request_id: Mutex<u64>,
}

/// Connection to an external MCP server via stdio
pub struct ServerConnection {
    child: Arc<std::sync::Mutex<Child>>,
    pending_requests: Arc<std::sync::Mutex<std::collections::HashMap<u64, tokio::sync::oneshot::Sender<JsonRpcResponse>>>>,
    server_info: ServerInfo,
}

#[derive(Debug, Clone, Deserialize)]
struct ServerInfo {
    protocol_version: String,
    server_info: ServerMetadata,
    capabilities: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
struct ServerMetadata {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
}

impl McpClient {
    /// Create a new MCP client
    pub fn new() -> Self {
        Self {
            next_request_id: Mutex::new(1),
        }
    }

    /// Connect to an MCP server via stdio transport
    pub async fn connect(&self, config: &McpServerConfig) -> Result<ServerConnection> {
        // Spawn the server process
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args)
            .envs(&config.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn()
            .map_err(|e| NexusError::Configuration(
                format!("Failed to spawn MCP server {}: {}", config.name, e)
            ))?;

        let mut stdin = child.stdin.take()
            .ok_or_else(|| NexusError::Configuration("Failed to get stdin".to_string()))?;
        let stdout = child.stdout.take()
            .ok_or_else(|| NexusError::Configuration("Failed to get stdout".to_string()))?;

        // Initialize the connection
        let server_info = Self::perform_initialize(&mut stdin).await?;

        // Verify protocol version compatibility
        if server_info.protocol_version != MCP_VERSION {
            eprintln!(
                "[MCP] Warning: Protocol version mismatch. Expected {}, got {}",
                MCP_VERSION, server_info.protocol_version
            );
        }

        let pending_requests: Arc<std::sync::Mutex<std::collections::HashMap<u64, tokio::sync::oneshot::Sender<JsonRpcResponse>>>> = 
            Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));

        let connection = ServerConnection {
            child: Arc::new(std::sync::Mutex::new(child)),
            pending_requests: pending_requests.clone(),
            server_info,
        };

        // Start the response reader task
        tokio::spawn(Self::response_reader(stdout, pending_requests));

        Ok(connection)
    }

    /// Connect to an MCP server via HTTP transport
    pub async fn connect_http(&self, url: &str) -> Result<HttpServerConnection> {
        // Send initialize request via HTTP
        let client = reqwest::Client::new();
        let init_request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": MCP_VERSION,
                "capabilities": {},
                "clientInfo": {
                    "name": "nexus",
                    "version": "0.1.0"
                }
            }
        });

        let response = client
            .post(format!("{}/mcp", url))
            .json(&init_request)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(NexusError::Configuration(
                format!("HTTP error: {}", response.status())
            ));
        }

        let rpc_response: JsonRpcResponse = response.json().await?;
        let server_info: ServerInfo = serde_json::from_value(
            rpc_response.result.ok_or_else(|| {
                NexusError::Configuration("No result in initialize response".to_string())
            })?
        )?;

        // Send initialized notification
        let _ = client
            .post(format!("{}/mcp", url))
            .json(&json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            }))
            .send()
            .await;

        Ok(HttpServerConnection {
            url: url.to_string(),
            client,
            next_request_id: Mutex::new(2),
            server_info,
        })
    }

    /// Perform the MCP initialize handshake
    async fn perform_initialize(
        stdin: &mut ChildStdin,
    ) -> Result<ServerInfo> {
        let init_request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: "initialize".to_string(),
            params: Some(json!({
                "protocolVersion": MCP_VERSION,
                "capabilities": {},
                "clientInfo": {
                    "name": "nexus",
                    "version": "0.1.0"
                }
            })),
        };

        let request_json = serde_json::to_string(&init_request)?;
        stdin.write_all(format!("{}\n", request_json).as_bytes()).await?;
        stdin.flush().await?;

        // For now, we'll return a placeholder - in a real implementation,
        // we'd need to read from stdout and parse the response
        // This is simplified for the initial implementation
        Ok(ServerInfo {
            protocol_version: MCP_VERSION.to_string(),
            server_info: ServerMetadata {
                name: "external-server".to_string(),
                version: Some("1.0.0".to_string()),
            },
            capabilities: json!({}),
        })
    }

    /// Read responses from the server stdout
    async fn response_reader(
        stdout: ChildStdout,
        pending: Arc<std::sync::Mutex<std::collections::HashMap<u64, tokio::sync::oneshot::Sender<JsonRpcResponse>>>>,
    ) {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            if let Ok(response) = serde_json::from_str::<JsonRpcResponse>(&line) {
                if let Some(id) = response.id {
                    let mut pending = pending.lock().unwrap();
                    if let Some(sender) = pending.remove(&id) {
                        let _ = sender.send(response);
                    }
                }
            }
        }
    }
}

impl ServerConnection {
    /// List available tools from the connected server
    pub async fn list_tools(&self) -> Result<Vec<Tool>> {
        let response = self.send_request("tools/list", None).await?;
        
        let tools: Vec<Tool> = serde_json::from_value(
            response.result
                .ok_or_else(|| NexusError::Configuration("No result in list_tools response".to_string()))?
                .get("tools")
                .cloned()
                .unwrap_or(serde_json::Value::Array(vec![]))
        )?;
        
        Ok(tools)
    }

    /// Call a tool on the connected server
    pub async fn call_tool(&self, tool_name: &str, arguments: serde_json::Value) -> Result<ToolResult> {
        let response = self.send_request(
            "tools/call",
            Some(json!({
                "name": tool_name,
                "arguments": arguments
            }))
        ).await?;

        if let Some(error) = response.error {
            return Ok(ToolResult {
                content: vec![crate::mcp::ToolContent::Text { 
                    text: format!("Error: {}", error.message) 
                }],
                is_error: Some(true),
            });
        }

        let result: ToolResult = serde_json::from_value(
            response.result.ok_or_else(|| {
                NexusError::Configuration("No result in call_tool response".to_string())
            })?
        )?;

        Ok(result)
    }

    /// Send a JSON-RPC request and wait for response
    async fn send_request(
        &self,
        _method: &str,
        _params: Option<serde_json::Value>,
    ) -> Result<JsonRpcResponse> {
        // This is a simplified implementation
        // In a real implementation, we'd track request IDs and match responses

        // For this simplified implementation, we return a mock response
        // Real implementation would wait for the response_reader to deliver the response
        Ok(JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: Some(1),
            result: Some(json!({"tools": []})),
            error: None,
        })
    }

    /// Disconnect from the server
    pub async fn disconnect(&self) -> Result<()> {
        let child_arc = self.child.clone();
        tokio::task::spawn_blocking(move || {
            if let Ok(mut child) = child_arc.lock() {
                let _ = child.start_kill();
            }
        }).await.ok();
        Ok(())
    }
}

/// HTTP-based server connection
pub struct HttpServerConnection {
    url: String,
    client: reqwest::Client,
    next_request_id: Mutex<u64>,
    server_info: ServerInfo,
}

impl HttpServerConnection {
    /// List available tools via HTTP
    pub async fn list_tools(&self) -> Result<Vec<Tool>> {
        let id = {
            let mut next_id = self.next_request_id.lock().await;
            let id = *next_id;
            *next_id += 1;
            id
        };

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id,
            method: "tools/list".to_string(),
            params: None,
        };

        let response = self.client
            .post(format!("{}/mcp", self.url))
            .json(&request)
            .send()
            .await?;

        let rpc_response: JsonRpcResponse = response.json().await?;
        
        let tools: Vec<Tool> = serde_json::from_value(
            rpc_response.result
                .ok_or_else(|| NexusError::Configuration("No result in list_tools response".to_string()))?
                .get("tools")
                .cloned()
                .unwrap_or(serde_json::Value::Array(vec![]))
        )?;
        
        Ok(tools)
    }

    /// Call a tool via HTTP
    pub async fn call_tool(&self, tool_name: &str, arguments: serde_json::Value) -> Result<ToolResult> {
        let id = {
            let mut next_id = self.next_request_id.lock().await;
            let id = *next_id;
            *next_id += 1;
            id
        };

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id,
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": tool_name,
                "arguments": arguments
            })),
        };

        let response = self.client
            .post(format!("{}/mcp", self.url))
            .json(&request)
            .send()
            .await?;

        let rpc_response: JsonRpcResponse = response.json().await?;

        if let Some(error) = rpc_response.error {
            return Ok(ToolResult {
                content: vec![crate::mcp::ToolContent::Text { 
                    text: format!("Error: {}", error.message) 
                }],
                is_error: Some(true),
            });
        }

        let result: ToolResult = serde_json::from_value(
            rpc_response.result.ok_or_else(|| {
                NexusError::Configuration("No result in call_tool response".to_string())
            })?
        )?;

        Ok(result)
    }

    /// Get server information
    pub fn get_server_info(&self) -> &ServerInfo {
        &self.server_info
    }
}
