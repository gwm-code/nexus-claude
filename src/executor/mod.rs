pub mod parser;
pub mod tools;

use crate::context::FileAccessTracker;
use crate::error::{NexusError, Result};
use crate::sandbox::{SandboxManager, ShadowRunResult};
use crate::sandbox::hydration::{Hydrator, HydrationPlan, FileChange};
use dialoguer::Confirm;
use std::path::PathBuf;
use tools::{ToolCall, ToolResult, parse_tool_calls};

/// The AgentExecutor handles AI tool calls and executes them safely
pub struct AgentExecutor {
    sandbox: SandboxManager,
    hydrator: Hydrator,
    working_dir: PathBuf,
    auto_mode: bool,
    file_tracker: FileAccessTracker,
}

impl AgentExecutor {
    pub fn new(working_dir: PathBuf, auto_mode: bool) -> Result<Self> {
        Ok(Self {
            sandbox: SandboxManager::new(),
            hydrator: Hydrator::new()?,
            working_dir,
            auto_mode,
            file_tracker: FileAccessTracker::new(),
        })
    }

    /// Get a reference to the file access tracker
    pub fn file_tracker(&self) -> &FileAccessTracker {
        &self.file_tracker
    }

    pub fn set_auto_mode(&mut self, auto_mode: bool) {
        self.auto_mode = auto_mode;
    }

    /// Process an AI response - display text and/or execute tool calls
    pub async fn process_response(&self, response: &str) -> Result<ExecutionOutcome> {
        // Check if response contains tool calls
        let tool_calls = parse_tool_calls(response);
        
        // Remove JSON/code blocks from response to get clean text
        let text_content = remove_tool_calls_from_text(response);
        
        // Print any text content first (if not empty)
        if !text_content.trim().is_empty() {
            println!("\n{}", text_content.trim());
        }
        
        if tool_calls.is_empty() {
            // No tool calls - just text response
            return Ok(ExecutionOutcome::Text {
                content: text_content,
            });
        }

        // Execute tool calls
        let mut results = Vec::new();
        let mut all_succeeded = true;

        println!("\n[SHADOW RUN] AI requested {} tool execution(s):", tool_calls.len());

        for (i, tool_call) in tool_calls.iter().enumerate() {
            println!("\n  [{}] Tool: {}", i + 1, tool_call.name);
            println!("      Args: {}", serde_json::to_string(&tool_call.arguments).unwrap_or_default());

            // Get user confirmation if not in auto mode
            if !self.auto_mode {
                let should_run = Confirm::new()
                    .with_prompt("Execute this tool in sandbox?")
                    .default(true)
                    .interact()?;

                if !should_run {
                    results.push(ToolResult {
                        tool_call_id: tool_call.id.clone(),
                        success: false,
                        output: "Skipped by user".to_string(),
                        error: Some("User declined".to_string()),
                    });
                    continue;
                }
            }

            // Execute the tool
            let result = self.execute_tool(&tool_call).await?;
            
            if result.success {
                println!("      ✓ Success");
            } else {
                println!("      ✗ Failed: {}", result.error.as_ref().unwrap_or(&"Unknown error".to_string()));
                all_succeeded = false;
            }
            
            results.push(result);
        }

        Ok(ExecutionOutcome::ToolExecution {
            success: all_succeeded,
            results,
        })
    }

    /// Execute a single tool call in the sandbox
    async fn execute_tool(&self, tool_call: &ToolCall) -> Result<ToolResult> {
        let result = match tool_call.name.as_str() {
            "execute_command" => {
                let command = tool_call.arguments.get("command")
                    .and_then(|c| c.as_str())
                    .unwrap_or_default();
                
                let shadow_result = self.sandbox.shadow_run(command, &self.working_dir).await?;
                
                ToolResult {
                    tool_call_id: tool_call.id.clone(),
                    success: shadow_result.success,
                    output: format!("stdout: {}\nstderr: {}", shadow_result.stdout, shadow_result.stderr),
                    error: if shadow_result.success { None } else { 
                        Some(format!("Exit code: {}", shadow_result.exit_code)) 
                    },
                }
            }
            "create_file" => {
                let path = tool_call.arguments.get("path")
                    .and_then(|p| p.as_str())
                    .unwrap_or_default();
                let content = tool_call.arguments.get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or_default();
                
                // Create file change
                let change = FileChange {
                    path: PathBuf::from(path),
                    content: content.to_string(),
                    backup_path: None,
                };
                
                let plan = HydrationPlan {
                    files_to_create: vec![change],
                    files_to_update: Vec::new(),
                    files_to_delete: Vec::new(),
                    directories_to_create: Vec::new(),
                };
                
                match self.hydrator.execute_plan(&plan) {
                    Ok(_) => ToolResult {
                        tool_call_id: tool_call.id.clone(),
                        success: true,
                        output: format!("Created file: {}", path),
                        error: None,
                    },
                    Err(e) => ToolResult {
                        tool_call_id: tool_call.id.clone(),
                        success: false,
                        output: String::new(),
                        error: Some(e.to_string()),
                    }
                }
            }
            "edit_file" => {
                let path = tool_call.arguments.get("path")
                    .and_then(|p| p.as_str())
                    .unwrap_or_default();
                let content = tool_call.arguments.get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or_default();
                
                let full_path = self.working_dir.join(path);
                
                // Check for staleness before editing
                if let Err(e) = self.file_tracker.check_staleness(&full_path) {
                    return Ok(ToolResult {
                        tool_call_id: tool_call.id.clone(),
                        success: false,
                        output: String::new(),
                        error: Some(e.to_string()),
                    });
                }
                
                let change = FileChange {
                    path: PathBuf::from(path),
                    content: content.to_string(),
                    backup_path: None,
                };
                
                let plan = HydrationPlan {
                    files_to_create: Vec::new(),
                    files_to_update: vec![change],
                    files_to_delete: Vec::new(),
                    directories_to_create: Vec::new(),
                };
                
                match self.hydrator.execute_plan(&plan) {
                    Ok(_) => ToolResult {
                        tool_call_id: tool_call.id.clone(),
                        success: true,
                        output: format!("Updated file: {}", path),
                        error: None,
                    },
                    Err(e) => ToolResult {
                        tool_call_id: tool_call.id.clone(),
                        success: false,
                        output: String::new(),
                        error: Some(e.to_string()),
                    }
                }
            }
            "read_file" => {
                let path = tool_call.arguments.get("path")
                    .and_then(|p| p.as_str())
                    .unwrap_or_default();
                
                let full_path = self.working_dir.join(path);
                match std::fs::read_to_string(&full_path) {
                    Ok(content) => {
                        // Record that we read this file for staleness detection
                        self.file_tracker.record_read(&full_path);
                        
                        ToolResult {
                            tool_call_id: tool_call.id.clone(),
                            success: true,
                            output: content,
                            error: None,
                        }
                    }
                    Err(e) => ToolResult {
                        tool_call_id: tool_call.id.clone(),
                        success: false,
                        output: String::new(),
                        error: Some(format!("Failed to read file: {}", e)),
                    }
                }
            }
            _ => ToolResult {
                tool_call_id: tool_call.id.clone(),
                success: false,
                output: String::new(),
                error: Some(format!("Unknown tool: {}", tool_call.name)),
            }
        };
        
        Ok(result)
    }
}

/// Remove JSON tool calls and code blocks from text to get clean content
fn remove_tool_calls_from_text(text: &str) -> String {
    use std::sync::LazyLock;
    static CODE_BLOCK_RE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"```(?:json)?\n.*?```").expect("invalid regex"));

    // Remove code blocks with JSON
    let text = CODE_BLOCK_RE.replace_all(text, "");
    
    // Remove raw JSON objects that look like tool calls
    // This is more aggressive - removes lines with {"tool": ...}
    let lines: Vec<&str> = text.lines().collect();
    let filtered: Vec<&str> = lines.into_iter()
        .filter(|line| !line.trim().starts_with("{\"tool\":") && !line.trim().starts_with("\"tool\""))
        .collect();
    
    filtered.join("\n")
}

#[derive(Debug)]
pub enum ExecutionOutcome {
    Text { content: String },
    ToolExecution { success: bool, results: Vec<ToolResult> },
}
