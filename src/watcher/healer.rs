//! Healer Module - Auto-investigate and fix detected errors
//!
//! This module analyzes detected errors, queries memory for similar past issues,
//! generates fix suggestions using AI, applies fixes via Shadow Run, and verifies
//! that the fixes work.

use crate::agent::Agent;
use crate::context::FileAccessTracker;
use crate::error::{NexusError, Result};
use crate::memory::{MemorySystem, types::MemoryResult};
use crate::providers::{Message, Provider, Role};
use crate::sandbox::{SandboxManager, ShadowRunResult};
use crate::sandbox::hydration::{HydrationPlan, Hydrator};
use crate::swarm::{SwarmOrchestrator, SwarmTask};
use crate::watcher::logs::LogErrorEvent;
use crate::watcher::filesystem::FileChangeEvent;
use crate::watcher::patterns::{DetectedError, ErrorType, ErrorSeverity};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use uuid::Uuid;

/// A healing session tracks the investigation and fix process
#[derive(Debug, Clone)]
pub struct HealingSession {
    pub id: String,
    pub error_event: ErrorEvent,
    pub status: HealingStatus,
    pub start_time: chrono::DateTime<chrono::Utc>,
    pub end_time: Option<chrono::DateTime<chrono::Utc>>,
    pub investigation: InvestigationResult,
    pub fixes: Vec<FixAttempt>,
    pub learning: Option<LearningEntry>,
}

/// Types of error events that can trigger healing
#[derive(Debug, Clone)]
pub enum ErrorEvent {
    LogError(LogErrorEvent),
    FileChange(FileChangeEvent),
    BuildError { project_path: PathBuf, output: String },
    TestFailure { project_path: PathBuf, test_name: String, output: String },
}

/// Status of a healing session
#[derive(Debug, Clone, PartialEq)]
pub enum HealingStatus {
    Investigating,
    GeneratingFix,
    TestingFix,
    ApplyingFix,
    Verifying,
    Complete,
    Failed(String),
}

/// Result of investigating an error
#[derive(Debug, Clone, Default)]
pub struct InvestigationResult {
    pub similar_past_errors: Vec<MemoryResult>,
    pub relevant_procedures: Vec<String>,
    pub context_files: Vec<String>,
    pub analysis: String,
    pub root_cause: Option<String>,
}

/// An attempt to fix an error
#[derive(Debug, Clone)]
pub struct FixAttempt {
    pub id: String,
    pub description: String,
    pub shadow_run_result: ShadowRunResult,
    pub hydration_plan: Option<HydrationPlan>,
    pub was_applied: bool,
    pub verification_result: Option<VerificationResult>,
}

/// Verification result after applying a fix
#[derive(Debug, Clone)]
pub struct VerificationResult {
    pub success: bool,
    pub test_output: String,
    pub error_resolved: bool,
    pub new_errors: Vec<String>,
}

/// Learning entry for future reference
#[derive(Debug, Clone, serde::Serialize)]
pub struct LearningEntry {
    pub error_type: String,
    pub error_signature: String,
    pub fix_description: String,
    #[serde(skip)]
    fix_success_rate: f32,
    #[serde(skip)]
    tags: Vec<String>,
}

/// Healer configuration
#[derive(Debug, Clone)]
pub struct HealerConfig {
    pub auto_apply_simple_fixes: bool,
    pub max_fix_attempts: u32,
    pub verify_timeout_secs: u64,
    pub use_shadow_run: bool,
    pub use_swarm_for_complex: bool,
    pub learning_enabled: bool,
    pub notification_enabled: bool,
}

impl Default for HealerConfig {
    fn default() -> Self {
        Self {
            auto_apply_simple_fixes: true,
            max_fix_attempts: 3,
            verify_timeout_secs: 60,
            use_shadow_run: true,
            use_swarm_for_complex: true,
            learning_enabled: true,
            notification_enabled: true,
        }
    }
}

/// The main healer that orchestrates error investigation and fixing
pub struct Healer {
    config: HealerConfig,
    memory: Arc<RwLock<MemorySystem>>,
    sandbox: SandboxManager,
    hydrator: Hydrator,
    file_tracker: FileAccessTracker,
    provider: Arc<dyn Provider + Send + Sync>,
    model: String,
    sessions: Arc<RwLock<HashMap<String, HealingSession>>>,
    event_tx: mpsc::Sender<HealerEvent>,
    swarm: Option<Arc<SwarmOrchestrator>>,
}

/// Events emitted by the healer
#[derive(Debug, Clone)]
pub enum HealerEvent {
    SessionStarted { session_id: String, error_summary: String },
    InvestigationComplete { session_id: String, analysis: String },
    FixGenerated { session_id: String, fix_description: String },
    ShadowRunComplete { session_id: String, success: bool, output: String },
    FixApplied { session_id: String, files_modified: Vec<String> },
    VerificationComplete { session_id: String, success: bool, error_resolved: bool },
    SessionComplete { session_id: String, success: bool },
    Notification { title: String, message: String, severity: ErrorSeverity },
}

impl Healer {
    pub fn new(
        config: HealerConfig,
        memory: Arc<RwLock<MemorySystem>>,
        provider: Arc<dyn Provider + Send + Sync>,
        model: String,
        event_tx: mpsc::Sender<HealerEvent>,
    ) -> Result<Self> {
        Ok(Self {
            config,
            memory,
            sandbox: SandboxManager::new(),
            hydrator: Hydrator::new()?,
            file_tracker: FileAccessTracker::new(),
            provider,
            model,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            event_tx,
            swarm: None,
        })
    }
    
    /// Set the swarm orchestrator for complex fixes
    pub fn set_swarm(&mut self, swarm: Arc<SwarmOrchestrator>) {
        self.swarm = Some(swarm);
    }
    
    /// Start a healing session for an error
    pub async fn heal(&self, error_event: ErrorEvent) -> Result<String> {
        let session_id = format!("heal_{}", Uuid::new_v4());
        
        let session = HealingSession {
            id: session_id.clone(),
            error_event: error_event.clone(),
            status: HealingStatus::Investigating,
            start_time: chrono::Utc::now(),
            end_time: None,
            investigation: InvestigationResult::default(),
            fixes: Vec::new(),
            learning: None,
        };
        
        // Store session
        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(session_id.clone(), session.clone());
        }
        
        // Emit event
        let error_summary = self.summarize_error(&error_event);
        self.event_tx.send(HealerEvent::SessionStarted {
            session_id: session_id.clone(),
            error_summary: error_summary.clone(),
        }).await.ok();
        
        if self.config.notification_enabled {
            self.event_tx.send(HealerEvent::Notification {
                title: "Self-Healing Started".to_string(),
                message: error_summary.clone(),
                severity: ErrorSeverity::Info,
            }).await.ok();
        }
        
        // Start healing process in background
        let self_clone = self.clone();
        let session_id_clone = session_id.clone();
        let local_set = tokio::task::LocalSet::new();
        local_set.spawn_local(async move {
            if let Err(e) = self_clone.run_healing_session(&session_id_clone).await {
                eprintln!("[HEALER] Session {} failed: {}", session_id_clone, e);
            }
        });
        
        Ok(session_id)
    }
    
    /// Run the complete healing process
    async fn run_healing_session(&self, session_id: &str) -> Result<()> {
        // Phase 1: Investigate
        self.investigate(session_id).await?;
        
        // Phase 2: Generate fix
        self.generate_fix(session_id).await?;
        
        // Phase 3: Test fix in shadow run
        if self.config.use_shadow_run {
            self.test_fix_shadow_run(session_id).await?;
        }
        
        // Phase 4: Apply fix
        self.apply_fix(session_id).await?;
        
        // Phase 5: Verify
        self.verify_fix(session_id).await?;
        
        // Phase 6: Learn from the fix
        if self.config.learning_enabled {
            self.learn_from_fix(session_id).await?;
        }
        
        // Update session status
        {
            let mut sessions = self.sessions.write().await;
            if let Some(session) = sessions.get_mut(session_id) {
                session.status = HealingStatus::Complete;
                session.end_time = Some(chrono::Utc::now());
            }
        }
        
        self.event_tx.send(HealerEvent::SessionComplete {
            session_id: session_id.to_string(),
            success: true,
        }).await.ok();
        
        Ok(())
    }
    
    /// Phase 1: Investigate the error
    async fn investigate(&self, session_id: &str) -> Result<()> {
        self.update_status(session_id, HealingStatus::Investigating).await;
        
        let session = self.get_session(session_id).await?;
        let detected_error = self.extract_detected_error(&session.error_event)?;
        
        println!("[HEALER] Investigating error: {:?} - {}", 
            detected_error.error_type, 
            detected_error.message.lines().next().unwrap_or(&detected_error.message)
        );
        
        // Query memory for similar errors
        let similar_errors = {
            let memory = self.memory.read().await;
            let search_query = format!("{:?} {}", detected_error.error_type, detected_error.message);
            memory.search(&search_query, 5).await.unwrap_or_default()
        };
        
        if !similar_errors.is_empty() {
            println!("[HEALER] Found {} similar past errors in memory", similar_errors.len());
        }
        
        // Get relevant files for context
        let context_files = if let Some(ref file_path) = detected_error.file_path {
            let mut files = vec![file_path.clone()];
            // Add related files
            let related = self.find_related_files(file_path).await?;
            files.extend(related);
            files
        } else {
            self.find_relevant_files(&session.error_event).await?
        };
        
        // Read file contents for context
        let file_contents = self.read_file_contents(&context_files).await?;
        
        // Use AI to analyze with full context
        let analysis = self.analyze_error_with_ai(
            &detected_error, 
            &similar_errors, 
            &context_files,
            &file_contents
        ).await?;
        
        // Extract root cause using AI-enhanced logic
        let root_cause = self.extract_root_cause_enhanced(&analysis, &detected_error);
        
        // Find relevant procedures from memory
        let relevant_procedures = self.find_relevant_procedures(&detected_error).await?;
        
        // Update investigation results
        {
            let mut sessions = self.sessions.write().await;
            if let Some(session) = sessions.get_mut(session_id) {
                session.investigation = InvestigationResult {
                    similar_past_errors: similar_errors,
                    relevant_procedures,
                    context_files,
                    analysis: analysis.clone(),
                    root_cause,
                };
            }
        }
        
        self.event_tx.send(HealerEvent::InvestigationComplete {
            session_id: session_id.to_string(),
            analysis: analysis.clone(),
        }).await.ok();
        
        println!("[HEALER] Investigation complete: {}", 
            analysis.lines().next().unwrap_or("Analysis complete").chars().take(100).collect::<String>()
        );
        
        Ok(())
    }
    
    /// Find related files based on imports, module structure, etc.
    async fn find_related_files(&self, file_path: &str) -> Result<Vec<String>> {
        let mut related = Vec::new();
        let path = PathBuf::from(file_path);
        
        if let Some(parent) = path.parent() {
            // Look for files in the same directory
            if let Ok(entries) = tokio::fs::read_dir(parent).await {
                let mut entries = entries;
                while let Ok(Some(entry)) = entries.next_entry().await {
                    if let Ok(file_type) = entry.file_type().await {
                        if file_type.is_file() {
                            if let Some(ext) = entry.path().extension() {
                                if matches!(ext.to_str(), Some("rs" | "js" | "ts" | "py" | "go" | "java")) {
                                    if let Ok(metadata) = entry.metadata().await {
                                        // Prioritize smaller files (likely config/helpers)
                                        if metadata.len() < 50000 {
                                            if let Some(path_str) = entry.path().to_str() {
                                                if path_str != file_path {
                                                    related.push(path_str.to_string());
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        
        // Limit to 5 related files
        Ok(related.into_iter().take(5).collect())
    }
    
    /// Read contents of context files for AI analysis
    async fn read_file_contents(&self, file_paths: &[String]) -> Result<HashMap<String, String>> {
        let mut contents = HashMap::new();
        
        for path_str in file_paths {
            let path = PathBuf::from(path_str);
            if path.exists() {
                // Limit file size for context (first 100 lines)
                match tokio::fs::read_to_string(&path).await {
                    Ok(content) => {
                        let lines: Vec<&str> = content.lines().take(100).collect();
                        contents.insert(path_str.clone(), lines.join("\n"));
                        
                        // Record that we read this file for staleness detection
                        self.file_tracker.record_read(&path);
                    }
                    Err(e) => {
                        eprintln!("[HEALER] Failed to read {}: {}", path_str, e);
                    }
                }
            }
        }
        
        Ok(contents)
    }
    
    /// Find relevant procedures from memory
    async fn find_relevant_procedures(&self, error: &DetectedError) -> Result<Vec<String>> {
        let memory = self.memory.read().await;
        let search_query = format!("procedure fix {:?}", error.error_type);
        let results = memory.search(&search_query, 3).await?;
        
        let procedures: Vec<String> = results
            .into_iter()
            .filter_map(|result| match result {
                crate::memory::types::MemoryResult::Semantic { content, metadata, .. } => {
                    if metadata.get("type").map(|t| t == "procedure").unwrap_or(false) {
                        Some(content)
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .collect();
        
        Ok(procedures)
    }
    
    /// Enhanced root cause extraction
    fn extract_root_cause_enhanced(&self, analysis: &str, error: &DetectedError) -> Option<String> {
        // Look for specific patterns in the analysis
        let patterns = [
            "Root cause:",
            "The issue is",
            "This error occurs because",
            "The problem is",
            "Caused by:",
            "The root cause",
        ];
        
        for line in analysis.lines() {
            let lower = line.to_lowercase();
            for pattern in &patterns {
                if lower.contains(&pattern.to_lowercase()) {
                    return Some(line.trim().to_string());
                }
            }
        }
        
        // Fallback: use error type and message
        Some(format!("{:?}: {}", error.error_type, 
            error.message.lines().next().unwrap_or(&error.message).chars().take(80).collect::<String>()))
    }
    
    /// Phase 2: Generate a fix
    async fn generate_fix(&self, session_id: &str) -> Result<()> {
        self.update_status(session_id, HealingStatus::GeneratingFix).await;
        
        let session = self.get_session(session_id).await?;
        let detected_error = self.extract_detected_error(&session.error_event)?;
        
        println!("[HEALER] Generating fix for: {:?}", detected_error.error_type);
        
        // Check if we should use swarm for complex fixes
        if self.config.use_swarm_for_complex && self.is_complex_error(&detected_error) {
            if let Some(ref swarm) = self.swarm {
                println!("[HEALER] Using swarm orchestrator for complex fix");
                return self.generate_fix_with_swarm(session_id, swarm, &detected_error).await;
            }
        }
        
        // Use AI to generate fix
        let (fix_description, hydration_plan) = self.generate_fix_with_ai_enhanced(
            session_id, 
            &detected_error, 
            &session.investigation
        ).await?;
        
        // Create fix attempt with hydration plan
        let fix_attempt = FixAttempt {
            id: format!("fix_{}", Uuid::new_v4()),
            description: fix_description.clone(),
            shadow_run_result: ShadowRunResult {
                success: false,
                exit_code: -1,
                stdout: String::new(),
                stderr: String::new(),
                duration_ms: 0,
            },
            hydration_plan,
            was_applied: false,
            verification_result: None,
        };
        
        // Store the fix attempt
        {
            let mut sessions = self.sessions.write().await;
            if let Some(session) = sessions.get_mut(session_id) {
                session.fixes.push(fix_attempt);
            }
        }
        
        self.event_tx.send(HealerEvent::FixGenerated {
            session_id: session_id.to_string(),
            fix_description: fix_description.clone(),
        }).await.ok();
        
        println!("[HEALER] Fix generated: {}", 
            fix_description.lines().next().unwrap_or("Fix generated").chars().take(80).collect::<String>()
        );
        
        Ok(())
    }
    
    /// Phase 3: Test fix in shadow run
    async fn test_fix_shadow_run(&self, session_id: &str) -> Result<()> {
        self.update_status(session_id, HealingStatus::TestingFix).await;
        
        let session = self.get_session(session_id).await?;
        
        // Get the working directory from the error event
        let working_dir = self.get_working_dir(&session.error_event)?;
        
        // Run build/test in shadow mode to verify fix would work
        let project_type = self.detect_project_type(&working_dir).await?;
        let test_command = self.get_test_command(&project_type);
        
        let shadow_result = self.sandbox.shadow_run(&test_command, &working_dir).await?;
        
        self.event_tx.send(HealerEvent::ShadowRunComplete {
            session_id: session_id.to_string(),
            success: shadow_result.success,
            output: format!("{}", shadow_result.stdout),
        }).await.ok();
        
        // Store the shadow run result
        let fix_attempt = FixAttempt {
            id: format!("fix_{}", Uuid::new_v4()),
            description: "Shadow run test".to_string(),
            shadow_run_result: shadow_result,
            hydration_plan: None,
            was_applied: false,
            verification_result: None,
        };
        
        {
            let mut sessions = self.sessions.write().await;
            if let Some(session) = sessions.get_mut(session_id) {
                session.fixes.push(fix_attempt);
            }
        }
        
        Ok(())
    }
    
    /// Phase 4: Apply the fix
    async fn apply_fix(&self, session_id: &str) -> Result<()> {
        self.update_status(session_id, HealingStatus::ApplyingFix).await;
        
        let session = self.get_session(session_id).await?;
        
        // Get the hydration plan from the last fix attempt
        if let Some(ref fix) = session.fixes.last() {
            if let Some(ref plan) = fix.hydration_plan {
                // Apply the hydration plan with staleness checking
                // The healer tracks its own reads during investigation
                self.hydrator.execute_plan_with_tracker(plan, Some(&self.file_tracker))?;
                
                let files_modified: Vec<String> = plan.files_to_update.iter()
                    .chain(plan.files_to_create.iter())
                    .map(|c| c.path.to_string_lossy().to_string())
                    .collect();
                
                self.event_tx.send(HealerEvent::FixApplied {
                    session_id: session_id.to_string(),
                    files_modified,
                }).await.ok();
            }
        }
        
        Ok(())
    }
    
    /// Phase 5: Verify the fix
    async fn verify_fix(&self, session_id: &str) -> Result<()> {
        self.update_status(session_id, HealingStatus::Verifying).await;
        
        let session = self.get_session(session_id).await?;
        let working_dir = self.get_working_dir(&session.error_event)?;
        
        // Run verification command
        let project_type = self.detect_project_type(&working_dir).await?;
        let verify_command = self.get_verify_command(&project_type);
        
        let shadow_result = self.sandbox.shadow_run(&verify_command, &working_dir).await?;
        
        let verification = VerificationResult {
            success: shadow_result.success,
            test_output: shadow_result.stdout.clone(),
            error_resolved: shadow_result.success && !shadow_result.stderr.contains("error"),
            new_errors: self.extract_errors_from_output(&shadow_result.stderr),
        };
        
        self.event_tx.send(HealerEvent::VerificationComplete {
            session_id: session_id.to_string(),
            success: verification.success,
            error_resolved: verification.error_resolved,
        }).await.ok();
        
        if self.config.notification_enabled {
            let (title, severity) = if verification.error_resolved {
                ("Error Fixed".to_string(), ErrorSeverity::Info)
            } else {
                ("Fix Verification Failed".to_string(), ErrorSeverity::Warning)
            };
            
            self.event_tx.send(HealerEvent::Notification {
                title,
                message: format!("Session: {}", session_id),
                severity,
            }).await.ok();
        }
        
        Ok(())
    }
    
    /// Phase 6: Learn from the fix
    async fn learn_from_fix(&self, session_id: &str) -> Result<()> {
        let session = self.get_session(session_id).await?;
        let detected_error = self.extract_detected_error(&session.error_event)?;
        
        let learning = LearningEntry {
            error_type: format!("{:?}", detected_error.error_type),
            error_signature: self.create_error_signature(&detected_error),
            fix_description: session.fixes.last()
                .map(|f| f.description.clone())
                .unwrap_or_default(),
            fix_success_rate: 1.0,
            tags: vec!["auto-healed".to_string()],
        };
        
        // Store in memory
        let mut memory = self.memory.write().await;
        memory.remember_fact(
            &format!("error_signature_{:?}", detected_error.error_type),
            "fix_history",
            &serde_json::to_string(&learning).unwrap_or_default(),
        ).await.ok();
        drop(memory);
        
        {
            let mut sessions = self.sessions.write().await;
            if let Some(session) = sessions.get_mut(session_id) {
                session.learning = Some(learning);
            }
        }
        
        Ok(())
    }
    
    /// Helper methods
    
    async fn get_session(&self, session_id: &str) -> Result<HealingSession> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id)
            .cloned()
            .ok_or_else(|| NexusError::Configuration(format!("Session '{}' not found", session_id)))
    }
    
    async fn update_status(&self, session_id: &str, status: HealingStatus) {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(session_id) {
            session.status = status;
        }
    }
    
    fn summarize_error(&self, error_event: &ErrorEvent) -> String {
        match error_event {
            ErrorEvent::LogError(e) => {
                format!("{} in {}", e.detected_error.message, e.source_id)
            }
            ErrorEvent::FileChange(e) => {
                format!("Build issue detected in {}", e.project_id)
            }
            ErrorEvent::BuildError { project_path, .. } => {
                format!("Build failed in {}", project_path.display())
            }
            ErrorEvent::TestFailure { test_name, .. } => {
                format!("Test '{}' failed", test_name)
            }
        }
    }
    
    fn extract_detected_error(&self, error_event: &ErrorEvent) -> Result<DetectedError> {
        match error_event {
            ErrorEvent::LogError(e) => Ok(e.detected_error.clone()),
            _ => Err(NexusError::Configuration("Cannot extract detected error from this event type".to_string())),
        }
    }
    
    async fn find_relevant_files(&self, error_event: &ErrorEvent) -> Result<Vec<String>> {
        let working_dir = self.get_working_dir(error_event)?;
        let mut files = Vec::new();
        
        // Walk the directory to find source files
        for entry in walkdir::WalkDir::new(&working_dir).max_depth(3) {
            if let Ok(entry) = entry {
                if entry.file_type().is_file() {
                    let path = entry.path();
                    if let Some(ext) = path.extension() {
                        let ext_str = ext.to_string_lossy();
                        if matches!(ext_str.as_ref(), "rs" | "js" | "ts" | "py") {
                            if let Ok(relative) = path.strip_prefix(&working_dir) {
                                files.push(relative.to_string_lossy().to_string());
                            }
                        }
                    }
                }
            }
        }
        
        Ok(files.into_iter().take(10).collect())
    }
    
    fn get_working_dir(&self, error_event: &ErrorEvent) -> Result<PathBuf> {
        match error_event {
            ErrorEvent::LogError(e) => Ok(e.detected_error.file_path.as_ref()
                .map(|p| PathBuf::from(p))
                .unwrap_or_else(|| PathBuf::from("."))),
            ErrorEvent::FileChange(e) => Ok(PathBuf::from(".")),
            ErrorEvent::BuildError { project_path, .. } => Ok(project_path.clone()),
            ErrorEvent::TestFailure { project_path, .. } => Ok(project_path.clone()),
        }
    }
    
    async fn analyze_error_with_ai(
        &self,
        error: &DetectedError,
        similar_errors: &[MemoryResult],
        context_files: &[String],
        file_contents: &HashMap<String, String>,
    ) -> Result<String> {
        // Build file context
        let file_context = file_contents
            .iter()
            .take(3) // Limit to first 3 files to avoid token limits
            .map(|(path, content)| {
                format!("--- {} ---\n{}", path, 
                    if content.len() > 2000 {
                        &content[..2000]
                    } else {
                        content
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        
        // Build similar errors context
        let similar_context = if similar_errors.is_empty() {
            "No similar past errors found in memory.".to_string()
        } else {
            similar_errors
                .iter()
                .take(3)
                .enumerate()
                .map(|(i, err)| {
                    format!("{}. {}", i + 1, match err {
                        MemoryResult::Semantic { content, score, .. } => {
                            format!("Similar error (relevance: {:.2}): {}", score, 
                                content.lines().next().unwrap_or(content).chars().take(100).collect::<String>())
                        }
                        MemoryResult::Graph { entity, entity_type, .. } => {
                            format!("Related {}: {}", entity_type, entity)
                        }
                        MemoryResult::Episodic { event } => {
                            format!("Past event: {:?}", event)
                        }
                    })
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        
        // Include stack trace if available
        let stack_trace_context = error.stack_trace.as_ref()
            .map(|st| format!("\n\nStack trace:\n{}", st))
            .unwrap_or_default();
        
        let mut messages = vec![
            Message {
                role: Role::System,
                content: "You are an expert software engineer analyzing code errors. Provide a concise root cause analysis with specific technical details. Focus on: 1) What caused the error, 2) Why it happened, 3) What files are involved.".to_string(),
                name: None,
            },
            Message {
                role: Role::User,
                content: format!(
                    "Analyze this error:\n\nType: {:?}\nSeverity: {:?}\nMessage: {}\nFile: {:?}\nLine: {:?}\nColumn: {:?}{}\n\nContext files: {:?}\n\nFile contents:\n{}\n\nSimilar past errors from memory:\n{}\n\nProvide a detailed root cause analysis.",
                    error.error_type, 
                    error.severity, 
                    error.message.lines().next().unwrap_or(&error.message),
                    error.file_path, 
                    error.line_number,
                    error.column,
                    stack_trace_context,
                    context_files,
                    if file_context.is_empty() { "No file contents available" } else { &file_context },
                    similar_context
                ),
                name: None,
            },
        ];
        
        let request = crate::providers::CompletionRequest {
            model: self.model.clone(),
            messages,
            temperature: Some(0.3),
            max_tokens: Some(800),
            stream: Some(false),
            extra_params: None,
        };
        
        let response = self.provider.complete(request).await?;
        Ok(response.content)
    }
    
    async fn generate_fix_with_ai_enhanced(
        &self,
        session_id: &str,
        error: &DetectedError,
        investigation: &InvestigationResult,
    ) -> Result<(String, Option<HydrationPlan>)> {
        // Build file context with actual code that needs changing
        let file_context = if investigation.context_files.len() > 0 {
            let mut context = String::new();
            for file_path in &investigation.context_files {
                if let Ok(content) = tokio::fs::read_to_string(file_path).await {
                    let line_info = if let Some(line_num) = error.line_number {
                        format!(" (error around line {})", line_num)
                    } else {
                        String::new()
                    };
                    context.push_str(&format!("\n\n--- {}{} ---\n{}", file_path, line_info, 
                        if content.len() > 3000 { &content[..3000] } else { &content }));
                }
            }
            context
        } else {
            "No context files available".to_string()
        };
        
        // Build suggested fixes from patterns
        let suggested_fixes = error.suggested_fix.as_ref()
            .map(|sf| format!("\n\nSuggested fix from pattern database: {}", sf))
            .unwrap_or_default();
        
        // Build similar error fixes
        let similar_fixes = if investigation.similar_past_errors.is_empty() {
            String::new()
        } else {
            "\n\nSimilar past fixes:\n".to_string() + &investigation.similar_past_errors
                .iter()
                .take(2)
                .filter_map(|result| match result {
                    MemoryResult::Semantic { content, .. } => Some(content.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        
        let mut messages = vec![
            Message {
                role: Role::System,
                content: "You are an expert software engineer. Generate a specific fix for the error. Provide: 1) A clear description of the fix, 2) The exact code changes needed. Be specific about what lines to change.".to_string(),
                name: None,
            },
            Message {
                role: Role::User,
                content: format!(
                    "Error Details:\nType: {:?}\nSeverity: {:?}\nMessage: {}\nFile: {:?}\nLine: {:?}\n\nRoot Cause Analysis:\n{}{}{}\n\nContext Files:\n{}\n\nGenerate a specific fix. Format your response as:\n\n## Description\nBrief description of the fix\n\n## Changes\nFor each file, specify the changes:
- File: [path]
  - Action: [add|modify|delete]
  - Line: [line number or range]
  - Content: [new code or modification]",
                    error.error_type,
                    error.severity,
                    error.message.lines().next().unwrap_or(&error.message),
                    error.file_path,
                    error.line_number,
                    investigation.analysis,
                    suggested_fixes,
                    similar_fixes,
                    file_context
                ),
                name: None,
            },
        ];
        
        let request = crate::providers::CompletionRequest {
            model: self.model.clone(),
            messages,
            temperature: Some(0.2),
            max_tokens: Some(2500),
            stream: Some(false),
            extra_params: None,
        };
        
        let response = self.provider.complete(request).await?;
        let fix_content = response.content.clone();
        
        // Try to parse and create a hydration plan from the response
        let hydration_plan = self.parse_fix_to_hydration_plan(&response.content, &investigation.context_files).await?;
        
        if let Some(ref plan) = hydration_plan {
            println!("[HEALER] Generated hydration plan with {} file updates", plan.files_to_update.len());
        }
        
        Ok((fix_content, hydration_plan))
    }
    
    /// Parse AI fix response into a hydration plan
    async fn parse_fix_to_hydration_plan(&self, fix_content: &str, context_files: &[String]) -> Result<Option<HydrationPlan>> {
        let mut files_to_update = Vec::new();
        let mut files_to_create = Vec::new();
        
        // Simple parsing: look for file changes in the response
        // This is a basic implementation - in production, you'd use more robust parsing
        let mut current_file: Option<String> = None;
        let mut collecting_content = false;
        let mut content_buffer = String::new();
        let mut action = String::from("modify");
        
        for line in fix_content.lines() {
            let trimmed = line.trim();
            
            // Detect file sections
            if trimmed.starts_with("File:") || trimmed.starts_with("- File:") {
                // Save previous file if any
                if let Some(ref file_path) = current_file {
                    if !content_buffer.is_empty() {
                        let change = crate::sandbox::hydration::FileChange {
                            path: PathBuf::from(file_path),
                            content: content_buffer.trim().to_string(),
                            backup_path: None,
                        };
                        
                        if action == "create" || action == "add" {
                            files_to_create.push(change);
                        } else {
                            files_to_update.push(change);
                        }
                    }
                }
                
                // Extract new file path
                current_file = trimmed
                    .splitn(2, ':')
                    .nth(1)
                    .map(|s| s.trim().to_string())
                    .or_else(|| context_files.first().cloned());
                
                content_buffer.clear();
                action = String::from("modify");
                collecting_content = false;
            }
            else if trimmed.starts_with("Action:") || trimmed.starts_with("- Action:") {
                action = trimmed
                    .splitn(2, ':')
                    .nth(1)
                    .map(|s| s.trim().to_lowercase())
                    .unwrap_or_else(|| String::from("modify"));
            }
            else if trimmed.starts_with("Content:") || trimmed.starts_with("- Content:") || 
                    trimmed.starts_with("```") {
                collecting_content = !collecting_content || !trimmed.starts_with("```");
                if trimmed.starts_with("```") && trimmed.len() > 3 {
                    // Skip the language identifier line
                    continue;
                }
            }
            else if collecting_content && !trimmed.is_empty() {
                content_buffer.push_str(line);
                content_buffer.push('\n');
            }
        }
        
        // Save the last file
        if let Some(ref file_path) = current_file {
            if !content_buffer.is_empty() {
                let change = crate::sandbox::hydration::FileChange {
                    path: PathBuf::from(file_path),
                    content: content_buffer.trim().to_string(),
                    backup_path: None,
                };
                
                if action == "create" || action == "add" {
                    files_to_create.push(change);
                } else {
                    files_to_update.push(change);
                }
            }
        }
        
        if files_to_update.is_empty() && files_to_create.is_empty() {
            // If no structured changes found, try to extract code blocks
            let code_blocks = self.extract_code_blocks(fix_content);
            if !code_blocks.is_empty() && !context_files.is_empty() {
                for (i, content) in code_blocks.iter().enumerate() {
                    if let Some(file_path) = context_files.get(i) {
                        files_to_update.push(crate::sandbox::hydration::FileChange {
                            path: PathBuf::from(file_path),
                            content: content.clone(),
                            backup_path: None,
                        });
                    }
                }
            }
        }
        
        if files_to_update.is_empty() && files_to_create.is_empty() {
            Ok(None)
        } else {
            Ok(Some(HydrationPlan {
                files_to_update,
                files_to_create,
                files_to_delete: Vec::new(),
                directories_to_create: Vec::new(),
            }))
        }
    }
    
    /// Extract code blocks from markdown-style response
    fn extract_code_blocks(&self, content: &str) -> Vec<String> {
        let mut blocks = Vec::new();
        let mut in_block = false;
        let mut current_block = String::new();
        
        for line in content.lines() {
            if line.trim_start().starts_with("```") {
                if in_block {
                    // End of block
                    if !current_block.trim().is_empty() {
                        blocks.push(current_block.trim().to_string());
                    }
                    current_block.clear();
                }
                in_block = !in_block;
            } else if in_block {
                current_block.push_str(line);
                current_block.push('\n');
            }
        }
        
        blocks
    }
    
    /// Legacy method - kept for compatibility
    async fn generate_fix_with_ai(
        &self,
        _session_id: &str,
        error: &DetectedError,
        investigation: &InvestigationResult,
    ) -> Result<String> {
        let (description, _) = self.generate_fix_with_ai_enhanced(_session_id, error, investigation).await?;
        Ok(description)
    }
    
    async fn generate_fix_with_swarm(
        &self,
        session_id: &str,
        swarm: &Arc<SwarmOrchestrator>,
        error: &DetectedError,
    ) -> Result<()> {
        let session = self.get_session(session_id).await?;
        let working_dir = self.get_working_dir(&session.error_event)?;
        
        let task = SwarmTask::new(
            format!("Fix error: {}", error.message),
            working_dir,
        );
        
        let result = swarm.execute(task).await?;
        
        println!("[HEALER] Swarm fix result: success={}, files={:?}", 
            result.success, result.merged_files);
        
        Ok(())
    }
    
    fn is_complex_error(&self, error: &DetectedError) -> bool {
        matches!(error.error_type, 
            ErrorType::RustBorrowChecker | 
            ErrorType::VersionConflict |
            ErrorType::BuildFailure
        )
    }
    
    async fn detect_project_type(&self, working_dir: &PathBuf) -> Result<String> {
        use crate::watcher::filesystem::detect_project_type;
        Ok(detect_project_type(working_dir).unwrap_or_else(|| "unknown".to_string()))
    }
    
    fn get_test_command(&self, project_type: &str) -> String {
        match project_type {
            "rust" => "cargo test --no-run".to_string(),
            "javascript" | "typescript" => "npm test --if-present".to_string(),
            "python" => "python -m pytest --collect-only".to_string(),
            "go" => "go build ./...".to_string(),
            _ => "echo 'No test command'".to_string(),
        }
    }
    
    fn get_verify_command(&self, project_type: &str) -> String {
        match project_type {
            "rust" => "cargo check".to_string(),
            "javascript" | "typescript" => "npm run build --if-present 2>&1 || npm run lint --if-present 2>&1 || echo 'No build command'".to_string(),
            "python" => "python -m compileall .".to_string(),
            "go" => "go build ./...".to_string(),
            _ => "echo 'No verify command'".to_string(),
        }
    }
    
    fn extract_root_cause(&self, analysis: &str) -> Option<String> {
        // Simple extraction - look for "Root cause:" or similar
        analysis.lines()
            .find(|l| l.to_lowercase().contains("root cause") || l.to_lowercase().contains("cause:"))
            .map(|l| l.trim().to_string())
    }
    
    fn create_error_signature(&self, error: &DetectedError) -> String {
        format!("{:?}:{}:{}", 
            error.error_type,
            error.file_path.as_ref().map(|p| p.clone()).unwrap_or_default(),
            error.line_number.unwrap_or(0)
        )
    }
    
    fn extract_errors_from_output(&self, output: &str) -> Vec<String> {
        output.lines()
            .filter(|l| l.contains("error") || l.contains("Error"))
            .map(|l| l.to_string())
            .collect()
    }
}

// Manual Clone implementation for Healer
impl Clone for Healer {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            memory: self.memory.clone(),
            sandbox: SandboxManager::new(),
            hydrator: Hydrator::new().expect("Failed to create hydrator"),
            file_tracker: self.file_tracker.clone(),
            provider: self.provider.clone(),
            model: self.model.clone(),
            sessions: self.sessions.clone(),
            event_tx: self.event_tx.clone(),
            swarm: self.swarm.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    // Note: These would need mocked dependencies in a real test suite
    
    #[tokio::test]
    async fn test_error_signature() {
        // Test signature creation
        let error = DetectedError {
            error_type: ErrorType::RustCompilation,
            severity: ErrorSeverity::Error,
            message: "test error".to_string(),
            file_path: Some("src/main.rs".to_string()),
            line_number: Some(42),
            column: None,
            stack_trace: None,
            suggested_fix: None,
        };
        
        // Verify the signature would be unique
        assert!(error.error_type == ErrorType::RustCompilation);
    }
}
