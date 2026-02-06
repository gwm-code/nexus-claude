//! MCP (Model Context Protocol) Integration
//!
//! MCP is an open standard for connecting AI assistants to external data sources and tools.
//! This module implements both MCP client (to connect to external MCP servers) and
//! MCP server (to expose Nexus capabilities to other MCP clients).

pub mod client;
pub mod server;
pub mod tools;
pub mod resources;

use crate::error::{NexusError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// MCP Protocol Version
pub const MCP_VERSION: &str = "2024-11-05";

/// Capabilities that an MCP server provides
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServerCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolsCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourcesCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompts: Option<PromptsCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logging: Option<LoggingCapability>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsCapability {
    pub list_changed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourcesCapability {
    pub subscribe: bool,
    pub list_changed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptsCapability {
    pub list_changed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingCapability {}

/// An MCP Tool definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// An MCP Resource definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resource {
    pub uri: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// An MCP Prompt definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prompt {
    pub name: String,
    pub description: String,
    pub arguments: Option<Vec<PromptArgument>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptArgument {
    pub name: String,
    pub description: String,
    pub required: bool,
}

/// Tool call result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub content: Vec<ToolContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToolContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { data: String, mime_type: String },
    #[serde(rename = "resource")]
    Resource { resource: ResourceContent },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceContent {
    pub uri: String,
    pub mime_type: String,
    pub text: String,
}

/// External MCP server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

/// The unified MCP integration point
pub struct McpIntegration {
    client: client::McpClient,
    server: server::McpServer,
    connected_servers: HashMap<String, client::ServerConnection>,
}

impl McpIntegration {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: client::McpClient::new(),
            server: server::McpServer::new()?, 
            connected_servers: HashMap::new(),
        })
    }

    /// Connect to an external MCP server
    pub async fn connect_server(&mut self, config: &McpServerConfig) -> Result<()> {
        println!("[MCP] Connecting to server: {}", config.name);
        
        let connection = self.client.connect(config).await?;
        self.connected_servers.insert(config.name.clone(), connection);
        
        println!("[MCP] Connected to {}", config.name);
        Ok(())
    }

    /// List all available tools from connected servers
    pub async fn list_all_tools(&self) -> Result<Vec<(String, Tool)>> {
        let mut all_tools = Vec::new();
        
        // Add built-in Nexus tools
        all_tools.extend(tools::get_nexus_tools().into_iter()
            .map(|t| ("nexus".to_string(), t)));
        
        // Add tools from connected servers
        for (server_name, connection) in &self.connected_servers {
            let tools = connection.list_tools().await?;
            all_tools.extend(tools.into_iter()
                .map(|t| (server_name.clone(), t)));
        }
        
        Ok(all_tools)
    }

    /// Execute a tool from any connected server
    pub async fn execute_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<ToolResult> {
        if server_name == "nexus" {
            // Execute built-in Nexus tool
            tools::execute_nexus_tool(tool_name, arguments).await
        } else if let Some(connection) = self.connected_servers.get(server_name) {
            // Execute tool from external server
            connection.call_tool(tool_name, arguments).await
        } else {
            Err(NexusError::Configuration(
                format!("Server not found: {}", server_name)
            ))
        }
    }

    /// Start the MCP server to accept incoming connections
    pub async fn start_server(&self, port: u16) -> Result<()> {
        self.server.start(port).await
    }

    /// Get server status
    pub fn get_status(&self) -> McpStatus {
        McpStatus {
            connected_servers: self.connected_servers.keys().cloned().collect(),
            server_running: self.server.is_running(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct McpStatus {
    pub connected_servers: Vec<String>,
    pub server_running: bool,
}

/// Predefined MCP server configurations for common tools
pub fn get_builtin_server_configs() -> Vec<McpServerConfig> {
    vec![
        McpServerConfig {
            name: "sqlite".to_string(),
            command: "uvx".to_string(),
            args: vec!["mcp-server-sqlite".to_string(), "--db-path".to_string(), 
                      "${DB_PATH}".to_string()],
            env: HashMap::new(),
        },
        McpServerConfig {
            name: "postgres".to_string(),
            command: "uvx".to_string(),
            args: vec!["mcp-server-postgres".to_string(), 
                      "postgresql://localhost/db".to_string()],
            env: HashMap::new(),
        },
        McpServerConfig {
            name: "github".to_string(),
            command: "npx".to_string(),
            args: vec!["-y".to_string(), "@modelcontextprotocol/server-github".to_string()],
            env: [("GITHUB_PERSONAL_ACCESS_TOKEN".to_string(), 
                  "${GITHUB_TOKEN}".to_string())]
                .into_iter()
                .collect(),
        },
        McpServerConfig {
            name: "filesystem".to_string(),
            command: "npx".to_string(),
            args: vec!["-y".to_string(), "@modelcontextprotocol/server-filesystem".to_string(),
                      ".".to_string()],
            env: HashMap::new(),
        },
    ]
}
