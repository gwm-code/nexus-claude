use crate::error::{NexusError, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub running: bool,
    pub pid: Option<u32>,
    pub interval_hours: Option<u8>,
    pub last_run: Option<String>,
    pub next_run: Option<String>,
}

pub struct DaemonManager {
    pid_file: PathBuf,
    status_file: PathBuf,
}

impl DaemonManager {
    pub fn new() -> Result<Self> {
        let config_dir = std::env::var("HOME")
            .map(|h| PathBuf::from(h).join(".config/nexus"))
            .unwrap_or_else(|_| PathBuf::from("~/.config/nexus"));

        fs::create_dir_all(&config_dir)?;

        Ok(Self {
            pid_file: config_dir.join("daemon.pid"),
            status_file: config_dir.join("daemon.status"),
        })
    }

    pub fn start(&self, interval_hours: u8) -> Result<()> {
        if interval_hours == 0 {
            return Err(NexusError::Configuration(
                "Interval must be between 1-24 hours. Use 'daemon stop' to disable.".to_string()
            ));
        }

        if interval_hours > 24 {
            return Err(NexusError::Configuration(
                "Interval cannot exceed 24 hours".to_string()
            ));
        }

        // Check if daemon already running
        if let Ok(status) = self.status() {
            if status.running {
                return Err(NexusError::Configuration(
                    format!("Daemon already running with PID {}", status.pid.unwrap_or(0))
                ));
            }
        }

        // Get the current executable path
        let exe = std::env::current_exe()?;

        // Spawn daemon as background process
        let child = Command::new(&exe)
            .arg("daemon")
            .arg("run-tasks")
            .arg("--loop")
            .arg(interval_hours.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;

        // Write PID file
        fs::write(&self.pid_file, child.id().to_string())?;

        // Write initial status
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let next_run_timestamp = now + (interval_hours as u64 * 3600);

        let status = DaemonStatus {
            running: true,
            pid: Some(child.id()),
            interval_hours: Some(interval_hours),
            last_run: None,
            next_run: Some(format_timestamp(next_run_timestamp)),
        };

        self.write_status(&status)?;

        Ok(())
    }

    pub fn stop(&self) -> Result<()> {
        let status = self.status()?;

        if !status.running {
            return Err(NexusError::Configuration(
                "Daemon is not running".to_string()
            ));
        }

        if let Some(pid) = status.pid {
            // Send SIGTERM to daemon process
            #[cfg(unix)]
            {
                use nix::sys::signal::{kill, Signal};
                use nix::unistd::Pid;
                kill(Pid::from_raw(pid as i32), Signal::SIGTERM)
                    .map_err(|e| NexusError::Configuration(format!("Failed to kill daemon process: {}", e)))?;
            }

            #[cfg(not(unix))]
            {
                // On Windows, use taskkill
                Command::new("taskkill")
                    .args(&["/PID", &pid.to_string(), "/F"])
                    .output()?;
            }

            // Clean up files
            let _ = fs::remove_file(&self.pid_file);
            let _ = fs::remove_file(&self.status_file);
        }

        Ok(())
    }

    pub fn status(&self) -> Result<DaemonStatus> {
        // Read PID file
        let pid = if self.pid_file.exists() {
            let pid_str = fs::read_to_string(&self.pid_file)?;
            pid_str.trim().parse::<u32>().ok()
        } else {
            None
        };

        // Check if process is actually running
        let running = if let Some(pid) = pid {
            is_process_running(pid)
        } else {
            false
        };

        // If PID file exists but process isn't running, clean up
        if !running && self.pid_file.exists() {
            let _ = fs::remove_file(&self.pid_file);
            let _ = fs::remove_file(&self.status_file);
        }

        // Read status file if it exists
        if running && self.status_file.exists() {
            let status_str = fs::read_to_string(&self.status_file)?;
            if let Ok(mut status) = serde_json::from_str::<DaemonStatus>(&status_str) {
                status.running = running;
                status.pid = pid;
                return Ok(status);
            }
        }

        // Return basic status
        Ok(DaemonStatus {
            running,
            pid,
            interval_hours: None,
            last_run: None,
            next_run: None,
        })
    }

    fn write_status(&self, status: &DaemonStatus) -> Result<()> {
        let status_json = serde_json::to_string_pretty(status)?;
        fs::write(&self.status_file, status_json)?;
        Ok(())
    }

    pub fn update_last_run(&self) -> Result<()> {
        let mut status = self.status()?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        status.last_run = Some(format_timestamp(now));

        if let Some(interval) = status.interval_hours {
            let next_run_timestamp = now + (interval as u64 * 3600);
            status.next_run = Some(format_timestamp(next_run_timestamp));
        }

        self.write_status(&status)?;
        Ok(())
    }
}

/// Run proactive tasks once
pub async fn run_proactive_tasks() -> Result<()> {
    println!("[DAEMON] Running proactive tasks...");

    // Task 1: Memory consolidation
    println!("[DAEMON] - Memory consolidation");
    if let Err(e) = run_memory_consolidation().await {
        eprintln!("[DAEMON] Memory consolidation failed: {}", e);
    }

    // Task 2: System health check
    println!("[DAEMON] - System health check");
    if let Err(e) = check_system_health().await {
        eprintln!("[DAEMON] System health check failed: {}", e);
    }

    // Task 3: Scan for TODOs/FIXMEs
    println!("[DAEMON] - Scanning for TODOs/FIXMEs");
    if let Err(e) = scan_todos().await {
        eprintln!("[DAEMON] TODO scan failed: {}", e);
    }

    // Task 4: Check for dependency updates
    println!("[DAEMON] - Checking dependency updates");
    if let Err(e) = check_dependencies().await {
        eprintln!("[DAEMON] Dependency check failed: {}", e);
    }

    // Task 5: Build error detection
    println!("[DAEMON] - Build error detection");
    if let Err(e) = check_build_errors().await {
        eprintln!("[DAEMON] Build check failed: {}", e);
    }

    println!("[DAEMON] Proactive tasks completed");
    Ok(())
}

async fn run_memory_consolidation() -> Result<()> {
    use crate::memory::MemorySystem;

    let memory_path = std::env::var("HOME")
        .map(|h| PathBuf::from(h).join(".config/nexus/memory"))
        .unwrap_or_else(|_| PathBuf::from("~/.config/nexus/memory"));

    let mut mem = MemorySystem::new(memory_path)?;
    mem.consolidate().await?;
    Ok(())
}

async fn check_system_health() -> Result<()> {
    // Check disk space
    // Check memory usage
    // Check if config files are valid
    // For now, just a placeholder
    Ok(())
}

async fn scan_todos() -> Result<()> {
    // Scan project for TODO, FIXME, XXX, HACK comments
    // Could integrate with memory system to track them
    // For now, just a placeholder
    Ok(())
}

async fn check_dependencies() -> Result<()> {
    // Check Cargo.toml for outdated dependencies (cargo outdated)
    // Check package.json if exists (npm outdated)
    // For now, just a placeholder
    Ok(())
}

async fn check_build_errors() -> Result<()> {
    // Try to compile project in check mode
    // Report any errors found
    // For now, just a placeholder
    Ok(())
}

fn is_process_running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::Pid;
        kill(Pid::from_raw(pid as i32), Signal::SIGHUP).is_ok()
            || kill(Pid::from_raw(pid as i32), None).is_ok()
    }

    #[cfg(not(unix))]
    {
        // On Windows, check if process exists
        Command::new("tasklist")
            .args(&["/FI", &format!("PID eq {}", pid)])
            .output()
            .map(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .contains(&pid.to_string())
            })
            .unwrap_or(false)
    }
}

fn format_timestamp(timestamp: u64) -> String {
    use chrono::{DateTime, Utc};
    let dt = DateTime::<Utc>::from(UNIX_EPOCH + Duration::from_secs(timestamp));
    dt.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}
