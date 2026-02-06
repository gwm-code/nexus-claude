//! Log Monitor - Monitors dev server logs for errors
//!
//! This module watches log files, stdout/stderr of dev servers, and terminal output
//! to detect errors in real-time and trigger healing actions.

use crate::error::{NexusError, Result};
use crate::watcher::patterns::{DetectedError, ErrorSeverity, Language, PatternsDatabase};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, RwLock};
use tokio::fs::File;
use tokio::time::{interval, Duration};

/// A monitored log source
#[derive(Debug, Clone)]
pub struct LogSource {
    pub id: String,
    pub name: String,
    pub source_type: LogSourceType,
    pub project_path: PathBuf,
    pub language_hint: Option<Language>,
    pub enabled: bool,
}

/// Types of log sources we can monitor
#[derive(Debug, Clone)]
pub enum LogSourceType {
    /// A file on disk (e.g., dev server log file)
    File { path: PathBuf },
    /// A running process's stdout/stderr
    Process { command: String, args: Vec<String> },
    /// A pipe or stream (for integration with external tools)
    Stream { name: String },
}

/// An error event detected in logs
#[derive(Debug, Clone)]
pub struct LogErrorEvent {
    pub source_id: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub detected_error: DetectedError,
    pub raw_log_line: String,
    pub context_lines: Vec<String>,
}

/// Active file watcher handle
#[derive(Debug)]
struct FileWatcherHandle {
    #[allow(dead_code)]
    task: tokio::task::JoinHandle<()>,
    stop_tx: mpsc::Sender<()>,
}

/// Active process monitor handle
#[derive(Debug)]
struct ProcessMonitorHandle {
    #[allow(dead_code)]
    task: tokio::task::JoinHandle<()>,
    stop_tx: mpsc::Sender<()>,
}

/// The log monitor that watches all configured sources
pub struct LogMonitor {
    sources: Arc<RwLock<HashMap<String, LogSource>>>,
    patterns_db: Arc<PatternsDatabase>,
    error_tx: mpsc::Sender<LogErrorEvent>,
    file_watchers: Arc<RwLock<HashMap<String, FileWatcherHandle>>>,
    process_monitors: Arc<RwLock<HashMap<String, ProcessMonitorHandle>>>,
    context_buffer: Arc<RwLock<HashMap<String, Vec<String>>>>, // source_id -> recent lines
}

/// Log monitoring statistics
#[derive(Debug, Clone, Default)]
pub struct LogMonitorStats {
    pub total_sources: usize,
    pub active_sources: usize,
    pub total_errors_detected: usize,
    pub errors_by_type: HashMap<String, usize>,
    pub last_error_timestamp: Option<chrono::DateTime<chrono::Utc>>,
}

impl LogMonitor {
    pub fn new(error_tx: mpsc::Sender<LogErrorEvent>, _buffer_size: usize) -> Self {
        Self {
            sources: Arc::new(RwLock::new(HashMap::new())),
            patterns_db: Arc::new(PatternsDatabase::new()),
            error_tx,
            file_watchers: Arc::new(RwLock::new(HashMap::new())),
            process_monitors: Arc::new(RwLock::new(HashMap::new())),
            context_buffer: Arc::new(RwLock::new(HashMap::new())),
        }
    }
    
    /// Add a new log source to monitor
    pub async fn add_source(&self, source: LogSource) -> Result<()> {
        let mut sources = self.sources.write().await;
        
        // Check if already exists
        if sources.contains_key(&source.id) {
            return Err(NexusError::Configuration(
                format!("Log source '{}' already exists", source.id)
            ));
        }
        
        let source_id = source.id.clone();
        let source_name = source.name.clone();
        let source_type = source.source_type.clone();
        sources.insert(source_id.clone(), source);
        drop(sources);
        
        // Start appropriate monitoring based on source type
        match source_type {
            LogSourceType::File { path } => {
                self.start_file_watcher(&source_id, &path).await?;
            }
            LogSourceType::Process { command, args } => {
                self.start_process_monitor(&source_id, &command, &args).await?;
            }
            LogSourceType::Stream { .. } => {
                // Stream sources are handled externally via process_log_line
            }
        }
        
        println!("[LOG MONITOR] Added source: {} ({})", source_id, source_name);
        Ok(())
    }
    
    /// Start watching a log file with tail-like functionality
    pub async fn start_file_watcher(&self, source_id: &str, path: &PathBuf) -> Result<()> {
        let path = path.clone();
        let source_id_clone = source_id.to_string();
        let patterns_db = self.patterns_db.clone();
        let error_tx = self.error_tx.clone();
        let context_buffer = self.context_buffer.clone();
        
        // Create stop channel
        let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);
        
        // Spawn file watcher task
        let task = tokio::spawn(async move {
            // Open the file and seek to the end
            let file = match File::open(&path).await {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("[LOG MONITOR] Failed to open file {}: {}", path.display(), e);
                    return;
                }
            };
            
            // Get initial file size and seek to end
            let metadata = match tokio::fs::metadata(&path).await {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("[LOG MONITOR] Failed to get metadata for {}: {}", path.display(), e);
                    return;
                }
            };
            
            let start_position = metadata.len();
            let mut reader = BufReader::new(file);
            
            // Seek to end for tail-like behavior
            if let Err(e) = tokio::io::AsyncSeekExt::seek(&mut reader, std::io::SeekFrom::Start(start_position)).await {
                eprintln!("[LOG MONITOR] Failed to seek in {}: {}", path.display(), e);
                return;
            }
            
            println!("[LOG MONITOR] Started watching file: {} (starting at byte {})", path.display(), start_position);
            
            // Poll for new content every 100ms
            let mut poll_interval = interval(Duration::from_millis(100));
            let mut line_buffer = String::new();
            
            loop {
                tokio::select! {
                    _ = poll_interval.tick() => {
                        // Try to read new lines
                        loop {
                            line_buffer.clear();
                            match reader.read_line(&mut line_buffer).await {
                                Ok(0) => break, // No more data
                                Ok(_) => {
                                    let line = line_buffer.trim_end().to_string();
                                    if !line.is_empty() {
                                        // Process the line
                                        Self::process_single_line(
                                            &source_id_clone,
                                            &line,
                                            &patterns_db,
                                            &error_tx,
                                            &context_buffer,
                                        ).await;
                                    }
                                }
                                Err(e) => {
                                    eprintln!("[LOG MONITOR] Error reading from {}: {}", path.display(), e);
                                    break;
                                }
                            }
                        }
                    }
                    _ = stop_rx.recv() => {
                        println!("[LOG MONITOR] Stopping file watcher for: {}", path.display());
                        break;
                    }
                }
            }
        });
        
        // Store the handle
        let handle = FileWatcherHandle { task, stop_tx };
        let mut watchers = self.file_watchers.write().await;
        watchers.insert(source_id.to_string(), handle);
        
        Ok(())
    }
    
    /// Start monitoring a dev server process
    pub async fn start_process_monitor(&self, source_id: &str, command: &str, args: &[String]) -> Result<()> {
        let source_id_clone = source_id.to_string();
        let command = command.to_string();
        let args = args.to_vec();
        let patterns_db = self.patterns_db.clone();
        let error_tx = self.error_tx.clone();
        let context_buffer = self.context_buffer.clone();
        let project_path = {
            let sources = self.sources.read().await;
            sources.get(source_id)
                .map(|s| s.project_path.clone())
                .unwrap_or_else(|| PathBuf::from("."))
        };
        
        // Create stop channel
        let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);
        
        // Spawn process monitor task
        let task = tokio::spawn(async move {
            println!("[LOG MONITOR] Starting process monitor: {} {:?}", command, args);
            
            // Spawn the process
            let mut child = match Command::new(&command)
                .args(&args)
                .current_dir(&project_path)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[LOG MONITOR] Failed to spawn process {}: {}", command, e);
                    return;
                }
            };
            
            // Take stdout and stderr
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();
            
            // Create tasks to read stdout and stderr
            let stdout_task = {
                let source_id = source_id_clone.clone();
                let patterns_db = patterns_db.clone();
                let error_tx = error_tx.clone();
                let context_buffer = context_buffer.clone();
                
                tokio::spawn(async move {
                    if let Some(stdout) = stdout {
                        let reader = BufReader::new(stdout);
                        let mut lines = reader.lines();
                        
                        while let Ok(Some(line)) = lines.next_line().await {
                            println!("[{} stdout] {}", source_id, line);
                            Self::process_single_line(
                                &source_id,
                                &line,
                                &patterns_db,
                                &error_tx,
                                &context_buffer,
                            ).await;
                        }
                    }
                })
            };
            
            let stderr_task = {
                let source_id = source_id_clone.clone();
                let patterns_db = patterns_db.clone();
                let error_tx = error_tx.clone();
                let context_buffer = context_buffer.clone();
                
                tokio::spawn(async move {
                    if let Some(stderr) = stderr {
                        let reader = BufReader::new(stderr);
                        let mut lines = reader.lines();
                        
                        while let Ok(Some(line)) = lines.next_line().await {
                            eprintln!("[{} stderr] {}", source_id, line);
                            Self::process_single_line(
                                &source_id,
                                &line,
                                &patterns_db,
                                &error_tx,
                                &context_buffer,
                            ).await;
                        }
                    }
                })
            };
            
            // Wait for either the process to exit or stop signal
            tokio::select! {
                _ = stop_rx.recv() => {
                    println!("[LOG MONITOR] Stopping process monitor for: {}", source_id_clone);
                    let _ = child.kill().await;
                }
                status = child.wait() => {
                    match status {
                        Ok(code) => println!("[LOG MONITOR] Process exited with code: {:?}", code.code()),
                        Err(e) => eprintln!("[LOG MONITOR] Process error: {}", e),
                    }
                }
            }
            
            // Wait for stdout/stderr readers to finish
            let _ = stdout_task.await;
            let _ = stderr_task.await;
        });
        
        // Store the handle (the child is managed inside the task)
        let handle = ProcessMonitorHandle { 
            task, 
            stop_tx,
        };
        let mut monitors = self.process_monitors.write().await;
        monitors.insert(source_id.to_string(), handle);
        
        Ok(())
    }
    
    /// Process a single log line and detect errors
    async fn process_single_line(
        source_id: &str,
        line: &str,
        patterns_db: &Arc<PatternsDatabase>,
        error_tx: &mpsc::Sender<LogErrorEvent>,
        context_buffer: &Arc<RwLock<HashMap<String, Vec<String>>>>,
    ) {
        // Get language hint from source
        let language_hint = None; // Could be stored in context_buffer
        
        // Add line to context buffer (keep last 50 lines)
        {
            let mut buffers = context_buffer.write().await;
            let buffer = buffers.entry(source_id.to_string()).or_insert_with(Vec::new);
            buffer.push(line.to_string());
            if buffer.len() > 50 {
                buffer.remove(0);
            }
        }
        
        // Detect errors
        let detected_errors = patterns_db.detect_errors(line, language_hint);
        
        for detected in detected_errors {
            if detected.severity >= ErrorSeverity::Warning {
                // Get context lines
                let context_lines = {
                    let buffers = context_buffer.read().await;
                    buffers.get(source_id)
                        .cloned()
                        .unwrap_or_default()
                };
                
                let event = LogErrorEvent {
                    source_id: source_id.to_string(),
                    timestamp: chrono::Utc::now(),
                    detected_error: detected,
                    raw_log_line: line.to_string(),
                    context_lines,
                };
                
                if let Err(e) = error_tx.send(event).await {
                    eprintln!("[LOG MONITOR] Failed to send error event: {}", e);
                }
            }
        }
    }
    
    /// Remove a log source
    pub async fn remove_source(&self, source_id: &str) -> Result<()> {
        // Stop any active watchers for this source
        {
            let mut file_watchers = self.file_watchers.write().await;
            if let Some(handle) = file_watchers.remove(source_id) {
                let _ = handle.stop_tx.send(()).await;
                handle.task.abort();
                println!("[LOG MONITOR] Stopped file watcher for: {}", source_id);
            }
        }
        
        {
            let mut process_monitors = self.process_monitors.write().await;
            if let Some(handle) = process_monitors.remove(source_id) {
                let _ = handle.stop_tx.send(()).await;
                handle.task.abort();
                println!("[LOG MONITOR] Stopped process monitor for: {}", source_id);
            }
        }
        
        // Remove from sources
        let mut sources = self.sources.write().await;
        sources.remove(source_id);
        
        // Clean up context buffer
        {
            let mut buffers = self.context_buffer.write().await;
            buffers.remove(source_id);
        }
        
        println!("[LOG MONITOR] Removed source: {}", source_id);
        Ok(())
    }
    
    /// Enable/disable a source
    pub async fn set_source_enabled(&self, source_id: &str, _enabled: bool) -> Result<()> {
        let mut sources = self.sources.write().await;
        
        if let Some(source) = sources.get_mut(source_id) {
            source.enabled = true; // Simplified - just enable for now
            drop(sources);
            
            println!("[LOG MONITOR] Source {} enabled", source_id);
            Ok(())
        } else {
            Err(NexusError::Configuration(
                format!("Log source '{}' not found", source_id)
            ))
        }
    }
    
    /// Process a single log line from an external source (for stream sources)
    pub async fn process_log_line(&self, source_id: &str, line: &str) -> Result<()> {
        let sources = self.sources.read().await;
        
        if let Some(source) = sources.get(source_id) {
            if !source.enabled {
                return Ok(());
            }
            
            let language_hint = source.language_hint.clone();
            drop(sources);
            
            // Get the source-specific context buffer or create a global one
            // For simplicity, we'll check without full context
            let detected_errors = self.patterns_db.detect_errors(line, language_hint);
            
            for detected in detected_errors {
                if detected.severity >= ErrorSeverity::Error {
                    let event = LogErrorEvent {
                        source_id: source_id.to_string(),
                        timestamp: chrono::Utc::now(),
                        detected_error: detected,
                        raw_log_line: line.to_string(),
                        context_lines: vec![line.to_string()],
                    };
                    
                    if let Err(e) = self.error_tx.send(event).await {
                        eprintln!("[LOG MONITOR] Failed to send error event: {}", e);
                    }
                }
            }
            
            Ok(())
        } else {
            Err(NexusError::Configuration(
                format!("Log source '{}' not found", source_id)
            ))
        }
    }
    
    /// Get all configured sources
    pub async fn get_sources(&self) -> Vec<LogSource> {
        let sources = self.sources.read().await;
        sources.values().cloned().collect()
    }
    
    /// Get monitoring statistics
    pub async fn get_stats(&self) -> LogMonitorStats {
        let sources = self.sources.read().await;
        
        LogMonitorStats {
            total_sources: sources.len(),
            active_sources: sources.len(),
            total_errors_detected: 0, // Would track this in a real implementation
            errors_by_type: HashMap::new(),
            last_error_timestamp: None,
        }
    }
    
    /// Stop all monitoring
    pub async fn shutdown(&self) {
        // Stop all file watchers
        {
            let mut file_watchers = self.file_watchers.write().await;
            for (source_id, handle) in file_watchers.drain() {
                let _ = handle.stop_tx.send(()).await;
                handle.task.abort();
                println!("[LOG MONITOR] Stopped file watcher for: {}", source_id);
            }
        }
        
        // Stop all process monitors
        {
            let mut process_monitors = self.process_monitors.write().await;
            for (source_id, handle) in process_monitors.drain() {
                let _ = handle.stop_tx.send(()).await;
                handle.task.abort();
                println!("[LOG MONITOR] Stopped process monitor for: {}", source_id);
            }
        }
        
        // Clear sources
        {
            let mut sources = self.sources.write().await;
            sources.clear();
        }
        
        // Clear context buffers
        {
            let mut buffers = self.context_buffer.write().await;
            buffers.clear();
        }
        
        println!("[LOG MONITOR] All monitoring stopped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_log_source_creation() {
        let (tx, _rx) = mpsc::channel(100);
        let monitor = LogMonitor::new(tx, 10);
        
        let source = LogSource {
            id: "test".to_string(),
            name: "Test Source".to_string(),
            source_type: LogSourceType::Stream { name: "test".to_string() },
            project_path: PathBuf::from("/tmp"),
            language_hint: Some(Language::Rust),
            enabled: false,
        };
        
        monitor.add_source(source).await.unwrap();
        
        let sources = monitor.get_sources().await;
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].id, "test");
    }
}
