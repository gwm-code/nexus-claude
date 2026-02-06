pub mod cache;
pub mod diff;
pub mod file_tracker;
pub mod vector;
pub mod memory;

pub use file_tracker::FileAccessTracker;

use crate::error::{NexusError, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Represents the state of a file in the cache
#[derive(Debug, Clone)]
pub struct FileState {
    pub path: PathBuf,
    pub content_hash: String,
    pub last_modified: SystemTime,
    pub size: u64,
}

/// The ContextManager handles efficient context loading for large repositories
pub struct ContextManager {
    /// Cache of file states (path -> state)
    file_cache: HashMap<PathBuf, FileState>,
    /// Last time the cache was synced
    last_sync: Option<SystemTime>,
    /// Repository root path
    repo_root: PathBuf,
    /// Maximum files to cache
    max_cache_size: usize,
}

impl ContextManager {
    pub fn new(repo_root: PathBuf) -> Self {
        Self {
            file_cache: HashMap::new(),
            last_sync: None,
            repo_root,
            max_cache_size: 10_000, // Default: cache up to 10k files
        }
    }

    /// Perform a "warm handshake" - initial full scan of the repository
    /// This loads the entire file tree into cache
    pub async fn warm_handshake(&mut self) -> Result<HandshakeResult> {
        println!("[CONTEXT] Performing warm handshake...");
        let start = std::time::Instant::now();
        
        let mut files_scanned = 0;
        let mut total_size = 0u64;
        
        // Walk the repository
        for entry in walkdir::WalkDir::new(&self.repo_root)
            .into_iter()
            .filter_entry(|e| !Self::should_ignore(e.path()))
        {
            let entry = entry.map_err(|e| NexusError::Io(e.into()))?;
            
            if entry.file_type().is_file() {
                let path = entry.path();
                let metadata = entry.metadata()
                    .map_err(|e| NexusError::Io(e.into()))?;
                
                // Calculate content hash
                let content_hash = self.compute_file_hash(path)?;
                
                let file_state = FileState {
                    path: path.to_path_buf(),
                    content_hash,
                    last_modified: metadata.modified()
                        .unwrap_or_else(|_| SystemTime::now()),
                    size: metadata.len(),
                };
                
                total_size += file_state.size;
                self.file_cache.insert(path.to_path_buf(), file_state);
                files_scanned += 1;
                
                if files_scanned % 1000 == 0 {
                    println!("  Scanned {} files...", files_scanned);
                }
                
                if files_scanned >= self.max_cache_size {
                    println!("[CONTEXT] Reached max cache size ({})", self.max_cache_size);
                    break;
                }
            }
        }
        
        self.last_sync = Some(SystemTime::now());
        let duration = start.elapsed();
        
        println!("[CONTEXT] Warm handshake complete: {} files, {} MB in {:.2}s", 
            files_scanned, 
            total_size / 1_000_000,
            duration.as_secs_f64()
        );
        
        Ok(HandshakeResult {
            files_scanned,
            total_size,
            duration,
        })
    }

    /// Get only the files that have changed since last sync
    pub async fn get_diff_only(&mut self) -> Result<Vec<FileChange>> {
        let mut changes = Vec::new();
        let mut new_cache = HashMap::new();
        
        println!("[CONTEXT] Checking for changes...");
        
        for entry in walkdir::WalkDir::new(&self.repo_root)
            .into_iter()
            .filter_entry(|e| !Self::should_ignore(e.path()))
        {
            let entry = entry.map_err(|e| NexusError::Io(e.into()))?;
            
            if entry.file_type().is_file() {
                let path = entry.path();
                let metadata = entry.metadata()
                    .map_err(|e| NexusError::Io(e.into()))?;
                
                // Check if file is in cache
                if let Some(cached_state) = self.file_cache.get(path) {
                    // Check if modified
                    let modified_time = metadata.modified()
                        .unwrap_or_else(|_| SystemTime::now());
                    
                    if modified_time > cached_state.last_modified {
                        // File changed - compute new hash
                        let new_hash = self.compute_file_hash(path)?;
                        
                        if new_hash != cached_state.content_hash {
                            // Content actually changed
                            changes.push(FileChange {
                                path: path.to_path_buf(),
                                change_type: ChangeType::Modified,
                                old_hash: Some(cached_state.content_hash.clone()),
                                new_hash: Some(new_hash.clone()),
                            });
                            
                            // Update cache
                            let new_state = FileState {
                                path: path.to_path_buf(),
                                content_hash: new_hash,
                                last_modified: modified_time,
                                size: metadata.len(),
                            };
                            new_cache.insert(path.to_path_buf(), new_state);
                        } else {
                            // Only timestamp changed, content same
                            new_cache.insert(path.to_path_buf(), cached_state.clone());
                        }
                    } else {
                        // No change
                        new_cache.insert(path.to_path_buf(), cached_state.clone());
                    }
                } else {
                    // New file
                    let content_hash = self.compute_file_hash(path)?;
                    changes.push(FileChange {
                        path: path.to_path_buf(),
                        change_type: ChangeType::Added,
                        old_hash: None,
                        new_hash: Some(content_hash.clone()),
                    });
                    
                    let new_state = FileState {
                        path: path.to_path_buf(),
                        content_hash,
                        last_modified: metadata.modified()
                            .unwrap_or_else(|_| SystemTime::now()),
                        size: metadata.len(),
                    };
                    new_cache.insert(path.to_path_buf(), new_state);
                }
            }
        }
        
        // Check for deleted files
        for (path, state) in &self.file_cache {
            if !path.exists() {
                changes.push(FileChange {
                    path: path.clone(),
                    change_type: ChangeType::Deleted,
                    old_hash: Some(state.content_hash.clone()),
                    new_hash: None,
                });
            }
        }
        
        // Update cache
        self.file_cache = new_cache;
        self.last_sync = Some(SystemTime::now());
        
        if !changes.is_empty() {
            println!("[CONTEXT] Found {} changed files", changes.len());
        }
        
        Ok(changes)
    }

    /// Get the full file tree for initial context
    pub fn get_file_tree(&self) -> Vec<FileEntry> {
        self.file_cache
            .values()
            .map(|state| FileEntry {
                path: state.path.clone(),
                size: state.size,
            })
            .collect()
    }

    /// Read file content (with caching)
    pub fn read_file(&self, path: &Path) -> Result<String> {
        std::fs::read_to_string(path)
            .map_err(|e| NexusError::Io(e))
    }

    /// Compute hash of file content (first 1KB + size)
    fn compute_file_hash(&self, path: &Path) -> Result<String> {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let content = std::fs::read(path)
            .map_err(|e| NexusError::Io(e))?;
        
        // Use first 1KB + total size for fast hashing
        let sample = &content[..content.len().min(1024)];
        let mut hasher = DefaultHasher::new();
        sample.hash(&mut hasher);
        content.len().hash(&mut hasher);
        let hash = hasher.finish();
        
        Ok(format!("{:x}", hash))
    }

    /// Check if a path should be ignored (gitignore-style)
    fn should_ignore(path: &Path) -> bool {
        let path_str = path.to_string_lossy();
        
        // Common directories to ignore
        let ignore_patterns = [
            "node_modules",
            ".git",
            "target",
            "__pycache__",
            ".venv",
            "venv",
            ".idea",
            ".vscode",
            "dist",
            "build",
            ".nuxt",
            ".next",
            "*.log",
        ];
        
        ignore_patterns.iter().any(|pattern| path_str.contains(pattern))
    }

    /// Get cache statistics
    pub fn get_stats(&self) -> CacheStats {
        let total_files = self.file_cache.len();
        let total_size: u64 = self.file_cache.values()
            .map(|s| s.size)
            .sum();
        
        CacheStats {
            total_files,
            total_size,
            last_sync: self.last_sync,
        }
    }
}

#[derive(Debug)]
pub struct HandshakeResult {
    pub files_scanned: usize,
    pub total_size: u64,
    pub duration: Duration,
}

#[derive(Debug, Clone)]
pub struct FileChange {
    pub path: PathBuf,
    pub change_type: ChangeType,
    pub old_hash: Option<String>,
    pub new_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ChangeType {
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: PathBuf,
    pub size: u64,
}

#[derive(Debug)]
pub struct CacheStats {
    pub total_files: usize,
    pub total_size: u64,
    pub last_sync: Option<SystemTime>,
}
