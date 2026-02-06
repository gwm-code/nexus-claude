use crate::context::FileAccessTracker;
use crate::error::{NexusError, Result};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct HydrationPlan {
    pub files_to_create: Vec<FileChange>,
    pub files_to_update: Vec<FileChange>,
    pub files_to_delete: Vec<PathBuf>,
    pub directories_to_create: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct FileChange {
    pub path: PathBuf,
    pub content: String,
    pub backup_path: Option<PathBuf>,
}

pub struct Hydrator {
    backup_dir: PathBuf,
}

impl Hydrator {
    pub fn new() -> Result<Self> {
        let backup_dir = Self::get_backup_dir()?;
        fs::create_dir_all(&backup_dir)?;

        Ok(Self { backup_dir })
    }

    pub fn create_plan(&self, sandbox_dir: &Path, host_dir: &Path) -> Result<HydrationPlan> {
        let mut plan = HydrationPlan {
            files_to_create: Vec::new(),
            files_to_update: Vec::new(),
            files_to_delete: Vec::new(),
            directories_to_create: Vec::new(),
        };

        // Walk the sandbox directory
        self.walk_sandbox(sandbox_dir, host_dir, sandbox_dir, &mut plan)?;

        Ok(plan)
    }

    fn walk_sandbox(
        &self,
        current_sandbox: &Path,
        host_dir: &Path,
        sandbox_root: &Path,
        plan: &mut HydrationPlan,
    ) -> Result<()> {
        for entry in fs::read_dir(current_sandbox)? {
            let entry = entry?;
            let path = entry.path();
            let relative_path = path.strip_prefix(sandbox_root).unwrap();
            let host_path = host_dir.join(relative_path);

            if path.is_dir() {
                if !host_path.exists() {
                    plan.directories_to_create.push(host_path.clone());
                }
                self.walk_sandbox(&path, host_dir, sandbox_root, plan)?;
            } else {
                let content = fs::read_to_string(&path)?;

                if host_path.exists() {
                    let host_content = fs::read_to_string(&host_path)?;
                    if content != host_content {
                        // File exists and content differs - update needed
                        let backup_path = if self.should_backup(&host_path) {
                            Some(self.create_backup(&host_path)?)
                        } else {
                            None
                        };

                        plan.files_to_update.push(FileChange {
                            path: host_path,
                            content,
                            backup_path,
                        });
                    }
                } else {
                    // File doesn't exist - create
                    plan.files_to_create.push(FileChange {
                        path: host_path,
                        content,
                        backup_path: None,
                    });
                }
            }
        }

        Ok(())
    }

    /// Execute a hydration plan, optionally checking file staleness
    ///
    /// If a file_tracker is provided, it will check that files haven't been modified
    /// since they were last read before applying changes. This prevents concurrent
    /// modification issues when multiple agents are working on the same files.
    pub fn execute_plan(&self, plan: &HydrationPlan) -> Result<Vec<PathBuf>> {
        self.execute_plan_with_tracker(plan, None)
    }

    /// Execute a hydration plan with an optional FileAccessTracker for staleness checking
    pub fn execute_plan_with_tracker(
        &self,
        plan: &HydrationPlan,
        file_tracker: Option<&FileAccessTracker>,
    ) -> Result<Vec<PathBuf>> {
        let mut applied_changes = Vec::new();

        // Check staleness for files to be updated if tracker is provided
        if let Some(tracker) = file_tracker {
            for file in &plan.files_to_update {
                tracker.check_staleness(&file.path)?;
            }
        }

        // Create directories
        for dir in &plan.directories_to_create {
            fs::create_dir_all(dir)?;
            applied_changes.push(dir.clone());
        }

        // Create new files
        for file in &plan.files_to_create {
            fs::write(&file.path, &file.content)?;
            applied_changes.push(file.path.clone());
        }

        // Update existing files
        for file in &plan.files_to_update {
            fs::write(&file.path, &file.content)?;
            applied_changes.push(file.path.clone());
        }

        // Delete files (if any)
        for path in &plan.files_to_delete {
            if path.exists() {
                fs::remove_file(path)?;
                applied_changes.push(path.clone());
            }
        }

        Ok(applied_changes)
    }

    pub fn rollback(&self, plan: &HydrationPlan) -> Result<()> {
        // Restore from backups
        for file in &plan.files_to_update {
            if let Some(ref backup) = file.backup_path {
                if backup.exists() {
                    fs::copy(backup, &file.path)?;
                }
            }
        }

        // Remove created files
        for file in &plan.files_to_create {
            if file.path.exists() {
                fs::remove_file(&file.path)?;
            }
        }

        // Remove created directories (reverse order - deepest first)
        let mut dirs = plan.directories_to_create.clone();
        dirs.sort_by(|a, b| b.components().count().cmp(&a.components().count()));
        for dir in dirs {
            if dir.exists() {
                let _ = fs::remove_dir(&dir);
            }
        }

        Ok(())
    }

    fn should_backup(&self, path: &Path) -> bool {
        // Backup files that exist and are not in node_modules, .git, etc.
        let path_str = path.to_string_lossy();
        !path_str.contains("node_modules")
            && !path_str.contains(".git")
            && !path_str.contains("target")
            && !path_str.contains("__pycache__")
    }

    fn create_backup(&self, original: &Path) -> Result<PathBuf> {
        let filename = original
            .file_name()
            .ok_or_else(|| NexusError::Configuration("Invalid file path".to_string()))?;

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let backup_name = format!("{}.{}", filename.to_string_lossy(), timestamp);
        let backup_path = self.backup_dir.join(backup_name);

        fs::copy(original, &backup_path)?;

        Ok(backup_path)
    }

    fn get_backup_dir() -> Result<PathBuf> {
        let project_dirs =
            directories::ProjectDirs::from("com", "nexus", "nexus").ok_or_else(|| {
                NexusError::Configuration("Could not determine backup directory".to_string())
            })?;

        Ok(project_dirs.data_dir().join("backups"))
    }

    pub fn cleanup_old_backups(&self, max_age_hours: u64) -> Result<usize> {
        let mut cleaned = 0;
        let now = std::time::SystemTime::now();

        for entry in fs::read_dir(&self.backup_dir)? {
            let entry = entry?;
            let metadata = entry.metadata()?;

            if let Ok(modified) = metadata.modified() {
                if let Ok(age) = now.duration_since(modified) {
                    if age.as_secs() > max_age_hours * 3600 {
                        fs::remove_file(entry.path())?;
                        cleaned += 1;
                    }
                }
            }
        }

        Ok(cleaned)
    }
}
