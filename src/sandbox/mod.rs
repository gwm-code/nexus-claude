pub mod docker;
pub mod hydration;
pub mod validator;

use crate::error::{NexusError, Result};
use std::process::Command;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum CommandType {
    FileSystem { operation: FileOp, path: PathBuf },
    Shell { command: String, args: Vec<String> },
    Package { manager: PackageManager, action: PackageAction, packages: Vec<String> },
}

#[derive(Debug, Clone)]
pub enum FileOp {
    Read,
    Write,
    Delete,
    Move { to: PathBuf },
    Copy { to: PathBuf },
}

#[derive(Debug, Clone)]
pub enum PackageManager {
    Npm,
    Yarn,
    Pip,
    Cargo,
    Apt,
    Brew,
}

#[derive(Debug, Clone)]
pub enum PackageAction {
    Install,
    Update,
    Remove,
}

pub struct CommandInterceptor;

impl CommandInterceptor {
    pub fn new() -> Self {
        Self
    }

    pub fn intercept(&self, command: &str) -> Result<CommandType> {
        let parts: Vec<&str> = command.split_whitespace().collect();
        if parts.is_empty() {
            return Err(NexusError::Configuration("Empty command".to_string()));
        }

        let cmd = parts[0];
        let args: Vec<String> = parts[1..].iter().map(|s| s.to_string()).collect();

        // Package manager commands
        let command_type = match cmd {
            "npm" => self.parse_npm_command(&args)?,
            "yarn" => self.parse_yarn_command(&args)?,
            "pip" => self.parse_pip_command(&args)?,
            "cargo" => self.parse_cargo_command(&args)?,
            "rm" => self.parse_rm_command(&args)?,
            "mv" => self.parse_mv_command(&args)?,
            "cp" => self.parse_cp_command(&args)?,
            _ => CommandType::Shell {
                command: cmd.to_string(),
                args,
            },
        };

        Ok(command_type)
    }

    fn parse_npm_command(&self, args: &[String]) -> Result<CommandType> {
        let action = if args.contains(&"install".to_string()) || args.contains(&"i".to_string()) {
            PackageAction::Install
        } else if args.contains(&"update".to_string()) || args.contains(&"up".to_string()) {
            PackageAction::Update
        } else if args.contains(&"uninstall".to_string()) || args.contains(&"remove".to_string()) || args.contains(&"rm".to_string()) {
            PackageAction::Remove
        } else {
            return Ok(CommandType::Shell {
                command: "npm".to_string(),
                args: args.to_vec(),
            });
        };

        let packages: Vec<String> = args.iter()
            .filter(|a| !a.starts_with('-') && !["install", "i", "update", "up", "uninstall", "remove", "rm"].contains(&a.as_str()))
            .cloned()
            .collect();

        Ok(CommandType::Package {
            manager: PackageManager::Npm,
            action,
            packages,
        })
    }

    fn parse_yarn_command(&self, args: &[String]) -> Result<CommandType> {
        let action = if args.contains(&"add".to_string()) {
            PackageAction::Install
        } else if args.contains(&"upgrade".to_string()) || args.contains(&"up".to_string()) {
            PackageAction::Update
        } else if args.contains(&"remove".to_string()) || args.contains(&"rm".to_string()) {
            PackageAction::Remove
        } else {
            return Ok(CommandType::Shell {
                command: "yarn".to_string(),
                args: args.to_vec(),
            });
        };

        let packages: Vec<String> = args.iter()
            .filter(|a| !a.starts_with('-') && !["add", "upgrade", "up", "remove", "rm"].contains(&a.as_str()))
            .cloned()
            .collect();

        Ok(CommandType::Package {
            manager: PackageManager::Yarn,
            action,
            packages,
        })
    }

    fn parse_pip_command(&self, args: &[String]) -> Result<CommandType> {
        let action = if args.contains(&"install".to_string()) {
            PackageAction::Install
        } else if args.contains(&"uninstall".to_string()) {
            PackageAction::Remove
        } else {
            return Ok(CommandType::Shell {
                command: "pip".to_string(),
                args: args.to_vec(),
            });
        };

        let packages: Vec<String> = args.iter()
            .filter(|a| !a.starts_with('-') && !["install", "uninstall"].contains(&a.as_str()))
            .cloned()
            .collect();

        Ok(CommandType::Package {
            manager: PackageManager::Pip,
            action,
            packages,
        })
    }

    fn parse_cargo_command(&self, args: &[String]) -> Result<CommandType> {
        let action = if args.contains(&"add".to_string()) {
            PackageAction::Install
        } else if args.contains(&"update".to_string()) || args.contains(&"up".to_string()) {
            PackageAction::Update
        } else if args.contains(&"remove".to_string()) || args.contains(&"rm".to_string()) {
            PackageAction::Remove
        } else {
            return Ok(CommandType::Shell {
                command: "cargo".to_string(),
                args: args.to_vec(),
            });
        };

        let packages: Vec<String> = args.iter()
            .filter(|a| !a.starts_with('-') && !["add", "update", "up", "remove", "rm"].contains(&a.as_str()))
            .cloned()
            .collect();

        Ok(CommandType::Package {
            manager: PackageManager::Cargo,
            action,
            packages,
        })
    }

    fn parse_rm_command(&self, args: &[String]) -> Result<CommandType> {
        // Extract paths and flags from rm command
        let mut paths = Vec::new();
        let mut flags = Vec::new();

        for arg in args {
            if arg.starts_with('-') {
                flags.push(arg.clone());
            } else {
                paths.push(PathBuf::from(arg));
            }
        }

        if paths.is_empty() {
            return Ok(CommandType::Shell {
                command: "rm".to_string(),
                args: args.to_vec(),
            });
        }

        // For now, handle the first path
        Ok(CommandType::FileSystem {
            operation: FileOp::Delete,
            path: paths[0].clone(),
        })
    }

    fn parse_mv_command(&self, args: &[String]) -> Result<CommandType> {
        if args.len() < 2 {
            return Ok(CommandType::Shell {
                command: "mv".to_string(),
                args: args.to_vec(),
            });
        }

        let from = PathBuf::from(&args[args.len() - 2]);
        let to = PathBuf::from(&args[args.len() - 1]);

        Ok(CommandType::FileSystem {
            operation: FileOp::Move { to },
            path: from,
        })
    }

    fn parse_cp_command(&self, args: &[String]) -> Result<CommandType> {
        if args.len() < 2 {
            return Ok(CommandType::Shell {
                command: "cp".to_string(),
                args: args.to_vec(),
            });
        }

        let from = PathBuf::from(&args[args.len() - 2]);
        let to = PathBuf::from(&args[args.len() - 1]);

        Ok(CommandType::FileSystem {
            operation: FileOp::Copy { to },
            path: from,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ShadowRunResult {
    pub success: bool,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
}

pub struct SandboxManager {
    docker: docker::DockerSandbox,
    interceptor: CommandInterceptor,
    validator: validator::Validator,
}

impl SandboxManager {
    pub fn new() -> Self {
        Self {
            docker: docker::DockerSandbox::new(),
            interceptor: CommandInterceptor::new(),
            validator: validator::Validator::new(),
        }
    }

    pub async fn shadow_run(&self, command: &str, working_dir: &std::path::Path) -> Result<ShadowRunResult> {
        // Intercept and classify the command
        let _cmd_type = self.interceptor.intercept(command)?;
        
        // Run in Docker sandbox (no network)
        let result = self.docker.execute(command, working_dir).await?;
        
        // Validate the result
        let validation = self.validator.validate(&result);
        
        Ok(ShadowRunResult {
            success: result.exit_code == 0 && validation.passed,
            exit_code: result.exit_code,
            stdout: result.stdout,
            stderr: result.stderr,
            duration_ms: result.duration_ms,
        })
    }
    
    pub async fn shadow_run_with_network(&self, command: &str, working_dir: &std::path::Path) -> Result<ShadowRunResult> {
        // Run in Docker sandbox with network enabled (for package managers)
        let result = self.docker.execute_with_network(command, working_dir).await?;
        
        // Validate the result
        let validation = self.validator.validate(&result);
        
        Ok(ShadowRunResult {
            success: result.exit_code == 0 && validation.passed,
            exit_code: result.exit_code,
            stdout: result.stdout,
            stderr: result.stderr,
            duration_ms: result.duration_ms,
        })
    }
}
