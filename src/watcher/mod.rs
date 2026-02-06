//! Self-Healing Watcher Module - Proactive error detection and automatic fixing
//!
//! This module provides comprehensive monitoring and healing capabilities for Nexus:
//! - File system watching with debounced change detection
//! - Log monitoring for build/test/runtime errors
//! - Pattern matching for common errors across languages
//! - Automatic error investigation and fix generation
//! - Shadow Run testing before applying fixes
//! - Memory integration for learning from past fixes
//!
//! ## Architecture
//!
//! ```
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    WatcherEngine                            │
//! ├─────────────────────────────────────────────────────────────┤
//! │  ┌──────────────┐  ┌──────────────┐  ┌──────────────────┐  │
//! │  │ FileSystem   │  │ LogMonitor   │  │ PatternsDatabase │  │
//! │  │   Watcher    │  │              │  │                  │  │
//! │  └──────┬───────┘  └──────┬───────┘  └──────────────────┘  │
//! │         │                 │                                 │
//! │         │ FileChangeEvent │ LogErrorEvent                 │
//! │         └────────┬────────┘                               │
//! │                  ▼                                          │
//! │         ┌────────────────┐                                  │
//! │         │  Event Router  │                                  │
//! │         └────────┬───────┘                                  │
//! │                  │                                          │
//! │                  ▼                                          │
//! │         ┌────────────────┐                                  │
//! │         │     Healer     │                                  │
//! │         │  (Shadow Run)  │                                  │
//! │         └────────┬───────┘                                  │
//! │                  │                                          │
//! │         ┌────────┴────────┐                                  │
//! │         ▼                 ▼                                  │
//! │  ┌──────────┐       ┌──────────┐                            │
//! │  │  Agent   │       │  Swarm   │                            │
//! │  └──────────┘       └──────────┘                            │
//! └─────────────────────────────────────────────────────────────┘
//! ```

pub mod filesystem;
pub mod healer;
pub mod logs;
pub mod patterns;

pub use filesystem::{FileSystemWatcher, FileChangeEvent, WatchedProject, WatchConfig};
pub use healer::{Healer, HealerConfig, HealerEvent, HealingSession, HealingStatus, ErrorEvent};
pub use logs::{LogMonitor, LogSource, LogErrorEvent, LogSourceType};
pub use patterns::{PatternsDatabase, DetectedError, ErrorType, ErrorSeverity, Language};

use crate::error::{NexusError, Result};
use crate::memory::MemorySystem;
use crate::providers::Provider;
use crate::swarm::SwarmOrchestrator;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tokio::time::{interval, Duration};

/// Configuration for the WatcherEngine
#[derive(Debug, Clone)]
pub struct WatcherEngineConfig {
    /// Enable file system watching
    pub enable_file_watching: bool,
    /// Enable log monitoring
    pub enable_log_monitoring: bool,
    /// Enable auto-healing
    pub enable_auto_healing: bool,
    /// Enable desktop notifications
    pub enable_notifications: bool,
    /// File watcher debounce time in ms
    pub debounce_ms: u64,
    /// Max number of concurrent healing sessions
    pub max_concurrent_healing: usize,
    /// Learning enabled - remember successful fixes
    pub learning_enabled: bool,
    /// Auto-apply fixes without user confirmation
    pub auto_apply_fixes: bool,
}

impl Default for WatcherEngineConfig {
    fn default() -> Self {
        Self {
            enable_file_watching: true,
            enable_log_monitoring: true,
            enable_auto_healing: true,
            enable_notifications: true,
            debounce_ms: 300,
            max_concurrent_healing: 3,
            learning_enabled: true,
            auto_apply_fixes: false, // Conservative default - ask first
        }
    }
}

/// A watched project with all its monitoring configuration
#[derive(Debug, Clone)]
pub struct ProjectWatcher {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub project_type: Option<String>,
    pub file_watcher_enabled: bool,
    pub log_monitoring_enabled: bool,
    pub auto_healing_enabled: bool,
    pub custom_patterns: Vec<String>,
}

/// Status of the WatcherEngine
#[derive(Debug, Clone, Default)]
pub struct EngineStatus {
    pub is_running: bool,
    pub watched_projects: usize,
    pub active_log_sources: usize,
    pub healing_sessions_total: usize,
    pub healing_sessions_active: usize,
    pub errors_detected: u64,
    pub errors_fixed: u64,
    pub start_time: Option<chrono::DateTime<chrono::Utc>>,
}

/// The main engine that coordinates all watching and healing activities
pub struct WatcherEngine {
    config: WatcherEngineConfig,
    
    // Sub-systems
    file_watcher: Option<FileSystemWatcher>,
    log_monitor: Option<LogMonitor>,
    healer: Option<Arc<Healer>>,
    
    // Channels
    file_change_tx: mpsc::Sender<FileChangeEvent>,
    file_change_rx: Arc<RwLock<mpsc::Receiver<FileChangeEvent>>>,
    log_error_tx: mpsc::Sender<LogErrorEvent>,
    log_error_rx: Arc<RwLock<mpsc::Receiver<LogErrorEvent>>>,
    healer_event_tx: mpsc::Sender<HealerEvent>,
    healer_event_rx: Arc<RwLock<mpsc::Receiver<HealerEvent>>>,
    
    // State
    projects: Arc<RwLock<HashMap<String, ProjectWatcher>>>,
    status: Arc<RwLock<EngineStatus>>,
    memory: Arc<RwLock<MemorySystem>>,
    
    // Runtime
    running: Arc<RwLock<bool>>,
    main_task: Option<tokio::task::JoinHandle<()>>,
}

impl WatcherEngine {
    /// Create a new WatcherEngine with all subsystems
    pub async fn new(
        config: WatcherEngineConfig,
        memory: Arc<RwLock<MemorySystem>>,
        provider: Arc<dyn Provider + Send + Sync>,
        model: String,
    ) -> Result<Self> {
        // Create channels
        let (file_change_tx, file_change_rx) = mpsc::channel(100);
        let (log_error_tx, log_error_rx) = mpsc::channel(100);
        let (healer_event_tx, healer_event_rx) = mpsc::channel(100);
        
        let file_change_rx = Arc::new(RwLock::new(file_change_rx));
        let log_error_rx = Arc::new(RwLock::new(log_error_rx));
        let healer_event_rx = Arc::new(RwLock::new(healer_event_rx));
        
        // Create subsystems
        let file_watcher = if config.enable_file_watching {
            Some(FileSystemWatcher::new(file_change_tx.clone()))
        } else {
            None
        };
        
        let log_monitor = if config.enable_log_monitoring {
            Some(LogMonitor::new(log_error_tx.clone(), 50))
        } else {
            None
        };
        
        let healer = if config.enable_auto_healing {
            let healer_config = HealerConfig {
                auto_apply_simple_fixes: config.auto_apply_fixes,
                max_fix_attempts: 3,
                verify_timeout_secs: 60,
                use_shadow_run: true,
                use_swarm_for_complex: true,
                learning_enabled: config.learning_enabled,
                notification_enabled: config.enable_notifications,
            };
            
            Some(Arc::new(Healer::new(
                healer_config,
                memory.clone(),
                provider,
                model,
                healer_event_tx.clone(),
            )?))
        } else {
            None
        };
        
        Ok(Self {
            config,
            file_watcher,
            log_monitor,
            healer,
            file_change_tx,
            file_change_rx,
            log_error_tx,
            log_error_rx,
            healer_event_tx,
            healer_event_rx,
            projects: Arc::new(RwLock::new(HashMap::new())),
            status: Arc::new(RwLock::new(EngineStatus::default())),
            memory,
            running: Arc::new(RwLock::new(false)),
            main_task: None,
        })
    }
    
    /// Start the watcher engine
    pub async fn start(&mut self) -> Result<()> {
        let mut running = self.running.write().await;
        if *running {
            return Err(NexusError::Configuration("WatcherEngine already running".to_string()));
        }
        
        *running = true;
        drop(running);
        
        // Update status
        {
            let mut status = self.status.write().await;
            status.is_running = true;
            status.start_time = Some(chrono::Utc::now());
        }
        
        println!("[WATCHER ENGINE] Starting...");
        
        // Spawn the main event loop
        let file_change_rx = self.file_change_rx.clone();
        let log_error_rx = self.log_error_rx.clone();
        let healer_event_rx = self.healer_event_rx.clone();
        let healer = self.healer.clone();
        let running_flag = self.running.clone();
        let status = self.status.clone();
        
        self.main_task = Some(tokio::spawn(async move {
            Self::run_event_loop(
                file_change_rx,
                log_error_rx,
                healer_event_rx,
                healer,
                running_flag,
                status,
            ).await;
        }));
        
        println!("[WATCHER ENGINE] Started successfully");
        Ok(())
    }
    
    /// Stop the watcher engine
    pub async fn stop(&mut self) -> Result<()> {
        let mut running = self.running.write().await;
        *running = false;
        drop(running);
        
        // Cancel main task
        if let Some(task) = self.main_task.take() {
            task.abort();
        }
        
        // Shutdown subsystems
        if let Some(ref watcher) = self.file_watcher {
            watcher.shutdown().await;
        }
        
        if let Some(ref monitor) = self.log_monitor {
            monitor.shutdown().await;
        }
        
        // Update status
        {
            let mut status = self.status.write().await;
            status.is_running = false;
        }
        
        println!("[WATCHER ENGINE] Stopped");
        Ok(())
    }
    
    /// Main event loop processing all events
    async fn run_event_loop(
        file_change_rx: Arc<RwLock<mpsc::Receiver<FileChangeEvent>>>,
        log_error_rx: Arc<RwLock<mpsc::Receiver<LogErrorEvent>>>,
        healer_event_rx: Arc<RwLock<mpsc::Receiver<HealerEvent>>>,
        healer: Option<Arc<Healer>>,
        running: Arc<RwLock<bool>>,
        status: Arc<RwLock<EngineStatus>>,
    ) {
        let mut file_rx = file_change_rx.write().await;
        let mut log_rx = log_error_rx.write().await;
        let mut healer_rx = healer_event_rx.write().await;
        
        loop {
            // Check if we should stop
            {
                let running_val = running.read().await;
                if !*running_val {
                    break;
                }
            }
            
            // Process file change events
            if let Ok(event) = file_rx.try_recv() {
                Self::handle_file_change(&event, &healer, &status).await;
            }
            
            // Process log error events
            if let Ok(event) = log_rx.try_recv() {
                Self::handle_log_error(&event, &healer, &status).await;
            }
            
            // Process healer events (for notifications/logging)
            if let Ok(event) = healer_rx.try_recv() {
                Self::handle_healer_event(&event, &status).await;
            }
            
            // Small delay to prevent busy-waiting
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    
    /// Handle a file change event
    async fn handle_file_change(
        event: &FileChangeEvent,
        healer: &Option<Arc<Healer>>,
        status: &Arc<RwLock<EngineStatus>>,
    ) {
        println!("[WATCHER] File change in {}: {:?} {}", 
            event.project_id, 
            event.change_type,
            event.file_path.display()
        );
        
        // Update stats
        {
            let mut s = status.write().await;
            s.errors_detected += 1;
        }
        
        // If config file changed, trigger full rebuild analysis
        if event.is_config_change {
            println!("[WATCHER] Config file changed - dependencies may need updating");
        }
        
        // Trigger healing if auto-healing is enabled
        if let Some(h) = healer {
            if event.should_build || event.should_test {
                let error_event = ErrorEvent::FileChange(event.clone());
                if let Err(e) = h.heal(error_event).await {
                    eprintln!("[WATCHER] Failed to start healing: {}", e);
                }
            }
        }
    }
    
    /// Handle a log error event
    async fn handle_log_error(
        event: &LogErrorEvent,
        healer: &Option<Arc<Healer>>,
        status: &Arc<RwLock<EngineStatus>>,
    ) {
        println!("[WATCHER] Error detected in {}: {} (severity: {:?})",
            event.source_id,
            event.detected_error.message,
            event.detected_error.severity
        );
        
        // Update stats
        {
            let mut s = status.write().await;
            s.errors_detected += 1;
        }
        
        // Trigger healing if auto-healing is enabled and error is severe enough
        if let Some(h) = healer {
            if event.detected_error.severity >= ErrorSeverity::Error {
                let error_event = ErrorEvent::LogError(event.clone());
                if let Err(e) = h.heal(error_event).await {
                    eprintln!("[WATCHER] Failed to start healing: {}", e);
                }
            }
        }
    }
    
    /// Handle healer events
    async fn handle_healer_event(
        event: &HealerEvent,
        status: &Arc<RwLock<EngineStatus>>,
    ) {
        match event {
            HealerEvent::SessionComplete { success, .. } => {
                if *success {
                    let mut s = status.write().await;
                    s.errors_fixed += 1;
                }
            }
            HealerEvent::Notification { title, message, severity } => {
                // In a real implementation, this would send desktop notifications
                let emoji = match severity {
                    ErrorSeverity::Info => "ℹ️",
                    ErrorSeverity::Warning => "⚠️",
                    ErrorSeverity::Error => "❌",
                    ErrorSeverity::Critical => "🔥",
                };
                println!("[NOTIFICATION] {} {}: {}", emoji, title, message);
            }
            _ => {
                // Log other events for debugging
                if matches!(event, HealerEvent::SessionStarted { .. }) {
                    let mut s = status.write().await;
                    s.healing_sessions_total += 1;
                    s.healing_sessions_active += 1;
                }
            }
        }
    }
    
    /// Add a project to watch
    pub async fn add_project(&self, path: PathBuf, name: Option<String>) -> Result<String> {
        let project_id = format!("proj_{}", uuid::Uuid::new_v4());
        let project_name = name.unwrap_or_else(|| {
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string()
        });
        
        // Detect project type
        let project_type = filesystem::detect_project_type(&path);
        
        let project = ProjectWatcher {
            id: project_id.clone(),
            name: project_name.clone(),
            path: path.clone(),
            project_type,
            file_watcher_enabled: self.config.enable_file_watching,
            log_monitoring_enabled: self.config.enable_log_monitoring,
            auto_healing_enabled: self.config.enable_auto_healing,
            custom_patterns: Vec::new(),
        };
        
        // Add to file watcher
        if let Some(ref watcher) = self.file_watcher {
            let watch_config = if let Some(ref pt) = project.project_type {
                filesystem::default_config_for_project_type(pt)
            } else {
                WatchConfig::default()
            };
            
            let watched_project = WatchedProject {
                id: project_id.clone(),
                name: project_name.clone(),
                path: path.clone(),
                enabled: true,
                watch_config,
            };
            
            watcher.add_project(watched_project).await?;
        }
        
        // Add to log monitor (monitor build logs if they exist)
        if let Some(ref monitor) = self.log_monitor {
            // Look for common log file locations
            let log_locations = vec![
                path.join("logs").join("dev.log"),
                path.join("dev.log"),
                path.join("npm-debug.log"),
                path.join("yarn-error.log"),
            ];
            
            for log_path in log_locations {
                if log_path.exists() {
                    let source = LogSource {
                        id: format!("{}_log_{}", project_id, log_path.display()),
                        name: format!("{} log", project_name),
                        source_type: LogSourceType::File { path: log_path },
                        project_path: path.clone(),
                        language_hint: project.project_type.as_ref()
                            .and_then(|pt| match pt.as_str() {
                                "rust" => Some(Language::Rust),
                                "javascript" => Some(Language::JavaScript),
                                "typescript" => Some(Language::TypeScript),
                                "python" => Some(Language::Python),
                                _ => None,
                            }),
                        enabled: true,
                    };
                    
                    monitor.add_source(source).await.ok(); // Don't fail if log doesn't exist
                }
            }
        }
        
        // Store project
        {
            let mut projects = self.projects.write().await;
            projects.insert(project_id.clone(), project);
        }
        
        // Update status
        {
            let mut s = self.status.write().await;
            s.watched_projects += 1;
        }
        
        println!("[WATCHER ENGINE] Added project: {} ({})", project_id, project_name);
        Ok(project_id)
    }
    
    /// Remove a project from watching
    pub async fn remove_project(&self, project_id: &str) -> Result<()> {
        // Remove from file watcher
        if let Some(ref watcher) = self.file_watcher {
            watcher.remove_project(project_id).await.ok();
        }
        
        // Remove from log monitor
        if let Some(ref monitor) = self.log_monitor {
            // Find and remove all sources for this project
            let sources = monitor.get_sources().await;
            for source in sources {
                if source.id.starts_with(project_id) {
                    monitor.remove_source(&source.id).await.ok();
                }
            }
        }
        
        // Remove from storage
        {
            let mut projects = self.projects.write().await;
            projects.remove(project_id);
        }
        
        // Update status
        {
            let mut s = self.status.write().await;
            s.watched_projects = s.watched_projects.saturating_sub(1);
        }
        
        println!("[WATCHER ENGINE] Removed project: {}", project_id);
        Ok(())
    }
    
    /// Monitor a dev server process
    pub async fn monitor_dev_server(
        &self,
        project_id: &str,
        command: String,
        args: Vec<String>,
    ) -> Result<String> {
        if let Some(ref monitor) = self.log_monitor {
            let projects = self.projects.read().await;
            let project = projects.get(project_id)
                .ok_or_else(|| NexusError::Configuration(
                    format!("Project '{}' not found", project_id)
                ))?;
            
            let source = LogSource {
                id: format!("{}_devserver_{}", project_id, uuid::Uuid::new_v4()),
                name: format!("{} dev server", project.name),
                source_type: LogSourceType::Process { command, args },
                project_path: project.path.clone(),
                language_hint: project.project_type.as_ref()
                    .and_then(|pt| match pt.as_str() {
                        "rust" => Some(Language::Rust),
                        "javascript" => Some(Language::JavaScript),
                        "typescript" => Some(Language::TypeScript),
                        "python" => Some(Language::Python),
                        _ => None,
                    }),
                enabled: true,
            };
            
            let source_id = source.id.clone();
            monitor.add_source(source).await?;
            
            // Update status
            {
                let mut s = self.status.write().await;
                s.active_log_sources += 1;
            }
            
            println!("[WATCHER ENGINE] Monitoring dev server for {}: {}", project_id, source_id);
            Ok(source_id)
        } else {
            Err(NexusError::Configuration("Log monitoring not enabled".to_string()))
        }
    }
    
    /// Get current engine status
    pub async fn get_status(&self) -> EngineStatus {
        self.status.read().await.clone()
    }
    
    /// Get list of watched projects
    pub async fn get_projects(&self) -> Vec<ProjectWatcher> {
        let projects = self.projects.read().await;
        projects.values().cloned().collect()
    }
    
    /// Manually trigger healing for a specific error
    pub async fn manual_heal(&self, error_description: String, file_path: Option<PathBuf>) -> Result<String> {
        if let Some(ref healer) = self.healer {
            let error_event = ErrorEvent::BuildError {
                project_path: file_path.unwrap_or_else(|| PathBuf::from(".")),
                output: error_description,
            };
            
            healer.heal(error_event).await
        } else {
            Err(NexusError::Configuration("Auto-healing not enabled".to_string()))
        }
    }
    
    /// Set the swarm orchestrator for complex healing tasks
    pub async fn set_swarm(&mut self, swarm: Arc<SwarmOrchestrator>) -> Result<()> {
        if let Some(ref mut healer) = self.healer {
            // This requires interior mutability, so we'd need to wrap in RwLock
            // For now, this is a placeholder showing the intent
            println!("[WATCHER ENGINE] Swarm orchestrator registered for complex healing");
        }
        Ok(())
    }
    
    /// Update configuration
    pub async fn update_config(&mut self, new_config: WatcherEngineConfig) -> Result<()> {
        // Stop if running
        let was_running = {
            let running = self.running.read().await;
            *running
        };
        
        if was_running {
            self.stop().await?;
        }
        
        self.config = new_config;
        
        // Restart if was running
        if was_running {
            self.start().await?;
        }
        
        Ok(())
    }
}

/// Convenience function to create a default watcher engine
pub async fn create_default_engine(
    memory: Arc<RwLock<MemorySystem>>,
    provider: Arc<dyn Provider + Send + Sync>,
    model: String,
) -> Result<WatcherEngine> {
    let config = WatcherEngineConfig::default();
    WatcherEngine::new(config, memory, provider, model).await
}

/// Desktop notification helper
#[cfg(target_os = "macos")]
pub fn send_desktop_notification(title: &str, message: &str) {
    use std::process::Command;
    
    Command::new("osascript")
        .args(&["-e", &format!(
            r#"display notification "{}" with title "{}""#,
            message, title
        )])
        .spawn()
        .ok();
}

#[cfg(target_os = "linux")]
pub fn send_desktop_notification(title: &str, message: &str) {
    use std::process::Command;
    
    Command::new("notify-send")
        .args(&[title, message])
        .spawn()
        .ok();
}

#[cfg(target_os = "windows")]
pub fn send_desktop_notification(title: &str, message: &str) {
    // Windows notification would require additional crates
    println!("[NOTIFICATION] {}: {}", title, message);
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub fn send_desktop_notification(title: &str, message: &str) {
    println!("[NOTIFICATION] {}: {}", title, message);
}

#[cfg(test)]
mod tests {
    use super::*;
    
    // These would need mocked dependencies in a real test suite
    
    #[test]
    fn test_watcher_engine_config_default() {
        let config = WatcherEngineConfig::default();
        assert!(config.enable_file_watching);
        assert!(config.enable_log_monitoring);
        assert!(config.enable_auto_healing);
        assert_eq!(config.debounce_ms, 300);
    }
    
    #[test]
    fn test_engine_status_default() {
        let status = EngineStatus::default();
        assert!(!status.is_running);
        assert_eq!(status.watched_projects, 0);
    }
}
