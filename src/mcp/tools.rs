use crate::error::{NexusError, Result};
use crate::mcp::{Tool, ToolResult, ToolContent};
use crate::mcp::command_validator::validate_command;
use serde_json::json;
use std::process::Command as StdCommand;
use tokio::fs;

/// Get the list of built-in Nexus tools exposed via MCP
pub fn get_nexus_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "file_read".to_string(),
            description: "Read the contents of a file. Use this when you need to see what's in a file.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The absolute or relative path to the file to read"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Line number to start reading from (0-based, optional)",
                        "default": 0
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of lines to read (optional)",
                        "default": 2000
                    }
                },
                "required": ["path"]
            }),
        },
        Tool {
            name: "file_write".to_string(),
            description: "Create a new file or overwrite an existing file with the given content. Use this when you need to write or modify files.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The absolute or relative path to the file to write"
                    },
                    "content": {
                        "type": "string",
                        "description": "The full content to write to the file"
                    },
                    "create_dirs": {
                        "type": "boolean",
                        "description": "Create parent directories if they don't exist",
                        "default": true
                    }
                },
                "required": ["path", "content"]
            }),
        },
        Tool {
            name: "shell_execute".to_string(),
            description: "Execute a shell command with safety checks. Use this when you need to run commands, install packages, or perform system operations.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to execute"
                    },
                    "working_dir": {
                        "type": "string",
                        "description": "Working directory for the command (optional)",
                        "default": "."
                    },
                    "timeout": {
                        "type": "integer",
                        "description": "Timeout in seconds (optional, max 300)",
                        "default": 60
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
            name: "git_status".to_string(),
            description: "Get the git status of a repository. Shows staged, unstaged, and untracked files.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the git repository (optional, defaults to current directory)",
                        "default": "."
                    }
                },
                "required": []
            }),
        },
        Tool {
            name: "search_code".to_string(),
            description: "Search for code patterns using regex or semantic similarity. Returns matching files and line numbers.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query (regex pattern or natural language for semantic search)"
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory to search in (optional, defaults to current directory)",
                        "default": "."
                    },
                    "semantic": {
                        "type": "boolean",
                        "description": "Use semantic similarity search instead of regex",
                        "default": false
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of results to return",
                        "default": 50
                    }
                },
                "required": ["query"]
            }),
        },
        Tool {
            name: "file_list".to_string(),
            description: "List files and directories at a given path.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The directory path to list"
                    },
                    "recursive": {
                        "type": "boolean",
                        "description": "List recursively",
                        "default": false
                    }
                },
                "required": ["path"]
            }),
        },
    ]
}

/// Execute a built-in Nexus tool
pub async fn execute_nexus_tool(name: &str, arguments: serde_json::Value) -> Result<ToolResult> {
    match name {
        "file_read" => file_read(arguments).await,
        "file_write" => file_write(arguments).await,
        "shell_execute" => shell_execute(arguments).await,
        "git_status" => git_status(arguments).await,
        "search_code" => search_code(arguments).await,
        "file_list" => file_list(arguments).await,
        _ => Err(NexusError::Configuration(format!("Unknown tool: {}", name))),
    }
}

/// Read file contents
async fn file_read(args: serde_json::Value) -> Result<ToolResult> {
    let path = args
        .get("path")
        .and_then(|p| p.as_str())
        .ok_or_else(|| NexusError::Configuration("Missing 'path' argument".to_string()))?;

    let offset = args
        .get("offset")
        .and_then(|o| o.as_u64())
        .unwrap_or(0) as usize;

    let limit = args
        .get("limit")
        .and_then(|l| l.as_u64())
        .unwrap_or(2000) as usize;

    let path_buf = std::path::PathBuf::from(path);
    let canonical_path = if path_buf.is_absolute() {
        path_buf.canonicalize()
    } else {
        std::env::current_dir()?.join(path_buf).canonicalize()
    }.map_err(|_| NexusError::Configuration(format!("Invalid or inaccessible path: {}", path)))?;

    let content = fs::read_to_string(&canonical_path).await
        .map_err(|e| NexusError::Io(e))?;

    let lines: Vec<&str> = content.lines().collect();
    let start = offset.min(lines.len());
    let end = (offset + limit).min(lines.len());
    let selected_lines = &lines[start..end];

    let result_text = if start > 0 || end < lines.len() {
        format!("Lines {}-{} of {}:\n{}", start + 1, end, lines.len(), selected_lines.join("\n"))
    } else {
        selected_lines.join("\n")
    };

    Ok(ToolResult {
        content: vec![ToolContent::Text { text: result_text }],
        is_error: None,
    })
}

/// Write file contents
async fn file_write(args: serde_json::Value) -> Result<ToolResult> {
    let path = args
        .get("path")
        .and_then(|p| p.as_str())
        .ok_or_else(|| NexusError::Configuration("Missing 'path' argument".to_string()))?;

    let content = args
        .get("content")
        .and_then(|c| c.as_str())
        .ok_or_else(|| NexusError::Configuration("Missing 'content' argument".to_string()))?;

    let create_dirs = args
        .get("create_dirs")
        .and_then(|c| c.as_bool())
        .unwrap_or(true);

    let path_buf = std::path::PathBuf::from(path);

    if create_dirs {
        if let Some(parent) = path_buf.parent() {
            fs::create_dir_all(parent).await
                .map_err(|e| NexusError::Io(e))?;
        }
    }

    fs::write(&path_buf, content).await
        .map_err(|e| NexusError::Io(e))?;

    Ok(ToolResult {
        content: vec![ToolContent::Text { 
            text: format!("Updated file: {}", path_buf.display())
        }],
        is_error: None,
    })
}

/// Execute shell commands with safety checks
async fn shell_execute(args: serde_json::Value) -> Result<ToolResult> {
    let command = args
        .get("command")
        .and_then(|c| c.as_str())
        .ok_or_else(|| NexusError::Configuration("Missing 'command' argument".to_string()))?;

    let _reason = args
        .get("reason")
        .and_then(|r| r.as_str())
        .ok_or_else(|| NexusError::Configuration("Missing 'reason' argument".to_string()))?;

    let working_dir = args
        .get("working_dir")
        .and_then(|w| w.as_str())
        .unwrap_or(".");

    let timeout_secs = args
        .get("timeout")
        .and_then(|t| t.as_u64())
        .unwrap_or(60)
        .min(300) as u64;

    // Validate command against allowlist and blocklist
    if let Err(e) = validate_command(command) {
        return Ok(ToolResult {
            content: vec![ToolContent::Text {
                text: format!("Command blocked: {}", e)
            }],
            is_error: Some(true),
        });
    }

    let output = tokio::time::timeout(
        tokio::time::Duration::from_secs(timeout_secs),
        tokio::task::spawn_blocking({
            let cmd = command.to_string();
            let dir = working_dir.to_string();
            move || {
                StdCommand::new("sh")
                    .arg("-c")
                    .arg(cmd)
                    .current_dir(dir)
                    .output()
            }
        })
    ).await;

    let result = match output {
        Ok(Ok(Ok(output))) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let mut text = String::new();
            
            if !stdout.is_empty() {
                text.push_str(&format!("STDOUT:\n{}", stdout));
            }
            if !stderr.is_empty() {
                if !text.is_empty() { text.push_str("\n\n"); }
                text.push_str(&format!("STDERR:\n{}", stderr));
            }
            if text.is_empty() {
                text = "Command executed successfully (no output)".to_string();
            }

            ToolResult {
                content: vec![ToolContent::Text { text }],
                is_error: if output.status.success() { None } else { Some(true) },
            }
        }
        Ok(Ok(Err(e))) => {
            ToolResult {
                content: vec![ToolContent::Text { 
                    text: format!("Failed to execute: {}", e)
                }],
                is_error: Some(true),
            }
        }
        Ok(Err(_)) | Err(_) => {
            ToolResult {
                content: vec![ToolContent::Text { 
                    text: format!("Command timed out after {}s", timeout_secs)
                }],
                is_error: Some(true),
            }
        }
    };

    Ok(result)
}

/// Get git repository status
async fn git_status(args: serde_json::Value) -> Result<ToolResult> {
    let path = args
        .get("path")
        .and_then(|p| p.as_str())
        .unwrap_or(".");

    let output = tokio::process::Command::new("git")
        .args(["-C", path, "status", "--porcelain", "-b"])
        .output()
        .await
        .map_err(|e| NexusError::Io(e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Ok(ToolResult {
            content: vec![ToolContent::Text { text: format!("Git error: {}", stderr) }],
            is_error: Some(true),
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    
    if stdout.trim().is_empty() {
        return Ok(ToolResult {
            content: vec![ToolContent::Text { text: "Working tree clean".to_string() }],
            is_error: None,
        });
    }

    let mut staged = Vec::new();
    let mut unstaged = Vec::new();
    let mut untracked = Vec::new();
    let mut branch_info = String::new();

    for line in stdout.lines() {
        if line.starts_with("##") {
            branch_info = line.trim_start_matches("## ").to_string();
        } else if line.starts_with("??") {
            untracked.push(line[3..].to_string());
        } else {
            let status = &line[..2];
            let file = &line[3..];
            
            if status.chars().next() != Some(' ') {
                staged.push(format!("{} ({})", file, status.chars().next().unwrap()));
            }
            if status.chars().nth(1) != Some(' ') {
                unstaged.push(format!("{} ({})", file, status.chars().nth(1).unwrap()));
            }
        }
    }

    let mut text = format!("Branch: {}\n", branch_info);
    if !staged.is_empty() {
        text.push_str(&format!("\nStaged:\n{}\n", staged.join("\n")));
    }
    if !unstaged.is_empty() {
        text.push_str(&format!("\nUnstaged:\n{}\n", unstaged.join("\n")));
    }
    if !untracked.is_empty() {
        text.push_str(&format!("\nUntracked:\n{}\n", untracked.join("\n")));
    }

    Ok(ToolResult {
        content: vec![ToolContent::Text { text }],
        is_error: None,
    })
}

/// Search code with regex
async fn search_code(args: serde_json::Value) -> Result<ToolResult> {
    let query = args
        .get("query")
        .and_then(|q| q.as_str())
        .ok_or_else(|| NexusError::Configuration("Missing 'query' argument".to_string()))?;

    let path = args
        .get("path")
        .and_then(|p| p.as_str())
        .unwrap_or(".");

    let _semantic = args
        .get("semantic")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);

    let max_results = args
        .get("max_results")
        .and_then(|m| m.as_u64())
        .unwrap_or(50) as usize;

    let pattern = regex::Regex::new(query)
        .map_err(|e| NexusError::Regex(e))?;

    let mut results = Vec::new();
    let mut count = 0;

    for entry in walkdir::WalkDir::new(path).max_depth(10) {
        if count >= max_results {
            break;
        }

        let entry = entry.map_err(|e| NexusError::Io(std::io::Error::new(
            std::io::ErrorKind::Other, e
        )))?;

        if !entry.file_type().is_file() {
            continue;
        }

        let file_path = entry.path();
        let content = match fs::read_to_string(file_path).await {
            Ok(c) => c,
            Err(_) => continue, // Skip binary files
        };

        for (line_num, line) in content.lines().enumerate() {
            if pattern.is_match(line) {
                results.push(format!(
                    "{}:{} - {}",
                    file_path.display(),
                    line_num + 1,
                    line.trim().chars().take(100).collect::<String>()
                ));
                count += 1;
                if count >= max_results {
                    break;
                }
            }
        }
    }

    let text = if results.is_empty() {
        format!("No results found for pattern: {}", query)
    } else {
        format!("Found {} results:\n{}", results.len(), results.join("\n"))
    };

    Ok(ToolResult {
        content: vec![ToolContent::Text { text }],
        is_error: None,
    })
}

/// List files in directory
async fn file_list(args: serde_json::Value) -> Result<ToolResult> {
    let path = args
        .get("path")
        .and_then(|p| p.as_str())
        .ok_or_else(|| NexusError::Configuration("Missing 'path' argument".to_string()))?;

    let recursive = args
        .get("recursive")
        .and_then(|r| r.as_bool())
        .unwrap_or(false);

    let path_buf = std::path::PathBuf::from(path);
    
    if !path_buf.exists() {
        return Err(NexusError::Configuration(format!("Path not found: {}", path)));
    }

    let mut entries = Vec::new();

    let walker = if recursive {
        walkdir::WalkDir::new(&path_buf)
    } else {
        walkdir::WalkDir::new(&path_buf).max_depth(1)
    };

    for entry in walker {
        let entry = entry.map_err(|e| NexusError::Io(std::io::Error::new(
            std::io::ErrorKind::Other, e
        )))?;

        let path_display = entry.path().strip_prefix(&path_buf)
            .unwrap_or(entry.path())
            .display()
            .to_string();

        let prefix = if entry.file_type().is_dir() { "[DIR]  " } else { "[FILE] " };
        entries.push(format!("{}{}", prefix, path_display));
    }

    let text = format!("Contents of {}:\n{}", path, entries.join("\n"));

    Ok(ToolResult {
        content: vec![ToolContent::Text { text }],
        is_error: None,
    })
}
