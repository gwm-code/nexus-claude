/// File cache operations
use crate::error::Result;
use std::collections::HashMap;
use std::path::PathBuf;

/// Simple in-memory file content cache
pub struct FileCache {
    cache: HashMap<PathBuf, String>,
    max_size: usize,
}

impl FileCache {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            max_size: 1000, // Cache up to 1000 files in memory
        }
    }

    pub fn get(&self, path: &PathBuf) -> Option<&String> {
        self.cache.get(path)
    }

    pub fn insert(&mut self, path: PathBuf, content: String) {
        if self.cache.len() >= self.max_size {
            // Simple LRU: remove arbitrary entry
            if let Some(first_key) = self.cache.keys().next().cloned() {
                self.cache.remove(&first_key);
            }
        }
        self.cache.insert(path, content);
    }

    pub fn clear(&mut self) {
        self.cache.clear();
    }
}
