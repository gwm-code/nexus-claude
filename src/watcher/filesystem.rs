//! File System Watcher - Watch for file changes and trigger actions
//!
//! This module uses the notify crate to watch for file system changes
//! and trigger builds, tests, or other actions when relevant files change.

use crate::error::{NexusError, Result};
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use notify_debouncer_full::{new_debouncer, DebouncedEvent, Debouncer, FileIdMap};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex, RwLock};

/// A watched project with its configuration
#[derive(Debug, Clone)]
pub struct WatchedProject {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub enabled: bool,
    pub watch_config: WatchConfig,
}

/// Configuration for file watching
#[derive(Debug, Clone)]
pub struct WatchConfig {
    /// Patterns to watch (glob patterns)
    pub watch_patterns: Vec<String>,
    /// Patterns to ignore
    pub ignore_patterns: Vec<String>,
    /// File extensions that trigger build
    pub build_extensions: Vec<String>,
    /// File extensions that trigger test
    pub test_extensions: Vec<String>,
    /// Configuration files that trigger full rebuild
    pub config_files: Vec<String>,
    /// Debounce duration in milliseconds
    pub debounce_ms: u64,
    /// Auto-run build on change
    pub auto_build: bool,
    /// Auto-run tests on change
    pub auto_test: bool,
    /// Auto-run lint on change
    pub auto_lint: bool,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            watch_patterns: vec!["**/*".to_string()],
            ignore_patterns: vec![
                "**/node_modules/**".to_string(),
                "**/target/**".to_string(),
                "**/.git/**".to_string(),
                "**/dist/**".to_string(),
                "**/build/**".to_string(),
                "**/*.lock".to_string(),
            ],
            build_extensions: vec![
                "rs".to_string(),
                "js".to_string(),
                "ts".to_string(),
                "jsx".to_string(),
                "tsx".to_string(),
                "py".to_string(),
                "go".to_string(),
                "java".to_string(),
            ],
            test_extensions: vec![
                "rs".to_string(),
                "js".to_string(),
                "ts".to_string(),
                "py".to_string(),
            ],
            config_files: vec![
                "Cargo.toml".to_string(),
                "package.json".to_string(),
                "requirements.txt".to_string(),
                "go.mod".to_string(),
                "pom.xml".to_string(),
                "build.gradle".to_string(),
                "tsconfig.json".to_string(),
                "webpack.config.js".to_string(),
                "vite.config.js".to_string(),
            ],
            debounce_ms: 300,
            auto_build: true,
            auto_test: true,
            auto_lint: true,
        }
    }
}

/// A file change event
#[derive(Debug, Clone)]
pub struct FileChangeEvent {
    pub project_id: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub change_type: ChangeType,
    pub file_path: PathBuf,
    pub should_build: bool,
    pub should_test: bool,
    pub should_lint: bool,
    pub is_config_change: bool,
}

/// Type of file change
#[derive(Debug, Clone, PartialEq)]
pub enum ChangeType {
    Created,
    Modified,
    Deleted,
    Renamed(PathBuf), // Contains the old path
}

/// File system watcher statistics
#[derive(Debug, Clone, Default)]
pub struct WatcherStats {
    pub total_projects: usize,
    pub active_projects: usize,
    pub total_changes: u64,
    pub changes_by_type: HashMap<String, u64>,
    pub last_change_timestamp: Option<chrono::DateTime<chrono::Utc>>,
}

/// The file system watcher that monitors projects
pub struct FileSystemWatcher {
    projects: Arc<RwLock<HashMap<String, WatchedProject>>>,
    debouncers: Arc<Mutex<HashMap<String, Debouncer<RecommendedWatcher, FileIdMap>>>>,
    change_tx: mpsc::Sender<FileChangeEvent>,
}

impl FileSystemWatcher {
    pub fn new(change_tx: mpsc::Sender<FileChangeEvent>) -> Self {
        Self {
            projects: Arc::new(RwLock::new(HashMap::new())),
            debouncers: Arc::new(Mutex::new(HashMap::new())),
            change_tx,
        }
    }
    
    /// Add a project to watch
    pub async fn add_project(&self, project: WatchedProject) -> Result<()> {
        let mut projects = self.projects.write().await;
        
        if projects.contains_key(&project.id) {
            return Err(NexusError::Configuration(
                format!("Project '{}' already being watched", project.id)
            ));
        }
        
        let project_id = project.id.clone();
        let project_name = project.name.clone();
        let project_enabled = project.enabled;
        projects.insert(project_id.clone(), project.clone());
        drop(projects);
        
        // Start watching if enabled
        if project_enabled {
            self.start_watching(project).await?;
        }
        
        println!("[FS WATCHER] Added project: {} ({})", project_id, project_name);
        Ok(())
    }
    
    /// Remove a project from watching
    pub async fn remove_project(&self, project_id: &str) -> Result<()> {
        // Stop watching first
        self.stop_watching(project_id).await;
        
        let mut projects = self.projects.write().await;
        projects.remove(project_id);
        
        println!("[FS WATCHER] Removed project: {}", project_id);
        Ok(())
    }
    
    /// Start watching a specific project
    async fn start_watching(&self, project: WatchedProject) -> Result<()> {
        let project_id = project.id.clone();
        let project_path = project.path.clone();
        let watch_config = project.watch_config.clone();
        let change_tx = self.change_tx.clone();
        
        // Create debouncer
        let debounce_duration = Duration::from_millis(watch_config.debounce_ms);
        let change_tx_clone = change_tx.clone();
        let project_id_clone = project_id.clone();
        let watch_config_clone = watch_config.clone();
        let mut debouncer = new_debouncer(debounce_duration, None, move |result: std::result::Result<Vec<DebouncedEvent>, Vec<notify::Error>>| {
            if let Ok(events) = result {
                // Process debounced events
                Self::process_debounced_events(
                    events,
                    &project_id_clone,
                    &watch_config_clone,
                    &change_tx_clone,
                );
            }
        }).map_err(|e| NexusError::Configuration(format!("Failed to create debouncer: {}", e)))?;
        
        // Watch the project directory
        debouncer.watcher()
            .watch(&project_path, RecursiveMode::Recursive)
            .map_err(|e| NexusError::Configuration(format!("Failed to watch path: {}", e)))?;
        
        // Store debouncer
        let mut debouncers = self.debouncers.lock().await;
        debouncers.insert(project.id.clone(), debouncer);
        
        println!("[FS WATCHER] Now watching: {} at {}", project.name, project.path.display());
        Ok(())
    }
    
    /// Stop watching a project
    async fn stop_watching(&self, project_id: &str) {
        let mut debouncers = self.debouncers.lock().await;
        if let Some(_debouncer) = debouncers.remove(project_id) {
            // Debouncer will be dropped, which stops watching
            println!("[FS WATCHER] Stopped watching: {}", project_id);
        }
    }
    
    /// Process debounced file system events
    fn process_debounced_events(
        events: Vec<DebouncedEvent>,
        project_id: &str,
        watch_config: &WatchConfig,
        change_tx: &mpsc::Sender<FileChangeEvent>,
    ) {
        // Collect unique file paths and their change types
        let mut changes: HashMap<PathBuf, ChangeType> = HashMap::new();
        
        for event in events {
            // Process based on event kind
            match &event.event.kind {
                notify::EventKind::Create(_) => {
                    for path in &event.event.paths {
                        changes.insert(path.clone(), ChangeType::Created);
                    }
                }
                notify::EventKind::Modify(_) => {
                    for path in &event.event.paths {
                        changes.insert(path.clone(), ChangeType::Modified);
                    }
                }
                notify::EventKind::Remove(_) => {
                    for path in &event.event.paths {
                        changes.insert(path.clone(), ChangeType::Deleted);
                    }
                }
                _ => {}
            }
        }
        
        // Generate change events for each file
        for (path, change_type) in changes {
            let should_build = Self::should_trigger_build(&path, watch_config);
            let should_test = Self::should_trigger_test(&path, watch_config);
            let should_lint = watch_config.auto_lint && Self::is_source_file(&path);
            let is_config_change = Self::is_config_file(&path, watch_config);
            
            let event = FileChangeEvent {
                project_id: project_id.to_string(),
                timestamp: chrono::Utc::now(),
                change_type,
                file_path: path,
                should_build,
                should_test,
                should_lint,
                is_config_change,
            };
            
            // Use try_send since we're in a non-async context
            if let Err(e) = change_tx.try_send(event) {
                eprintln!("[FS WATCHER] Failed to send change event: {}", e);
            }
        }
    }
    
    /// Check if a file change should trigger a build
    fn should_trigger_build(path: &Path, config: &WatchConfig) -> bool {
        if let Some(ext) = path.extension() {
            let ext_str = ext.to_string_lossy().to_lowercase();
            return config.build_extensions.contains(&ext_str);
        }
        
        // Check if it's a config file
        if let Some(file_name) = path.file_name() {
            let name = file_name.to_string_lossy();
            if config.config_files.contains(&name.to_string()) {
                return true;
            }
        }
        
        false
    }
    
    /// Check if a file change should trigger tests
    fn should_trigger_test(path: &Path, config: &WatchConfig) -> bool {
        if let Some(ext) = path.extension() {
            let ext_str = ext.to_string_lossy().to_lowercase();
            return config.test_extensions.contains(&ext_str);
        }
        false
    }
    
    /// Check if a file is a config file
    fn is_config_file(path: &Path, config: &WatchConfig) -> bool {
        if let Some(file_name) = path.file_name() {
            let name = file_name.to_string_lossy();
            config.config_files.contains(&name.to_string())
        } else {
            false
        }
    }
    
    /// Check if a file is a source code file
    fn is_source_file(path: &Path) -> bool {
        if let Some(ext) = path.extension() {
            let ext_str = ext.to_string_lossy().to_lowercase();
            matches!(ext_str.as_str(), "rs" | "js" | "ts" | "jsx" | "tsx" | "py" | "go" | "java" | "c" | "cpp" | "h" | "hpp")
        } else {
            false
        }
    }
    
    /// Get all watched projects
    pub async fn get_projects(&self) -> Vec<WatchedProject> {
        let projects = self.projects.read().await;
        projects.values().cloned().collect()
    }
    
    /// Get statistics
    pub async fn get_stats(&self) -> WatcherStats {
        let projects = self.projects.read().await;
        let debouncers = self.debouncers.lock().await;
        
        WatcherStats {
            total_projects: projects.len(),
            active_projects: debouncers.len(),
            total_changes: 0, // Would track this in a real implementation
            changes_by_type: HashMap::new(),
            last_change_timestamp: None,
        }
    }
    
    /// Stop all watching
    pub async fn shutdown(&self) {
        let mut debouncers = self.debouncers.lock().await;
        debouncers.clear();
        println!("[FS WATCHER] All watching stopped");
    }
}

/// Detect project type from directory contents
pub fn detect_project_type(path: &Path) -> Option<String> {
    let indicators: Vec<(&str, &str)> = vec![
        ("Cargo.toml", "rust"),
        ("package.json", "javascript"),
        ("requirements.txt", "python"),
        ("pyproject.toml", "python"),
        ("setup.py", "python"),
        ("go.mod", "go"),
        ("pom.xml", "java-maven"),
        ("build.gradle", "java-gradle"),
        ("Gemfile", "ruby"),
        ("composer.json", "php"),
    ];
    
    for (file, project_type) in indicators {
        if path.join(file).exists() {
            return Some(project_type.to_string());
        }
    }
    
    None
}

/// Generate default watch config for a project type
pub fn default_config_for_project_type(project_type: &str) -> WatchConfig {
    let mut config = WatchConfig::default();
    
    match project_type {
        "rust" => {
            config.watch_patterns = vec!["src/**/*.rs".to_string(), "Cargo.toml".to_string(), "Cargo.lock".to_string()];
            config.build_extensions = vec!["rs".to_string()];
            config.config_files = vec!["Cargo.toml".to_string(), "Cargo.lock".to_string()];
            config.auto_build = true;
            config.auto_test = true;
        }
        "javascript" | "typescript" => {
            config.watch_patterns = vec!["src/**/*".to_string(), "package.json".to_string()];
            config.build_extensions = vec!["js".to_string(), "ts".to_string(), "jsx".to_string(), "tsx".to_string()];
            config.test_extensions = vec!["js".to_string(), "ts".to_string(), "test.js".to_string(), "spec.js".to_string()];
            config.config_files = vec!["package.json".to_string(), "tsconfig.json".to_string()];
            config.ignore_patterns.push("**/node_modules/**".to_string());
            config.auto_build = false; // JS projects often don't need explicit builds
            config.auto_test = true;
        }
        "python" => {
            config.watch_patterns = vec!["**/*.py".to_string(), "requirements.txt".to_string()];
            config.build_extensions = vec!["py".to_string()];
            config.test_extensions = vec!["py".to_string()];
            config.config_files = vec!["requirements.txt".to_string(), "pyproject.toml".to_string()];
            config.ignore_patterns.push("**/__pycache__/**".to_string());
            config.ignore_patterns.push("**/*.pyc".to_string());
            config.auto_build = false;
            config.auto_test = true;
        }
        "go" => {
            config.watch_patterns = vec!["**/*.go".to_string(), "go.mod".to_string()];
            config.build_extensions = vec!["go".to_string()];
            config.test_extensions = vec!["go".to_string()];
            config.config_files = vec!["go.mod".to_string(), "go.sum".to_string()];
            config.auto_build = true;
            config.auto_test = true;
        }
        _ => {}
    }
    
    config
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_watcher_creation() {
        let (tx, _rx) = mpsc::channel(100);
        let watcher = FileSystemWatcher::new(tx);
        
        let project = WatchedProject {
            id: "test".to_string(),
            name: "Test Project".to_string(),
            path: PathBuf::from("/tmp"),
            enabled: false,
            watch_config: WatchConfig::default(),
        };
        
        watcher.add_project(project).await.unwrap();
        
        let projects = watcher.get_projects().await;
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].id, "test");
    }
    
    #[test]
    fn test_project_type_detection() {
        // Note: This would need actual directories in a real test
        // For now, we just test the function exists
        assert!(detect_project_type(Path::new("/")).is_none());
    }
    
    #[test]
    fn test_default_configs() {
        let rust_config = default_config_for_project_type("rust");
        assert!(rust_config.build_extensions.contains(&"rs".to_string()));
        assert!(rust_config.config_files.contains(&"Cargo.toml".to_string()));
        
        let js_config = default_config_for_project_type("javascript");
        assert!(js_config.test_extensions.contains(&"js".to_string()));
    }
}
