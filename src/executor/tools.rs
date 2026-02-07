use crate::error::Result;
use regex::Regex;
use serde::{Deserialize, Serialize};

/// A tool that the AI can call to execute system commands or file operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// A tool call made by the AI
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Result of executing a tool call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
}

/// Available tools for the AI
pub fn get_available_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "execute_command".to_string(),
            description: "Execute a shell command in the sandbox. Use this when the user asks you to run a command, install packages, or perform system operations. The command will be run in an isolated Docker container first for safety.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to execute (e.g., 'npm install express', 'mkdir newdir')"
                    },
                    "reason": {
                        "type": "string",
                        "description": "Brief explanation of why this command needs to run"
                    }
                },
                "required": ["command", "reason"]
            }),
        },
        Tool {
            name: "create_file".to_string(),
            description: "Create a new file with the given content. Use this when the user asks you to create a file or write code.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The file path to create (e.g., 'src/main.js')"
                    },
                    "content": {
                        "type": "string",
                        "description": "The full content to write to the file"
                    },
                    "reason": {
                        "type": "string",
                        "description": "Brief explanation of what this file contains"
                    }
                },
                "required": ["path", "content", "reason"]
            }),
        },
        Tool {
            name: "edit_file".to_string(),
            description: "Edit an existing file. Use this when the user asks you to modify, update, or change a file.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The file path to edit"
                    },
                    "content": {
                        "type": "string",
                        "description": "The new full content of the file"
                    },
                    "reason": {
                        "type": "string",
                        "description": "Brief explanation of what changed and why"
                    }
                },
                "required": ["path", "content", "reason"]
            }),
        },
        Tool {
            name: "read_file".to_string(),
            description: "Read the contents of a file. Use this when you need to see what's in a file before editing it.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The file path to read"
                    }
                },
                "required": ["path"]
            }),
        },
        Tool {
            name: "run_tests".to_string(),
            description: "Run the test suite for the project. Use this after making changes to verify they work.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The test command to run (e.g., 'npm test', 'cargo test', 'pytest')"
                    }
                },
                "required": ["command"]
            }),
        },
    ]
}

/// Convert tool calls to a system prompt message for models that don't support native tool calling
pub fn create_tool_system_prompt() -> String {
    let tools = get_available_tools();
    let mut prompt = String::from(
        "You are Nexus, an AI CLI assistant. You help users with software engineering tasks.\n\n\
        You have access to the following tools to help the user. When you need to perform an action, \
        you MUST call the appropriate tool by responding with a JSON object in this exact format:\n\n"
    );

    for tool in &tools {
        prompt.push_str(&format!(
            "Tool: {}\nDescription: {}\nParameters: {}\n\n",
            tool.name,
            tool.description,
            serde_json::to_string_pretty(&tool.parameters).unwrap_or_default()
        ));
    }

    prompt.push_str(
        "IMPORTANT INSTRUCTIONS:\n\
        1. Complete the FULL task the user requests - do not stop after one step\n\
        2. If you need to check something first (like OS), do it, then CONTINUE to complete the rest\n\
        3. Chain multiple tool calls as needed to finish the job\n\
        4. After each tool executes, you will see the result - use it to continue\n\
        5. When using tools, respond with ONLY the JSON - no other text\n\
        6. After ALL tools complete, give the user a summary\n\n\
        Example:\n\
        User: \"check my distro and install xclip\"\n\
        You: {\"tool\": \"execute_command\", \"arguments\": {\"command\": \"cat /etc/os-release\"}}\n\
        [System: Result shows Alpine]\n\
        You: {\"tool\": \"execute_command\", \"arguments\": {\"command\": \"apk add xclip\"}}\n\
        [System: Result shows success]\n\
        You: \"Done! xclip is installed on your Alpine system.\"\n\n\
        Tool format:\n\
        {\n  \"tool\": \"tool_name\",\n  \"arguments\": { ... }\n}"
    );

    prompt
}

/// Parse a response to extract tool calls
pub fn parse_tool_calls(response: &str) -> Vec<ToolCall> {
    let mut tool_calls = Vec::new();

    // First, try to extract JSON from code blocks (AI often wraps in ```json)
    use std::sync::LazyLock;
    static CODE_BLOCK_RE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"```(?:json)?\n(.*?)```").expect("invalid regex"));
    let code_block_regex = &*CODE_BLOCK_RE;
    for cap in code_block_regex.captures_iter(response) {
        if let Some(json_str) = cap.get(1) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(json_str.as_str().trim()) {
                if let Some(tool_name) = json.get("tool").and_then(|t| t.as_str()) {
                    let arguments = json
                        .get("arguments")
                        .cloned()
                        .unwrap_or(serde_json::json!({}));
                    tool_calls.push(ToolCall {
                        id: format!("call_{}", uuid::Uuid::new_v4()),
                        name: tool_name.to_string(),
                        arguments,
                    });
                }
            }
        }
    }

    // Also try raw JSON if no code blocks found
    if tool_calls.is_empty() {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(response.trim()) {
            if let Some(tool_name) = json.get("tool").and_then(|t| t.as_str()) {
                let arguments = json
                    .get("arguments")
                    .cloned()
                    .unwrap_or(serde_json::json!({}));
                tool_calls.push(ToolCall {
                    id: format!("call_{}", uuid::Uuid::new_v4()),
                    name: tool_name.to_string(),
                    arguments,
                });
            }
        }
    }

    tool_calls
}

/// Check if a response contains a tool call
pub fn is_tool_call(response: &str) -> bool {
    parse_tool_calls(response).len() > 0
}
