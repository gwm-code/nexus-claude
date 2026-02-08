use crate::context::FileAccessTracker;
use crate::error::Result;
use tracing::{debug, info, warn};
use crate::providers::{CompletionRequest, Message, Role};
use crate::providers::retry::retry_with_backoff;
use crate::providers::token_budget::TokenBudget;
use crate::sandbox::SandboxManager;
use crate::sandbox::hydration::{Hydrator, HydrationPlan, FileChange};
use crate::executor::tools::{ToolCall, ToolResult, parse_tool_calls};
use std::time::Duration;

/// Maximum tool-calling turns before forcing termination
const MAX_TURNS: usize = 20;

/// The Agent runs multi-turn conversations with tool calling
pub struct Agent {
    sandbox: SandboxManager,
    hydrator: Hydrator,
    working_dir: std::path::PathBuf,
    file_tracker: FileAccessTracker,
}

impl Agent {
    pub fn new(working_dir: std::path::PathBuf) -> Result<Self> {
        Ok(Self {
            sandbox: SandboxManager::new(),
            hydrator: Hydrator::new()?,
            working_dir,
            file_tracker: FileAccessTracker::new(),
        })
    }

    /// Get a reference to the file access tracker
    pub fn file_tracker(&self) -> &FileAccessTracker {
        &self.file_tracker
    }

    /// Run a complete task - handles multi-turn tool calling automatically
    pub async fn run_task(
        &self,
        messages: &mut Vec<Message>,
        provider: &dyn crate::providers::Provider,
        model: String,
    ) -> Result<String> {
        let mut budget = TokenBudget::default();

        // Agent loop: keep going until no more tool calls (with safety limit)
        for turn in 0..MAX_TURNS {
            // Check token budget before sending
            if !budget.can_continue() {
                warn!(used = budget.used_input_tokens + budget.used_output_tokens, remaining = budget.remaining(), "Token budget exhausted");
                return Ok(format!("[Agent stopped: token budget exhausted after {} turns. Used {} tokens.]", turn, budget.used_input_tokens + budget.used_output_tokens));
            }

            // Estimate input tokens
            let input_estimate: u32 = messages.iter().map(|m| TokenBudget::estimate_tokens(&m.content)).sum();

            // Send request to AI with tools
            let request = CompletionRequest {
                model: model.clone(),
                messages: messages.clone(),
                temperature: Some(0.7),
                max_tokens: Some(budget.dynamic_max_tokens()),
                stream: Some(false),
                tools: Some(crate::executor::tools::get_available_tools()),
                extra_params: None,
            };

            let response = retry_with_backoff(3, Duration::from_secs(1), || {
                let req = request.clone();
                async move { provider.complete(req).await }
            })
            .await?;

            // Record token usage
            let output_estimate = TokenBudget::estimate_tokens(&response.content);
            if let Some(ref usage) = response.usage {
                budget.record_usage(usage.prompt_tokens, usage.completion_tokens);
            } else {
                budget.record_usage(input_estimate, output_estimate);
            }
            info!(input_tokens = budget.used_input_tokens, output_tokens = budget.used_output_tokens, remaining = budget.remaining(), "Token usage");

            // Check if response has tool calls (use native tool_calls if available, fallback to parsing)
            let tool_calls = response.tool_calls.clone().unwrap_or_else(|| {
                // Fallback: parse from content for providers without native tool calling
                parse_tool_calls(&response.content)
            });

            if tool_calls.is_empty() {
                // No tools - AI is done, return final answer
                messages.push(Message {
                    role: Role::Assistant,
                    content: response.content.clone(),
                    name: None,
                });
                return Ok(response.content);
            }

            // Execute tools
            info!(count = tool_calls.len(), "Executing tools");
            let mut tool_results = Vec::new();

            for tool_call in tool_calls {
                let result = self.execute_tool(&tool_call).await?;

                // Log to stderr (not stdout) so it doesn't pollute chat responses
                if result.success {
                    debug!(tool = %tool_call.name, "Tool executed successfully");
                } else {
                    debug!(tool = %tool_call.name, error = ?result.error, "Tool execution failed");
                }

                tool_results.push(result);
            }

            // Add assistant message (with tool calls)
            messages.push(Message {
                role: Role::Assistant,
                content: response.content,
                name: None,
            });

            // Add tool results as system messages
            for result in &tool_results {
                let tool_content = format!(
                    "Tool '{}' result:\nSuccess: {}\nOutput: {}\nError: {}",
                    result.tool_call_id,
                    result.success,
                    result.output,
                    result.error.as_ref().unwrap_or(&"None".to_string())
                );
                
                messages.push(Message {
                    role: Role::Tool,
                    content: tool_content,
                    name: Some(result.tool_call_id.clone()),
                });
            }

            // Loop continues - AI will see tool results and respond
            info!("AI analyzing tool results");
        }

        // Safety: if we exhausted MAX_TURNS without a final response
        Ok(format!("[Agent reached max turns limit ({}). Last tool results were processed but no final summary was generated.]", MAX_TURNS))
    }

    async fn execute_tool(&self, tool_call: &ToolCall) -> Result<ToolResult> {
        let result = match tool_call.name.as_str() {
            "execute_command" => {
                let command = tool_call.arguments.get("command")
                    .and_then(|c| c.as_str())
                    .unwrap_or_default();
                
                // Use network for package managers
                let is_package_command = command.contains("npm") || 
                                        command.contains("pip") || 
                                        command.contains("apk") || 
                                        command.contains("apt") || 
                                        command.contains("dnf") || 
                                        command.contains("yarn") || 
                                        command.contains("cargo install");
                
                let shadow_result = if is_package_command {
                    self.sandbox.shadow_run_with_network(command, &self.working_dir).await?
                } else {
                    self.sandbox.shadow_run(command, &self.working_dir).await?
                };
                
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
                
                let change = FileChange {
                    path: std::path::PathBuf::from(path),
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
                    path: std::path::PathBuf::from(path),
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
