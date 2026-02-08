use crate::error::{NexusError, Result};
use crate::providers::{CompletionRequest, Message, Provider, Role};
use crate::swarm::SwarmTask;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// Status of a task in the system
#[derive(Debug, Clone, PartialEq)]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
    Failed(String),
    Cancelled,
}

/// A subtask created by the architect
#[derive(Debug, Clone)]
pub struct Task {
    pub id: String,
    pub description: String,
    pub worker_type_hint: Option<String>,
    pub dependencies: Vec<String>,
    pub estimated_effort: u32, // in minutes
    pub context: String,
    pub status: TaskStatus,
}

/// Output from the architect's decomposition
#[derive(Debug, Deserialize, Serialize)]
struct DecompositionOutput {
    subtasks: Vec<SubtaskDefinition>,
    overall_strategy: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct SubtaskDefinition {
    id: String,
    description: String,
    #[serde(rename = "type")]
    task_type: String,
    dependencies: Vec<String>,
    estimated_minutes: u32,
}

/// The Architect Agent decomposes high-level tasks into subtasks
pub struct ArchitectAgent {
    provider: Arc<dyn Provider + Send + Sync>,
    model: String,
}

impl ArchitectAgent {
    pub fn new(
        provider: Arc<dyn Provider + Send + Sync>,
        model: impl Into<String>,
    ) -> Result<Self> {
        Ok(Self {
            provider,
            model: model.into(),
        })
    }

    /// Decompose a high-level task into subtasks
    pub async fn decompose_task(&self, swarm_task: &SwarmTask) -> Result<Vec<Task>> {
        let prompt = self.build_decomposition_prompt(swarm_task);
        
        let messages = vec![
            Message {
                role: Role::System,
                content: ARCHITECT_SYSTEM_PROMPT.to_string(),
                name: None,
            },
            Message {
                role: Role::User,
                content: prompt,
                name: None,
            },
        ];

        let request = CompletionRequest {
            model: self.model.clone(),
            messages,
            temperature: Some(0.3), // Lower temperature for consistent decomposition
            max_tokens: Some(4096),
            stream: Some(false),
                tools: None,
            extra_params: None,
        };

        let response = self.provider.complete(request).await?;
        
        // Parse the response into structured tasks
        let tasks = self.parse_decomposition_response(&response.content)?;
        
        // Build dependency graph
        self.validate_dependencies(&tasks)?;
        
        Ok(tasks)
    }

    /// Create a dependency graph for visualization or analysis
    pub fn create_dependency_graph(&self, tasks: &[Task]) -> HashMap<String, Vec<String>> {
        let mut graph = HashMap::new();
        
        for task in tasks {
            let deps: Vec<String> = task.dependencies.clone();
            graph.insert(task.id.clone(), deps);
        }
        
        graph
    }

    /// Find the critical path through tasks
    pub fn find_critical_path(&self, tasks: &[Task]) -> Vec<String> {
        let graph = self.create_dependency_graph(tasks);
        let task_map: HashMap<String, &Task> = tasks.iter()
            .map(|t| (t.id.clone(), t))
            .collect();
        
        // Calculate longest path using topological sort
        let mut in_degree: HashMap<String, usize> = HashMap::new();
        for (id, deps) in &graph {
            in_degree.entry(id.clone()).or_insert(0);
            for dep in deps {
                *in_degree.entry(dep.clone()).or_insert(0) += 1;
            }
        }
        
        // Find start nodes (no dependencies)
        let mut queue: Vec<String> = in_degree
            .iter()
            .filter(|&(_, &deg)| deg == 0)
            .map(|(id, _)| id.clone())
            .collect();
        
        let mut longest_time: HashMap<String, u32> = HashMap::new();
        let mut path: HashMap<String, Vec<String>> = HashMap::new();
        
        for id in &queue {
            longest_time.insert(id.clone(), task_map.get(id)
                .map(|t| t.estimated_effort)
                .unwrap_or(0));
            path.insert(id.clone(), vec![id.clone()]);
        }
        
        // Process in topological order
        while let Some(current) = queue.pop() {
            // Find tasks that depend on current
            for (task_id, deps) in &graph {
                if deps.contains(&current) {
                    let current_time = longest_time.get(&current).copied().unwrap_or(0);
                    let task_time = task_map.get(task_id)
                        .map(|t| t.estimated_effort)
                        .unwrap_or(0);
                    let new_time = current_time + task_time;
                    
                    if new_time > longest_time.get(task_id).copied().unwrap_or(0) {
                        longest_time.insert(task_id.clone(), new_time);
                        let mut new_path = path.get(&current).cloned().unwrap_or_default();
                        new_path.push(task_id.clone());
                        path.insert(task_id.clone(), new_path);
                    }
                    
                    let deg = in_degree.get_mut(task_id).unwrap();
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push(task_id.clone());
                    }
                }
            }
        }
        
        // Find the path with maximum time
        path.values()
            .max_by_key(|p| {
                p.iter()
                    .filter_map(|id| longest_time.get(id))
                    .sum::<u32>()
            })
            .cloned()
            .unwrap_or_default()
    }

    fn build_decomposition_prompt(&self, swarm_task: &SwarmTask) -> String {
        let context_str = swarm_task.context.as_ref()
            .map(|c| format!("\nAdditional Context:\n{}", c))
            .unwrap_or_default();

        format!(
            "Decompose the following high-level task into specific, actionable subtasks.\n\n\
            Task: {}\n{}\n\n\
            Instructions:\n\
            1. Break down the task into 3-8 subtasks\n\
            2. Assign each subtask a type: 'frontend', 'backend', or 'qa'\n\
            3. Specify dependencies between subtasks\n\
            4. Estimate effort in minutes for each subtask\n\
            5. Ensure subtasks are specific and implementable\n\n\
            Respond with JSON in this format:\n\
            {{\n\
              \"subtasks\": [\n\
                {{\n\
                  \"id\": \"subtask-1\",\n\
                  \"description\": \"Description of what to do\",\n\
                  \"type\": \"frontend\",\n\
                  \"dependencies\": [],\n\
                  \"estimated_minutes\": 30\n\
                }}\n\
              ],\n\
              \"overall_strategy\": \"Brief description of overall approach\"\n\
            }}",
            swarm_task.description,
            context_str
        )
    }

    fn parse_decomposition_response(&self, content: &str) -> Result<Vec<Task>> {
        // Try to extract JSON from the response
        let json_str = if content.contains("```json") {
            content.split("```json")
                .nth(1)
                .and_then(|s| s.split("```").next())
                .unwrap_or(content)
                .trim()
        } else if content.contains("```") {
            content.split("```")
                .nth(1)
                .unwrap_or(content)
                .trim()
        } else {
            content.trim()
        };

        let decomposition: DecompositionOutput = serde_json::from_str(json_str)
            .map_err(|e| NexusError::Json(e))?;

        let tasks: Vec<Task> = decomposition.subtasks.into_iter()
            .map(|def| Task {
                id: def.id,
                description: def.description,
                worker_type_hint: Some(def.task_type),
                dependencies: def.dependencies,
                estimated_effort: def.estimated_minutes,
                context: String::new(),
                status: TaskStatus::Pending,
            })
            .collect();

        Ok(tasks)
    }

    fn validate_dependencies(&self, tasks: &[Task]) -> Result<()> {
        let task_ids: std::collections::HashSet<String> = tasks
            .iter()
            .map(|t| t.id.clone())
            .collect();

        // Check all dependencies exist
        for task in tasks {
            for dep in &task.dependencies {
                if !task_ids.contains(dep) {
                    return Err(NexusError::Configuration(
                        format!("Task {} has unknown dependency: {}", task.id, dep)
                    ));
                }
            }
        }

        // Check for circular dependencies
        let graph = self.create_dependency_graph(tasks);
        if self.has_cycle(&graph) {
            return Err(NexusError::Configuration(
                "Circular dependency detected in task graph".to_string()
            ));
        }

        Ok(())
    }

    fn has_cycle(&self, graph: &HashMap<String, Vec<String>>) -> bool {
        let mut visited = std::collections::HashSet::new();
        let mut rec_stack = std::collections::HashSet::new();

        fn dfs(
            node: &str,
            graph: &HashMap<String, Vec<String>>,
            visited: &mut std::collections::HashSet<String>,
            rec_stack: &mut std::collections::HashSet<String>,
        ) -> bool {
            visited.insert(node.to_string());
            rec_stack.insert(node.to_string());

            if let Some(neighbors) = graph.get(node) {
                for neighbor in neighbors {
                    if !visited.contains(neighbor) {
                        if dfs(neighbor, graph, visited, rec_stack) {
                            return true;
                        }
                    } else if rec_stack.contains(neighbor) {
                        return true;
                    }
                }
            }

            rec_stack.remove(node);
            false
        }

        for node in graph.keys() {
            if !visited.contains(node) {
                if dfs(node, graph, &mut visited, &mut rec_stack) {
                    return true;
                }
            }
        }

        false
    }
}

const ARCHITECT_SYSTEM_PROMPT: &str = r#"You are the Architect Agent for a software development swarm system.

Your role is to decompose high-level tasks into specific, actionable subtasks that can be executed by specialized worker agents.

Guidelines:
1. Create clear, specific subtasks with well-defined scope
2. Assign appropriate worker types: 'frontend' (UI/CSS/HTML), 'backend' (APIs/databases/logic), or 'qa' (testing/validation)
3. Define dependencies carefully - tests usually depend on implementation
4. Keep subtasks focused and implementable (15-60 minutes each)
5. Consider the logical flow and data dependencies between components

Response Format:
You must respond with valid JSON containing:
- subtasks: Array of task definitions
- overall_strategy: Brief description of the approach

Each subtask must have:
- id: Unique identifier (e.g., "subtask-1")
- description: Clear, actionable description
- type: One of "frontend", "backend", or "qa"
- dependencies: Array of subtask IDs this depends on
- estimated_minutes: Realistic time estimate

Think step by step and create a coherent plan that the swarm can execute in parallel where possible."#;
