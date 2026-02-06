pub mod architect;
pub mod merger;
pub mod scheduler;
pub mod worker;

use crate::error::{NexusError, Result};
use crate::providers::Provider;
use crate::swarm::architect::{ArchitectAgent, Task, TaskStatus};
use crate::swarm::scheduler::{ExecutionPlan, Scheduler};
use crate::swarm::merger::GitMerger;
use crate::swarm::worker::{WorkerAgent, WorkerType};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

/// Configuration for the SwarmOrchestrator
#[derive(Debug, Clone)]
pub struct SwarmConfig {
    /// Maximum number of concurrent workers
    pub max_concurrent_workers: usize,
    /// Maximum number of retry attempts for failed tasks
    pub max_retries: u32,
    /// Timeout for each task in seconds
    pub task_timeout_secs: u64,
    /// Enable automatic merging of conflicts
    pub auto_merge: bool,
}

impl Default for SwarmConfig {
    fn default() -> Self {
        Self {
            max_concurrent_workers: 4,
            max_retries: 3,
            task_timeout_secs: 300,
            auto_merge: true,
        }
    }
}

/// High-level task for the swarm to execute
#[derive(Debug, Clone)]
pub struct SwarmTask {
    pub id: String,
    pub description: String,
    pub context: Option<String>,
    pub working_dir: PathBuf,
}

impl SwarmTask {
    pub fn new(description: impl Into<String>, working_dir: impl Into<PathBuf>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            description: description.into(),
            context: None,
            working_dir: working_dir.into(),
        }
    }

    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context = Some(context.into());
        self
    }
}

/// Result of a completed swarm execution
#[derive(Debug, Clone)]
pub struct SwarmResult {
    pub task_id: String,
    pub success: bool,
    pub subtask_results: Vec<SubtaskResult>,
    pub merged_files: Vec<String>,
    pub conflicts: Vec<MergeConflict>,
    pub execution_time_ms: u64,
}

/// Result of an individual subtask
#[derive(Debug, Clone)]
pub struct SubtaskResult {
    pub task_id: String,
    pub worker_type: WorkerType,
    pub success: bool,
    pub output: String,
    pub files_modified: Vec<String>,
    pub execution_time_ms: u64,
}

/// Conflicts detected during merge phase
#[derive(Debug, Clone)]
pub struct MergeConflict {
    pub file_path: String,
    pub worker_a: String,
    pub worker_b: String,
    pub resolution: ConflictResolution,
}

#[derive(Debug, Clone)]
pub enum ConflictResolution {
    AutoMerged,
    ManualRequired,
    Skipped,
}

/// The main orchestrator that manages the entire swarm
pub struct SwarmOrchestrator {
    config: SwarmConfig,
    architect: ArchitectAgent,
    scheduler: Scheduler,
    merger: GitMerger,
    workers: HashMap<WorkerType, Arc<WorkerAgent>>,
    active_tasks: Arc<RwLock<HashMap<String, TaskHandle>>>,
    provider: Arc<dyn Provider + Send + Sync>,
    model: String,
}

struct TaskHandle {
    task_id: String,
    status: TaskStatus,
    start_time: std::time::Instant,
}

impl SwarmOrchestrator {
    pub fn new(
        config: SwarmConfig,
        provider: Arc<dyn Provider + Send + Sync>,
        model: String,
    ) -> Result<Self> {
        let architect = ArchitectAgent::new(provider.clone(), model.clone())?;
        let scheduler = Scheduler::new(config.max_concurrent_workers);
        let merger = GitMerger::new(config.auto_merge);

        let mut workers = HashMap::new();
        for worker_type in [WorkerType::Frontend, WorkerType::Backend, WorkerType::QA] {
            let worker = WorkerAgent::new(
                worker_type,
                provider.clone(),
                model.clone(),
            )?;
            workers.insert(worker_type, Arc::new(worker));
        }

        Ok(Self {
            config,
            architect,
            scheduler,
            merger,
            workers,
            active_tasks: Arc::new(RwLock::new(HashMap::new())),
            provider,
            model: model.into(),
        })
    }

    /// Execute a high-level task using the swarm
    pub async fn execute(&self, swarm_task: SwarmTask) -> Result<SwarmResult> {
        let start_time = std::time::Instant::now();
        
        // Phase 1: Decomposition - Architect breaks down the task
        println!("[SWARM] Phase 1: Decomposing task...");
        let subtasks = self.architect.decompose_task(&swarm_task).await?;
        println!("[SWARM] Decomposed into {} subtasks", subtasks.len());

        // Phase 2: Scheduling - Create execution plan respecting dependencies
        println!("[SWARM] Phase 2: Creating execution plan...");
        let execution_plan = self.scheduler.create_plan(&subtasks)?;
        
        // Phase 3: Execution - Run workers in parallel
        println!("[SWARM] Phase 3: Executing subtasks in parallel...");
        let execution_results = self.execute_plan(&execution_plan, &swarm_task.working_dir).await?;

        // Phase 4: Merging - Resolve conflicts between worker outputs
        println!("[SWARM] Phase 4: Merging results...");
        let (merged_files, conflicts) = self.merger.merge_results(&execution_results).await?;

        let execution_time_ms = start_time.elapsed().as_millis() as u64;

        let success = conflicts.is_empty() || 
            conflicts.iter().all(|c| matches!(c.resolution, ConflictResolution::AutoMerged));

        Ok(SwarmResult {
            task_id: swarm_task.id,
            success,
            subtask_results: execution_results,
            merged_files,
            conflicts,
            execution_time_ms,
        })
    }

    /// Execute all tasks in the plan respecting dependencies
    async fn execute_plan(
        &self,
        plan: &ExecutionPlan,
        working_dir: &PathBuf,
    ) -> Result<Vec<SubtaskResult>> {
        let results = Arc::new(Mutex::new(Vec::new()));
        let completed_tasks = Arc::new(Mutex::new(HashMap::<String, bool>::new()));
        
        // Track which tasks are currently running
        let running_tasks = Arc::new(Mutex::new(HashMap::<String, tokio::task::JoinHandle<()>>::new()));

        // Collect all tasks from stages
        let mut pending_tasks: Vec<_> = plan.stages.iter().flat_map(|s| s.tasks.clone()).collect();
        
        while !pending_tasks.is_empty() {
            // Find tasks with satisfied dependencies
            let ready_tasks: Vec<_> = pending_tasks
                .iter()
                .filter(|t| {
                    t.dependencies.iter().all(|dep| {
                        completed_tasks.blocking_lock().get(dep).copied().unwrap_or(false)
                    })
                })
                .cloned()
                .collect();

            if ready_tasks.is_empty() && !pending_tasks.is_empty() {
                // Check if any tasks are running
                let running = running_tasks.lock().await;
                if running.is_empty() {
                    return Err(NexusError::Configuration(
                        "Circular dependency detected or all tasks blocked".to_string()
                    ));
                }
                drop(running);
                
                // Wait for some tasks to complete
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                continue;
            }

            // Spawn workers for ready tasks (up to max_concurrent limit)
            let max_to_spawn = self.config.max_concurrent_workers.saturating_sub(
                running_tasks.lock().await.len()
            );

            for task in ready_tasks.iter().take(max_to_spawn) {
                let task_id = task.id.clone();
                let task_id_for_running = task_id.clone();
                
                // Remove from pending
                pending_tasks.retain(|t| t.id != task_id);

                // Spawn worker
                let worker_type = self.determine_worker_type(&task.description);
                let worker = self.workers.get(&worker_type)
                    .ok_or_else(|| NexusError::Configuration(
                        format!("No worker available for type: {:?}", worker_type)
                    ))?
                    .clone();

                let results_clone = results.clone();
                let completed_clone = completed_tasks.clone();
                let running_clone = running_tasks.clone();
                let working_dir = working_dir.clone();
                let task = task.clone();
                let max_retries = self.config.max_retries;
                let timeout_secs = self.config.task_timeout_secs;

                let handle = tokio::spawn(async move {
                    let result = Self::execute_with_timeout(
                        worker,
                        task.clone(),
                        working_dir,
                        max_retries,
                        timeout_secs,
                    ).await;

                    // Record completion
                    completed_clone.lock().await.insert(task_id.clone(), result.is_ok());
                    
                    if let Ok(subtask_result) = result {
                        results_clone.lock().await.push(subtask_result);
                    }

                    // Remove from running
                    running_clone.lock().await.remove(&task_id);
                });

                running_tasks.lock().await.insert(task_id_for_running, handle);
            }

            // Small delay to prevent tight loop
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }

        // Wait for all remaining tasks
        let mut running = running_tasks.lock().await;
        for (_, handle) in running.drain() {
            let _ = handle.await;
        }
        drop(running);

        // Return results
        let final_results = results.lock().await.clone();
        Ok(final_results)
    }

    /// Execute a single task with timeout and retry logic
    async fn execute_with_timeout(
        worker: Arc<WorkerAgent>,
        task: Task,
        working_dir: PathBuf,
        max_retries: u32,
        timeout_secs: u64,
    ) -> Result<SubtaskResult> {
        let start_time = std::time::Instant::now();
        let mut attempts = 0;

        loop {
            attempts += 1;
            
            let worker_clone = worker.clone();
            let task_clone = task.clone();
            let working_dir_clone = working_dir.clone();
            
            let timeout_result = tokio::time::timeout(
                tokio::time::Duration::from_secs(timeout_secs),
                worker_clone.execute(&task_clone, &working_dir_clone)
            ).await;

            match timeout_result {
                Ok(Ok(result)) => {
                    return Ok(SubtaskResult {
                        task_id: task.id,
                        worker_type: worker.worker_type(),
                        success: true,
                        output: result.output,
                        files_modified: result.files_modified,
                        execution_time_ms: start_time.elapsed().as_millis() as u64,
                    });
                }
                Ok(Err(e)) => {
                    if attempts >= max_retries {
                        return Ok(SubtaskResult {
                            task_id: task.id,
                            worker_type: worker.worker_type(),
                            success: false,
                            output: format!("Failed after {} attempts: {}", attempts, e),
                            files_modified: Vec::new(),
                            execution_time_ms: start_time.elapsed().as_millis() as u64,
                        });
                    }
                    println!("  [WORKER] Task {} failed (attempt {}/{}), retrying...", 
                        task.id, attempts, max_retries);
                }
                Err(_) => {
                    if attempts >= max_retries {
                        return Ok(SubtaskResult {
                            task_id: task.id,
                            worker_type: worker.worker_type(),
                            success: false,
                            output: format!("Task timed out after {} seconds ({} attempts)", 
                                timeout_secs, attempts),
                            files_modified: Vec::new(),
                            execution_time_ms: start_time.elapsed().as_millis() as u64,
                        });
                    }
                    println!("  [WORKER] Task {} timed out (attempt {}/{}), retrying...", 
                        task.id, attempts, max_retries);
                }
            }

            // Exponential backoff
            tokio::time::sleep(tokio::time::Duration::from_millis(100 * attempts as u64)).await;
        }
    }

    /// Determine which worker type should handle a task based on description
    fn determine_worker_type(&self, description: &str) -> WorkerType {
        let desc_lower = description.to_lowercase();
        
        if desc_lower.contains("test") 
            || desc_lower.contains("validate")
            || desc_lower.contains("review")
            || desc_lower.contains("check") {
            WorkerType::QA
        } else if desc_lower.contains("ui")
            || desc_lower.contains("css")
            || desc_lower.contains("html")
            || desc_lower.contains("frontend")
            || desc_lower.contains("component")
            || desc_lower.contains("react")
            || desc_lower.contains("vue")
            || desc_lower.contains("angular") {
            WorkerType::Frontend
        } else {
            // Default to backend for API, database, logic tasks
            WorkerType::Backend
        }
    }

    /// Get the current status of all active tasks
    pub async fn get_active_tasks(&self) -> Vec<(String, TaskStatus)> {
        let tasks = self.active_tasks.read().await;
        tasks
            .iter()
            .map(|(id, handle)| (id.clone(), handle.status.clone()))
            .collect()
    }

    /// Cancel a running task
    pub async fn cancel_task(&self, task_id: &str) -> Result<()> {
        let mut tasks = self.active_tasks.write().await;
        if let Some(_handle) = tasks.remove(task_id) {
            println!("[SWARM] Cancelled task {}", task_id);
            Ok(())
        } else {
            Err(NexusError::Configuration(
                format!("Task {} not found or already completed", task_id)
            ))
        }
    }
}
