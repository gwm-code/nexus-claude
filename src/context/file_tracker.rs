//! File Access Tracker - Prevents concurrent modification issues
//!
//! This module tracks when files are read and checks for staleness before edits.
//! It prevents multiple agents from overwriting each other's changes.

use crate::error::{NexusError, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

/// Thread-safe tracker for file read operations
///
/// This tracker records when each file was last read by the current session.
/// Before editing a file, we check if it has been modified since the last read.
/// This prevents stale edits in concurrent environments with multiple agents.
#[derive(Debug, Clone)]
pub struct FileAccessTracker {
    /// Maps file paths to the time they were last read
    read_timestamps: Arc<RwLock<HashMap<PathBuf, SystemTime>>>,
}

impl FileAccessTracker {
    /// Create a new FileAccessTracker
    pub fn new() -> Self {
        Self {
            read_timestamps: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Record that a file has been read
    ///
    /// This should be called whenever a file is read to track the read timestamp.
    /// The timestamp is used later to detect if the file has been modified.
    ///
    /// # Arguments
    /// * `path` - The path to the file that was read
    pub fn record_read(&self, path: &Path) {
        let canonical_path = match self.canonicalize_path(path) {
            Some(p) => p,
            None => path.to_path_buf(),
        };

        let now = SystemTime::now();

        if let Ok(mut timestamps) = self.read_timestamps.write() {
            timestamps.insert(canonical_path, now);
        }
    }

    /// Check if a file is stale (modified since last read)
    ///
    /// Returns Ok(()) if the file is fresh (not modified since last read)
    /// or if the file has never been read (new file scenario).
    /// Returns Err(NexusError::FileStale) if the file has been modified.
    ///
    /// # Arguments
    /// * `path` - The path to check for staleness
    ///
    /// # Errors
    /// Returns `NexusError::FileStale` if the file's modification time
    /// is newer than when it was last read.
    pub fn check_staleness(&self, path: &Path) -> Result<()> {
        let canonical_path = match self.canonicalize_path(path) {
            Some(p) => p,
            None => path.to_path_buf(),
        };

        // Get the last read timestamp for this file
        let last_read = {
            let timestamps = self.read_timestamps.read().map_err(|_| {
                NexusError::Configuration("Failed to acquire read lock".to_string())
            })?;
            timestamps.get(&canonical_path).copied()
        };

        // If we've never read this file, we can't determine staleness
        // In this case, allow the operation (new file scenario)
        let last_read = match last_read {
            Some(time) => time,
            None => return Ok(()),
        };

        // Check the file's current modification time
        let metadata = match std::fs::metadata(&canonical_path) {
            Ok(m) => m,
            Err(e) => {
                // If file doesn't exist, it's not stale (it might be a new file)
                if e.kind() == std::io::ErrorKind::NotFound {
                    return Ok(());
                }
                return Err(NexusError::Io(e));
            }
        };

        let modified_time = match metadata.modified() {
            Ok(t) => t,
            Err(_) => {
                // If we can't get modification time, assume it's fresh
                return Ok(());
            }
        };

        // Compare modification time with last read time
        // If modified_time > last_read, the file is stale
        if modified_time > last_read {
            return Err(NexusError::FileStale {
                path: canonical_path.display().to_string(),
            });
        }

        Ok(())
    }

    /// Check staleness for multiple files at once
    ///
    /// Returns the first stale file found, or Ok if all are fresh.
    ///
    /// # Arguments
    /// * `paths` - Iterator of paths to check
    pub fn check_staleness_batch<'a>(&self, paths: impl Iterator<Item = &'a Path>) -> Result<()> {
        for path in paths {
            self.check_staleness(path)?;
        }
        Ok(())
    }

    /// Get the last read timestamp for a file
    ///
    /// Returns None if the file has never been read
    pub fn get_last_read(&self, path: &Path) -> Option<SystemTime> {
        let canonical_path = self.canonicalize_path(path)?;

        let timestamps = self.read_timestamps.read().ok()?;
        timestamps.get(&canonical_path).copied()
    }

    /// Remove a file from tracking
    ///
    /// Useful when a file is deleted
    pub fn remove_tracking(&self, path: &Path) {
        let canonical_path = match self.canonicalize_path(path) {
            Some(p) => p,
            None => return,
        };

        if let Ok(mut timestamps) = self.read_timestamps.write() {
            timestamps.remove(&canonical_path);
        }
    }

    /// Clear all tracking data
    pub fn clear(&self) {
        if let Ok(mut timestamps) = self.read_timestamps.write() {
            timestamps.clear();
        }
    }

    /// Get the number of tracked files
    pub fn tracked_count(&self) -> usize {
        self.read_timestamps
            .read()
            .map(|timestamps| timestamps.len())
            .unwrap_or(0)
    }

    /// Canonicalize a path for consistent tracking
    fn canonicalize_path(&self, path: &Path) -> Option<PathBuf> {
        // Try to canonicalize, but fall back to the original if it fails
        // (e.g., for files that don't exist yet)
        path.canonicalize()
            .ok()
            .or_else(|| Some(path.to_path_buf()))
    }
}

impl Default for FileAccessTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_record_and_check_fresh_file() {
        let tracker = FileAccessTracker::new();
        let mut temp_file = NamedTempFile::new().unwrap();

        // Write initial content
        temp_file.write_all(b"initial content").unwrap();
        temp_file.flush().unwrap();

        let path = temp_file.path().to_path_buf();

        // Record read
        tracker.record_read(&path);

        // File should be fresh immediately after read
        assert!(tracker.check_staleness(&path).is_ok());
    }

    #[test]
    fn test_detect_stale_file() {
        let tracker = FileAccessTracker::new();
        let mut temp_file = NamedTempFile::new().unwrap();

        // Write initial content and flush
        temp_file.write_all(b"initial content").unwrap();
        temp_file.flush().unwrap();

        let path = temp_file.path().to_path_buf();

        // Record read
        tracker.record_read(&path);

        // Wait a tiny bit to ensure different timestamps
        std::thread::sleep(std::time::Duration::from_millis(10));

        // Get a fresh handle and modify the file by path
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        file.write_all(b"modified content").unwrap();
        file.flush().unwrap();
        drop(file);

        // File should now be stale
        let result = tracker.check_staleness(&path);
        assert!(result.is_err());

        // Verify it's the right error type
        match result {
            Err(NexusError::FileStale { .. }) => (),
            _ => panic!("Expected FileStale error"),
        }
    }

    #[test]
    fn test_unread_file_not_stale() {
        let tracker = FileAccessTracker::new();
        let temp_file = NamedTempFile::new().unwrap();

        // Don't record a read
        // File that has never been read should not be considered stale
        assert!(tracker.check_staleness(temp_file.path()).is_ok());
    }

    #[test]
    fn test_nonexistent_file() {
        let tracker = FileAccessTracker::new();
        let path = PathBuf::from("/tmp/nonexistent_file_12345.txt");

        // Non-existent files should not be stale (new file scenario)
        assert!(tracker.check_staleness(&path).is_ok());
    }

    #[test]
    fn test_clear_tracking() {
        let tracker = FileAccessTracker::new();
        let temp_file = NamedTempFile::new().unwrap();

        tracker.record_read(temp_file.path());
        assert_eq!(tracker.tracked_count(), 1);

        tracker.clear();
        assert_eq!(tracker.tracked_count(), 0);
    }

    #[test]
    fn test_batch_check() {
        let tracker = FileAccessTracker::new();
        let temp_file1 = NamedTempFile::new().unwrap();
        let temp_file2 = NamedTempFile::new().unwrap();

        tracker.record_read(temp_file1.path());
        tracker.record_read(temp_file2.path());

        let paths = [temp_file1.path(), temp_file2.path()];
        assert!(tracker.check_staleness_batch(paths.iter().copied()).is_ok());
    }
}
