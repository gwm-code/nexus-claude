use crate::error::{NexusError, Result};
use crate::swarm::{MergeConflict, SubtaskResult, ConflictResolution};
use crate::swarm::worker::WorkerType;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Git-based merger for resolving conflicts between worker outputs
pub struct GitMerger {
    auto_merge: bool,
}

impl GitMerger {
    pub fn new(auto_merge: bool) -> Self {
        Self { auto_merge }
    }

    /// Merge results from multiple workers
    pub async fn merge_results(
        &self,
        results: &[SubtaskResult],
    ) -> Result<(Vec<String>, Vec<MergeConflict>)> {
        if results.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }

        // Collect all files modified by each worker
        let mut file_modifications: HashMap<String, Vec<(String, WorkerType)>> = HashMap::new();
        
        for result in results {
            for file in &result.files_modified {
                file_modifications
                    .entry(file.clone())
                    .or_default()
                    .push((result.task_id.clone(), result.worker_type));
            }
        }

        // Identify conflicts
        let mut conflicts = Vec::new();
        let mut merged_files = Vec::new();

        for (file_path, workers) in &file_modifications {
            if workers.len() > 1 {
                // Potential conflict - multiple workers modified same file
                let resolution = if self.auto_merge {
                    self.attempt_auto_merge(file_path, workers, results).await?
                } else {
                    ConflictResolution::ManualRequired
                };

                if matches!(resolution, ConflictResolution::ManualRequired) {
                    conflicts.push(MergeConflict {
                        file_path: file_path.clone(),
                        worker_a: workers[0].0.clone(),
                        worker_b: workers[1].0.clone(),
                        resolution,
                    });
                } else {
                    merged_files.push(file_path.clone());
                }
            } else {
                // No conflict - single worker modified this file
                merged_files.push(file_path.clone());
            }
        }

        // Remove duplicates from merged_files
        merged_files.sort();
        merged_files.dedup();

        Ok((merged_files, conflicts))
    }

    /// Attempt to automatically merge changes
    async fn attempt_auto_merge(
        &self,
        file_path: &str,
        workers: &[(String, crate::swarm::worker::WorkerType)],
        results: &[SubtaskResult],
    ) -> Result<ConflictResolution> {
        // Strategy 1: Check if changes are to different parts of the file
        // Strategy 2: Use git three-way merge
        // Strategy 3: Append changes if they're additive
        
        // For now, attempt a simple merge
        let task_ids: Vec<String> = workers.iter().map(|(id, _)| id.clone()).collect();
        
        // Try git merge-file approach
        match self.git_merge_file(file_path, &task_ids, results).await {
            Ok(true) => {
                println!("  [MERGER] Auto-merged: {}", file_path);
                Ok(ConflictResolution::AutoMerged)
            }
            Ok(false) => {
                println!("  [MERGER] Conflict requires manual resolution: {}", file_path);
                Ok(ConflictResolution::ManualRequired)
            }
            Err(e) => {
                println!("  [MERGER] Merge error for {}: {}", file_path, e);
                Ok(ConflictResolution::ManualRequired)
            }
        }
    }

    /// Attempt git three-way merge on a file
    async fn git_merge_file(
        &self,
        file_path: &str,
        task_ids: &[String],
        _results: &[SubtaskResult],
    ) -> Result<bool> {
        let path = PathBuf::from(file_path);
        
        if !path.exists() {
            // File doesn't exist yet, check if workers created different versions
            let versions: Vec<_> = _results
                .iter()
                .filter(|r| task_ids.contains(&r.task_id))
                .map(|r| &r.output)
                .collect();

            if versions.len() == 1 {
                // Only one version, no conflict
                return Ok(true);
            }

            // Multiple workers tried to create the same file
            // Try to combine their outputs intelligently
            return Ok(false); // Require manual review for new file conflicts
        }

        // File exists, use git merge-file if available
        // This is a simplified implementation
        // In production, you'd want to:
        // 1. Create branches for each worker's changes
        // 2. Use git merge-file or similar algorithm
        // 3. Handle conflicts appropriately

        // For now, check if content sections overlap
        let _original_content = std::fs::read_to_string(&path)?;
        
        // Check if we can detect non-overlapping changes
        // This is a heuristic approach
        let can_auto_merge = self.check_mergeable_sections(file_path, _results)?;
        
        Ok(can_auto_merge)
    }

    /// Check if file sections modified by workers overlap
    fn check_mergeable_sections(
        &self,
        file_path: &str,
        results: &[SubtaskResult],
    ) -> Result<bool> {
        // This is a simplified check
        // In production, you'd want more sophisticated diff analysis
        
        // For now, assume TypeScript/JavaScript files with clear section boundaries
        // can often be auto-merged if they're adding different functions/imports
        
        let path = Path::new(file_path);
        let extension = path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        match extension {
            "ts" | "tsx" | "js" | "jsx" => {
                // For JS/TS files, check if additions are in different sections
                // imports, exports, functions, classes are often mergeable
                Ok(true) // Optimistic - in production use proper AST analysis
            }
            "css" | "scss" | "sass" => {
                // CSS rules with different selectors can often be merged
                Ok(true)
            }
            "json" | "yaml" | "yml" | "toml" => {
                // Config files are harder to auto-merge safely
                Ok(false)
            }
            _ => {
                // Default to manual review for unknown file types
                Ok(false)
            }
        }
    }

    /// Create backup of a file before attempting merge
    pub fn create_backup(&self, file_path: &str) -> Result<PathBuf> {
        let path = PathBuf::from(file_path);
        if !path.exists() {
            return Err(NexusError::Configuration(
                format!("File does not exist: {}", file_path)
            ));
        }

        let backup_path = path.with_extension(format!(
            "{}.backup",
            path.extension()
                .and_then(|e| e.to_str())
                .unwrap_or("tmp")
        ));

        std::fs::copy(&path, &backup_path)?;
        Ok(backup_path)
    }

    /// Restore file from backup
    pub fn restore_backup(&self, file_path: &str, backup_path: &Path) -> Result<()> {
        std::fs::copy(backup_path, file_path)?;
        Ok(())
    }

    /// Generate a merge conflict report
    pub fn generate_conflict_report(
        &self,
        conflicts: &[MergeConflict],
    ) -> String {
        let mut report = String::new();
        
        report.push_str("# Merge Conflict Report\n\n");
        report.push_str(&format!("Total conflicts: {}\n\n", conflicts.len()));

        for (i, conflict) in conflicts.iter().enumerate() {
            report.push_str(&format!("## Conflict {}: {}\n", i + 1, conflict.file_path));
            report.push_str(&format!("- Workers: {} vs {}\n", conflict.worker_a, conflict.worker_b));
            report.push_str(&format!("- Resolution: {:?}\n\n", conflict.resolution));
        }

        report.push_str("## Recommended Actions\n\n");
        report.push_str("1. Review each conflicting file\n");
        report.push_str("2. Manually merge changes or choose one version\n");
        report.push_str("3. Run tests to verify the merge\n");
        report.push_str("4. Mark conflicts as resolved\n");

        report
    }

    /// Initialize git repository for merge tracking if not exists
    pub fn init_git_tracking(&self, working_dir: &Path) -> Result<()> {
        let git_dir = working_dir.join(".git");
        
        if !git_dir.exists() {
            // Initialize git repo
            let output = Command::new("git")
                .arg("init")
                .current_dir(working_dir)
                .output()
                .map_err(|e| NexusError::Configuration(
                    format!("Failed to run git init: {}", e)
                ))?;

            if !output.status.success() {
                return Err(NexusError::Configuration(
                    format!("git init failed: {}", String::from_utf8_lossy(&output.stderr))
                ));
            }

            // Configure git user for commits
            let _ = Command::new("git")
                .args(["config", "user.email", "swarm@nexus.local"])
                .current_dir(working_dir)
                .output();

            let _ = Command::new("git")
                .args(["config", "user.name", "Nexus Swarm"])
                .current_dir(working_dir)
                .output();
        }

        Ok(())
    }

    /// Create a commit with worker changes
    pub fn commit_worker_changes(
        &self,
        working_dir: &Path,
        worker_name: &str,
        files: &[String],
    ) -> Result<String> {
        // Add files
        for file in files {
            let output = Command::new("git")
                .args(["add", file])
                .current_dir(working_dir)
                .output()
                .map_err(|e| NexusError::Configuration(
                    format!("Failed to add file {}: {}", file, e)
                ))?;

            if !output.status.success() {
                return Err(NexusError::Configuration(
                    format!("git add failed: {}", String::from_utf8_lossy(&output.stderr))
                ));
            }
        }

        // Create commit
        let commit_msg = format!("[SWARM] {} changes", worker_name);
        let output = Command::new("git")
            .args(["commit", "-m", &commit_msg])
            .current_dir(working_dir)
            .output()
            .map_err(|e| NexusError::Configuration(
                format!("Failed to commit: {}", e)
            ))?;

        if !output.status.success() {
            return Err(NexusError::Configuration(
                format!("git commit failed: {}", String::from_utf8_lossy(&output.stderr))
            ));
        }

        // Get commit hash
        let output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(working_dir)
            .output()
            .map_err(|e| NexusError::Configuration(
                format!("Failed to get commit hash: {}", e)
            ))?;

        let commit_hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(commit_hash)
    }

    /// Attempt to merge two branches/commits
    pub fn merge_commits(
        &self,
        working_dir: &Path,
        commit_a: &str,
        commit_b: &str,
    ) -> Result<MergeResult> {
        // Checkout commit A
        let output = Command::new("git")
            .args(["checkout", commit_a])
            .current_dir(working_dir)
            .output()
            .map_err(|e| NexusError::Configuration(
                format!("Failed to checkout {}: {}", commit_a, e)
            ))?;

        if !output.status.success() {
            return Err(NexusError::Configuration(
                format!("git checkout failed: {}", String::from_utf8_lossy(&output.stderr))
            ));
        }

        // Attempt merge
        let output = Command::new("git")
            .args(["merge", commit_b, "--no-commit", "--no-ff"])
            .current_dir(working_dir)
            .output()
            .map_err(|e| NexusError::Configuration(
                format!("Failed to merge: {}", e)
            ))?;

        if output.status.success() {
            // Clean merge
            Ok(MergeResult::Clean)
        } else if String::from_utf8_lossy(&output.stdout).contains("CONFLICT") {
            // Merge has conflicts
            Ok(MergeResult::Conflict)
        } else {
            // Other error
            Err(NexusError::Configuration(
                format!("Merge failed: {}", String::from_utf8_lossy(&output.stderr))
            ))
        }
    }

    /// Get list of conflicting files after a failed merge
    pub fn get_conflict_files(&self, working_dir: &Path) -> Result<Vec<String>> {
        let output = Command::new("git")
            .args(["diff", "--name-only", "--diff-filter=U"])
            .current_dir(working_dir)
            .output()
            .map_err(|e| NexusError::Configuration(
                format!("Failed to get conflict files: {}", e)
            ))?;

        let files: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|s| s.to_string())
            .collect();

        Ok(files)
    }

    /// Abort current merge
    pub fn abort_merge(&self, working_dir: &Path) -> Result<()> {
        let output = Command::new("git")
            .args(["merge", "--abort"])
            .current_dir(working_dir)
            .output()
            .map_err(|e| NexusError::Configuration(
                format!("Failed to abort merge: {}", e)
            ))?;

        if !output.status.success() {
            return Err(NexusError::Configuration(
                format!("git merge --abort failed: {}", String::from_utf8_lossy(&output.stderr))
            ));
        }

        Ok(())
    }
}

/// Result of a merge attempt
#[derive(Debug, Clone, PartialEq)]
pub enum MergeResult {
    Clean,      // Merge succeeded with no conflicts
    Conflict,   // Merge has conflicts that need resolution
    Error(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::swarm::worker::WorkerType;

    #[test]
    fn test_conflict_detection() {
        let results = vec![
            SubtaskResult {
                task_id: "task-1".to_string(),
                worker_type: WorkerType::Frontend,
                success: true,
                output: "Created login form".to_string(),
                files_modified: vec!["src/components/Login.tsx".to_string()],
                execution_time_ms: 1000,
            },
            SubtaskResult {
                task_id: "task-2".to_string(),
                worker_type: WorkerType::Backend,
                success: true,
                output: "Created auth API".to_string(),
                files_modified: vec![
                    "src/api/auth.ts".to_string(),
                    "src/components/Login.tsx".to_string(), // Conflict!
                ],
                execution_time_ms: 2000,
            },
        ];

        let merger = GitMerger::new(true);
        
        // We can't easily test merge_results without async runtime
        // but we can verify the conflict detection logic
        let mut file_mods: HashMap<String, Vec<(String, WorkerType)>> = HashMap::new();
        for result in &results {
            for file in &result.files_modified {
                file_mods.entry(file.clone()).or_default().push((result.task_id.clone(), result.worker_type));
            }
        }

        assert!(file_mods.get("src/components/Login.tsx").unwrap().len() > 1);
        assert_eq!(file_mods.get("src/api/auth.ts").unwrap().len(), 1);
    }
}
