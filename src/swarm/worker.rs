use crate::context::FileAccessTracker;
use crate::error::Result;
use crate::executor::tools::{ToolCall, ToolResult, create_tool_system_prompt, parse_tool_calls};
use crate::providers::{CompletionRequest, Message, Provider, Role};
use crate::sandbox::SandboxManager;
use crate::sandbox::hydration::{FileChange, HydrationPlan, Hydrator};
use crate::swarm::architect::Task;
use std::path::PathBuf;
use std::sync::Arc;

/// Maximum tool-calling turns before forcing termination
const MAX_TURNS: usize = 20;

/// Types of specialized workers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkerType {
    Frontend,
    Backend,
    QA,
}

impl WorkerType {
    pub fn as_str(&self) -> &'static str {
        match self {
            WorkerType::Frontend => "frontend",
            WorkerType::Backend => "backend",
            WorkerType::QA => "qa",
        }
    }

    pub fn get_system_prompt(&self) -> &'static str {
        match self {
            WorkerType::Frontend => FRONTEND_WORKER_PROMPT,
            WorkerType::Backend => BACKEND_WORKER_PROMPT,
            WorkerType::QA => QA_WORKER_PROMPT,
        }
    }
}

/// Result of a worker execution
#[derive(Debug, Clone)]
pub struct WorkerResult {
    pub task_id: String,
    pub output: String,
    pub files_modified: Vec<String>,
    pub tests_passed: Option<bool>,
}

/// A specialized worker agent that executes specific tasks
pub struct WorkerAgent {
    worker_type: WorkerType,
    provider: Arc<dyn Provider + Send + Sync>,
    model: String,
    sandbox: SandboxManager,
    hydrator: Hydrator,
    file_tracker: FileAccessTracker,
}

impl WorkerAgent {
    pub fn new(
        worker_type: WorkerType,
        provider: Arc<dyn Provider + Send + Sync>,
        model: impl Into<String>,
    ) -> Result<Self> {
        Ok(Self {
            worker_type,
            provider,
            model: model.into(),
            sandbox: SandboxManager::new(),
            hydrator: Hydrator::new()?,
            file_tracker: FileAccessTracker::new(),
        })
    }

    pub fn worker_type(&self) -> WorkerType {
        self.worker_type
    }

    /// Execute a task with multi-turn tool calling and return the result
    pub async fn execute(
        &self,
        task: &Task,
        working_dir: &PathBuf,
    ) -> Result<WorkerResult> {
        println!(
            "  [WORKER {:?}] Starting task: {} - {}",
            self.worker_type,
            task.id,
            &task.description[..task.description.len().min(50)]
        );

        let tool_prompt = create_tool_system_prompt();

        let system_prompt = format!(
            "{}\n\n{}\n\nWorking directory: {}\nTask context: {}",
            self.worker_type.get_system_prompt(),
            tool_prompt,
            working_dir.display(),
            task.context
        );

        let user_message = format!(
            "Execute the following task:\n\nTask ID: {}\nDescription: {}\n\n\
            Use the available tools to create or modify files as needed.\n\n\
            When complete, summarize:\n\
            1. What was implemented\n\
            2. Files created or modified\n\
            3. Any tests or validation performed",
            task.id,
            task.description
        );

        let mut messages = vec![
            Message {
                role: Role::System,
                content: system_prompt,
                name: None,
            },
            Message {
                role: Role::User,
                content: user_message,
                name: None,
            },
        ];

        let mut final_response = String::new();

        // Multi-turn tool calling loop
        for turn in 0..MAX_TURNS {
            let request = CompletionRequest {
                model: self.model.clone(),
                messages: messages.clone(),
                temperature: Some(0.7),
                max_tokens: Some(4096),
                stream: Some(false),
                extra_params: None,
            };

            let response = self.provider.complete(request).await?;

            // Check for tool calls
            let tool_calls = parse_tool_calls(&response.content);

            if tool_calls.is_empty() {
                // No tools — done, this is the final response
                messages.push(Message {
                    role: Role::Assistant,
                    content: response.content.clone(),
                    name: None,
                });
                final_response = response.content;
                break;
            }

            // Execute tools
            println!(
                "  [WORKER {:?}] Turn {}: Executing {} tool(s)...",
                self.worker_type,
                turn + 1,
                tool_calls.len()
            );
            let mut tool_results = Vec::new();

            for tool_call in &tool_calls {
                let result = self.execute_tool(tool_call, working_dir).await?;

                if result.success {
                    println!("    ✓ {} - Success", tool_call.name);
                } else {
                    println!(
                        "    ✗ {} - Failed: {}",
                        tool_call.name,
                        result.error.as_ref().unwrap_or(&"Unknown".to_string())
                    );
                }

                tool_results.push(result);
            }

            // Add assistant message (with tool calls)
            messages.push(Message {
                role: Role::Assistant,
                content: response.content,
                name: None,
            });

            // Add tool results as tool messages
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

            // If this is the last allowed turn, record what we have
            if turn == MAX_TURNS - 1 {
                final_response = format!(
                    "[Worker hit MAX_TURNS limit ({})]\nLast tool results processed.",
                    MAX_TURNS
                );
            }
        }

        // Parse the final response to extract file modifications
        let files_modified = self.extract_files_modified(&final_response);
        let tests_passed = self.check_tests_passed(&final_response);

        println!(
            "  [WORKER {:?}] Completed task {} - Modified {} files",
            self.worker_type,
            task.id,
            files_modified.len()
        );

        Ok(WorkerResult {
            task_id: task.id.clone(),
            output: final_response,
            files_modified,
            tests_passed,
        })
    }

    /// Execute a single tool call (mirrors Agent::execute_tool)
    async fn execute_tool(&self, tool_call: &ToolCall, working_dir: &PathBuf) -> Result<ToolResult> {
        let result = match tool_call.name.as_str() {
            "execute_command" => {
                let command = tool_call
                    .arguments
                    .get("command")
                    .and_then(|c| c.as_str())
                    .unwrap_or_default();

                let is_package_command = command.contains("npm")
                    || command.contains("pip")
                    || command.contains("apk")
                    || command.contains("apt")
                    || command.contains("dnf")
                    || command.contains("yarn")
                    || command.contains("cargo install");

                let shadow_result = if is_package_command {
                    self.sandbox
                        .shadow_run_with_network(command, working_dir)
                        .await?
                } else {
                    self.sandbox.shadow_run(command, working_dir).await?
                };

                ToolResult {
                    tool_call_id: tool_call.id.clone(),
                    success: shadow_result.success,
                    output: format!(
                        "stdout: {}\nstderr: {}",
                        shadow_result.stdout, shadow_result.stderr
                    ),
                    error: if shadow_result.success {
                        None
                    } else {
                        Some(format!("Exit code: {}", shadow_result.exit_code))
                    },
                }
            }
            "create_file" => {
                let path = tool_call
                    .arguments
                    .get("path")
                    .and_then(|p| p.as_str())
                    .unwrap_or_default();
                let content = tool_call
                    .arguments
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or_default();

                let plan = HydrationPlan {
                    files_to_create: vec![FileChange {
                        path: PathBuf::from(path),
                        content: content.to_string(),
                        backup_path: None,
                    }],
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
                    },
                }
            }
            "edit_file" => {
                let path = tool_call
                    .arguments
                    .get("path")
                    .and_then(|p| p.as_str())
                    .unwrap_or_default();
                let content = tool_call
                    .arguments
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or_default();

                let full_path = working_dir.join(path);

                // Check staleness
                if let Err(e) = self.file_tracker.check_staleness(&full_path) {
                    return Ok(ToolResult {
                        tool_call_id: tool_call.id.clone(),
                        success: false,
                        output: String::new(),
                        error: Some(e.to_string()),
                    });
                }

                let plan = HydrationPlan {
                    files_to_create: Vec::new(),
                    files_to_update: vec![FileChange {
                        path: PathBuf::from(path),
                        content: content.to_string(),
                        backup_path: None,
                    }],
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
                    },
                }
            }
            "read_file" => {
                let path = tool_call
                    .arguments
                    .get("path")
                    .and_then(|p| p.as_str())
                    .unwrap_or_default();

                let full_path = working_dir.join(path);
                match std::fs::read_to_string(&full_path) {
                    Ok(content) => {
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
                    },
                }
            }
            _ => ToolResult {
                tool_call_id: tool_call.id.clone(),
                success: false,
                output: String::new(),
                error: Some(format!("Unknown tool: {}", tool_call.name)),
            },
        };

        Ok(result)
    }

    /// Extract file paths from the worker's response
    fn extract_files_modified(&self, content: &str) -> Vec<String> {
        let mut files = Vec::new();

        let patterns = [
            r"Created file:\s*(.+)",
            r"Modified file:\s*(.+)",
            r"Updated file:\s*(.+)",
            r"`(.+\.\w+)`",
            r"\*\*(.+\.\w+)\*\*",
        ];

        for pattern in &patterns {
            let regex = regex::Regex::new(pattern).ok();
            if let Some(re) = regex {
                for cap in re.captures_iter(content) {
                    if let Some(matched) = cap.get(1) {
                        let path = matched.as_str().trim().to_string();
                        if !files.contains(&path) {
                            files.push(path);
                        }
                    }
                }
            }
        }

        files
    }

    /// Check if tests passed based on the response
    fn check_tests_passed(&self, content: &str) -> Option<bool> {
        let content_lower = content.to_lowercase();

        if content_lower.contains("test passed")
            || content_lower.contains("tests passed")
            || content_lower.contains("✓ all tests")
            || content_lower.contains("validation successful")
        {
            Some(true)
        } else if content_lower.contains("test failed")
            || content_lower.contains("tests failed")
            || content_lower.contains("✗ test")
            || content_lower.contains("validation failed")
        {
            Some(false)
        } else {
            None
        }
    }
}

const FRONTEND_WORKER_PROMPT: &str = r#"You are a Frontend Worker in a software development swarm.

Your expertise: UI components, CSS, HTML, JavaScript/TypeScript, React, Vue, Angular, styling, responsive design, accessibility, and client-side interactions.

Your role:
1. Create clean, semantic HTML and CSS
2. Build reusable UI components with proper state management
3. Ensure responsive design and accessibility (ARIA labels, keyboard navigation)
4. Follow modern frontend best practices
5. Use available tools to create and edit files

Guidelines:
- Create maintainable, modular code
- Add comments for complex logic
- Consider mobile responsiveness
- Follow existing project conventions
- Validate HTML/CSS when possible

When implementing:
1. Check existing files and patterns first
2. Create files in appropriate locations
3. Use consistent naming conventions
4. Add styling that matches the project aesthetic
5. Include any necessary TypeScript types

Always confirm what files you created or modified in your response."#;

const BACKEND_WORKER_PROMPT: &str = r#"You are a Backend Worker in a software development swarm.

Your expertise: APIs, databases, business logic, server-side code, authentication, data models, and backend architecture.

Your role:
1. Design and implement RESTful or GraphQL APIs
2. Create database schemas and queries
3. Implement business logic and data processing
4. Set up authentication and authorization
5. Ensure security best practices
6. Use available tools to create and edit files

Guidelines:
- Write clean, testable code
- Follow API design best practices
- Implement proper error handling
- Consider performance and scalability
- Add appropriate logging
- Follow existing project patterns

When implementing:
1. Review existing API patterns and database models
2. Design endpoints with clear contracts
3. Implement proper validation and error handling
4. Add database migrations if needed
5. Include authentication checks where required
6. Document API endpoints

Always confirm what files you created or modified in your response."#;

const QA_WORKER_PROMPT: &str = r#"You are a QA Worker in a software development swarm.

Your expertise: Testing strategies, test automation, code review, validation, edge case analysis, and quality assurance.

Your role:
1. Write comprehensive test suites (unit, integration, e2e)
2. Review code for correctness and best practices
3. Validate implementations against requirements
4. Identify edge cases and potential issues
5. Run tests and report results
6. Use available tools to create, edit, and validate files

Guidelines:
- Write clear, descriptive test cases
- Cover happy paths and edge cases
- Follow testing best practices for the project
- Provide actionable feedback
- Run tests before reporting completion
- Flag any issues or concerns

When testing:
1. Read the implementation first
2. Understand the requirements and expected behavior
3. Create tests that verify correctness
4. Test edge cases and error conditions
5. Run the tests and verify results
6. Report any failures with clear explanations

Always report:
- Test coverage summary
- Any failures or issues found
- Recommendations for improvements

Be thorough and critical - quality is your priority."#;
