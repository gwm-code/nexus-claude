mod agent;
mod config;
mod context;
mod error;
mod executor;
mod memory;
mod mcp;
mod providers;
mod sandbox;
mod swarm;
mod watcher;

use anyhow::Result;
use clap::{Parser, Subcommand};
use config::{ConfigManager, ProviderConfig, ProviderType};
use dialoguer::{Confirm, Input, Select};
use executor::tools::create_tool_system_prompt;
use memory::MemorySystem;
use sandbox::SandboxManager;
use providers::{create_provider, list_available_providers, Message, Role, create_provider_arc};
use crate::mcp::get_builtin_server_configs;
use swarm::SwarmOrchestrator;
use std::env;
use std::io::{self, BufRead, Write};
use std::sync::Arc;

// ============================================================================
// CLI Argument Parsing
// ============================================================================

#[derive(Parser)]
#[command(name = "nexus", version, about = "Nexus - AI CLI Assistant")]
struct Cli {
    /// Output JSON instead of human-readable text
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Send a chat message (non-interactive)
    Chat {
        /// The message to send
        message: String,
    },
    /// Scan the current project repository
    Scan {
        /// Path to scan (defaults to current directory)
        path: Option<String>,
    },
    /// Show cache/project status
    Status,
    /// Show memory statistics
    MemoryStats,
    /// Initialize memory system for current project
    MemoryInit,
    /// Run memory consolidation
    MemoryConsolidate,
    /// Show system info (version, platform, etc.)
    Info,
    /// Show watcher status
    WatcherStatus,
    /// List configured providers
    Providers,
}

/// JSON envelope for non-interactive output
fn json_output(success: bool, data: serde_json::Value, error: Option<&str>) -> String {
    serde_json::json!({
        "success": success,
        "data": data,
        "error": error,
    }).to_string()
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // If a subcommand was provided, run non-interactively
    if let Some(command) = cli.command {
        return run_command(command, cli.json).await;
    }

    // No subcommand → fall through to existing REPL
    run_repl().await
}

// ============================================================================
// Non-Interactive Command Runner
// ============================================================================

async fn run_command(command: Commands, json_mode: bool) -> Result<()> {
    match command {
        Commands::Info => {
            let version = env!("CARGO_PKG_VERSION");
            let platform = std::env::consts::OS;
            if json_mode {
                println!("{}", json_output(true, serde_json::json!({
                    "version": version,
                    "platform": platform,
                    "name": "nexus",
                }), None));
            } else {
                println!("Nexus v{}", version);
                println!("Platform: {}", platform);
            }
        }
        Commands::Scan { path } => {
            let scan_path = match path {
                Some(p) => std::path::PathBuf::from(p),
                None => std::env::current_dir()?,
            };
            let mut context_manager = context::ContextManager::new(scan_path);
            match context_manager.warm_handshake().await {
                Ok(result) => {
                    if json_mode {
                        println!("{}", json_output(true, serde_json::json!({
                            "files_scanned": result.files_scanned,
                            "total_size": result.total_size,
                            "duration_ms": result.duration.as_millis(),
                        }), None));
                    } else {
                        println!("Repository scanned: {} files, {} MB",
                            result.files_scanned,
                            result.total_size / 1_000_000);
                    }
                }
                Err(e) => {
                    if json_mode {
                        println!("{}", json_output(false, serde_json::Value::Null, Some(&e.to_string())));
                    } else {
                        eprintln!("Error scanning repository: {}", e);
                    }
                    std::process::exit(1);
                }
            }
        }
        Commands::Status => {
            let context_manager = context::ContextManager::new(std::env::current_dir()?);
            let stats = context_manager.get_stats();
            if json_mode {
                println!("{}", json_output(true, serde_json::json!({
                    "total_files": stats.total_files,
                    "total_size": stats.total_size,
                    "last_sync": stats.last_sync.map(|t| format!("{:?}", t)),
                }), None));
            } else {
                println!("Cache status:");
                println!("  Files: {}", stats.total_files);
                println!("  Size: {} MB", stats.total_size / 1_000_000);
                if let Some(last_sync) = stats.last_sync {
                    println!("  Last sync: {:?}", last_sync);
                } else {
                    println!("  Last sync: Never");
                }
            }
        }
        Commands::MemoryStats => {
            let working_dir = std::env::current_dir()?;
            let project_name = working_dir.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");
            let memory_path = std::env::var("HOME")
                .map(|h| std::path::PathBuf::from(h).join(".config/nexus/memory"))
                .unwrap_or_else(|_| std::path::PathBuf::from("~/.config/nexus/memory"));

            match MemorySystem::new(memory_path) {
                Ok(mem) => {
                    let stats = mem.get_stats();
                    if json_mode {
                        println!("{}", json_output(true, serde_json::json!({
                            "project": project_name,
                            "total_memories": stats.total_memories,
                            "events_count": stats.events_count,
                            "graph_entities": stats.graph_entities,
                            "vector_documents": stats.vector_documents,
                            "size_bytes": stats.size_bytes,
                        }), None));
                    } else {
                        println!("Memory statistics for '{}'", project_name);
                        println!("  Total memories: {}", stats.total_memories);
                        println!("  Size on disk: {} bytes", stats.size_bytes);
                        println!("  Last updated: {:?}", stats.last_updated);
                    }
                }
                Err(e) => {
                    if json_mode {
                        println!("{}", json_output(false, serde_json::Value::Null, Some(&e.to_string())));
                    } else {
                        eprintln!("Failed to get memory stats: {}", e);
                        eprintln!("  Run 'nexus memory-init' first.");
                    }
                    std::process::exit(1);
                }
            }
        }
        Commands::MemoryInit => {
            let working_dir = std::env::current_dir()?;
            let project_name = working_dir.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");
            let memory_path = std::env::var("HOME")
                .map(|h| std::path::PathBuf::from(h).join(".config/nexus/memory"))
                .unwrap_or_else(|_| std::path::PathBuf::from("~/.config/nexus/memory"));

            match MemorySystem::new(memory_path.clone()) {
                Ok(mem) => {
                    match mem.init_project(&working_dir, project_name).await {
                        Ok(_) => {
                            if json_mode {
                                println!("{}", json_output(true, serde_json::json!({
                                    "project": project_name,
                                    "memory_path": memory_path.join(project_name).to_string_lossy(),
                                }), None));
                            } else {
                                println!("Memory system initialized for project '{}'", project_name);
                                println!("  Memory path: {:?}", memory_path.join(project_name));
                            }
                        }
                        Err(e) => {
                            if json_mode {
                                println!("{}", json_output(false, serde_json::Value::Null, Some(&e.to_string())));
                            } else {
                                eprintln!("Failed to initialize project in memory system: {}", e);
                            }
                            std::process::exit(1);
                        }
                    }
                }
                Err(e) => {
                    if json_mode {
                        println!("{}", json_output(false, serde_json::Value::Null, Some(&e.to_string())));
                    } else {
                        eprintln!("Failed to initialize memory system: {}", e);
                    }
                    std::process::exit(1);
                }
            }
        }
        Commands::MemoryConsolidate => {
            let memory_path = std::env::var("HOME")
                .map(|h| std::path::PathBuf::from(h).join(".config/nexus/memory"))
                .unwrap_or_else(|_| std::path::PathBuf::from("~/.config/nexus/memory"));

            match MemorySystem::new(memory_path) {
                Ok(mut mem) => {
                    match mem.consolidate().await {
                        Ok(_) => {
                            if json_mode {
                                println!("{}", json_output(true, serde_json::json!({
                                    "status": "completed",
                                }), None));
                            } else {
                                println!("Memory consolidation completed successfully");
                            }
                        }
                        Err(e) => {
                            if json_mode {
                                println!("{}", json_output(false, serde_json::Value::Null, Some(&e.to_string())));
                            } else {
                                eprintln!("Memory consolidation failed: {}", e);
                            }
                            std::process::exit(1);
                        }
                    }
                }
                Err(e) => {
                    if json_mode {
                        println!("{}", json_output(false, serde_json::Value::Null, Some(&e.to_string())));
                    } else {
                        eprintln!("Failed to initialize memory system: {}", e);
                    }
                    std::process::exit(1);
                }
            }
        }
        Commands::WatcherStatus => {
            // Non-interactive watcher status: watcher is not running in this mode
            if json_mode {
                println!("{}", json_output(true, serde_json::json!({
                    "is_running": false,
                    "watched_projects": 0,
                    "active_log_sources": 0,
                    "errors_detected": 0,
                    "errors_fixed": 0,
                    "healing_sessions_total": 0,
                    "healing_sessions_active": 0,
                }), None));
            } else {
                println!("Watcher Status:");
                println!("  Running: false (watcher runs in interactive mode)");
            }
        }
        Commands::Providers => {
            let config_manager = ConfigManager::new()?;
            let providers = config_manager.list_providers();
            let default = config_manager.get().default_provider.clone();

            if json_mode {
                let provider_list: Vec<serde_json::Value> = providers.iter().map(|p| {
                    serde_json::json!({
                        "name": p,
                        "is_default": default.as_ref() == Some(p),
                    })
                }).collect();
                println!("{}", json_output(true, serde_json::json!({
                    "providers": provider_list,
                }), None));
            } else {
                println!("Configured providers:");
                for provider in providers {
                    let is_default = default.as_ref() == Some(provider);
                    if is_default {
                        println!("  * {} (default)", provider);
                    } else {
                        println!("    {}", provider);
                    }
                }
            }
        }
        Commands::Chat { message } => {
            // Non-interactive chat requires a configured provider
            let config_manager = ConfigManager::new()?;
            let provider_name = config_manager.get().default_provider.clone()
                .ok_or_else(|| anyhow::anyhow!("No default provider configured. Run 'nexus' interactively first to set up."))?;

            let provider_config = config_manager.get_provider(&provider_name)
                .ok_or_else(|| anyhow::anyhow!("Provider not found: {}", provider_name))?
                .clone();

            let mut provider = create_provider(&provider_config.provider_type, &provider_config)?;

            if !provider.is_authenticated() {
                provider.authenticate().await?;
            }

            let model = provider_config.default_model
                .unwrap_or_else(|| provider.info().default_model.clone());

            let mut messages = vec![
                Message {
                    role: Role::System,
                    content: create_tool_system_prompt(),
                    name: None,
                },
                Message {
                    role: Role::User,
                    content: message.clone(),
                    name: None,
                },
            ];

            let agent = agent::Agent::new(std::env::current_dir()?)?;
            match agent.run_task(&mut messages, &*provider, model).await {
                Ok(response) => {
                    if json_mode {
                        println!("{}", json_output(true, serde_json::json!({
                            "response": response,
                        }), None));
                    } else {
                        println!("{}", response);
                    }
                }
                Err(e) => {
                    if json_mode {
                        println!("{}", json_output(false, serde_json::Value::Null, Some(&e.to_string())));
                    } else {
                        eprintln!("Error: {}", e);
                    }
                    std::process::exit(1);
                }
            }
        }
    }

    Ok(())
}

// ============================================================================
// Interactive REPL (existing functionality, unchanged)
// ============================================================================

async fn run_repl() -> Result<()> {
    let mut config_manager = ConfigManager::new()?;

    // Check if we have a default provider configured
    let default_provider = config_manager.get().default_provider.clone();

    if default_provider.is_none() || config_manager.get().providers.is_empty() {
        println!("Welcome to Nexus! Let's set up your first provider.");
        setup_wizard(&mut config_manager).await?;
    }

    // Let user select a provider
    let provider_name = select_provider(&config_manager)?;
    let provider_config = config_manager.get_provider(&provider_name)
        .ok_or_else(|| anyhow::anyhow!("Provider not found: {}", provider_name))?
        .clone();

    let mut provider = create_provider(&provider_config.provider_type, &provider_config)?;

    // Authenticate if needed
    if !provider.is_authenticated() {
        println!("Authenticating with {}...", provider.info().display_name);
        provider.authenticate().await?;
    }

    println!("\nConnected to {}", provider.info().display_name);
    println!("Type /help for commands, /exit to quit\n");

    // Initialize memory system for watcher
    let memory_path = std::env::var("HOME")
        .map(|h| std::path::PathBuf::from(h).join(".config/nexus/memory"))
        .unwrap_or_else(|_| std::path::PathBuf::from("~/.config/nexus/memory"));

    let memory = Arc::new(tokio::sync::RwLock::new(
        MemorySystem::new(memory_path)?
    ));

    // Create provider arc for watcher
    let provider_arc = create_provider_arc(
        &provider_config.provider_type,
        &provider_config,
    )?;

    let model = config_manager.get()
        .providers.get(&provider_name)
        .and_then(|p| p.default_model.clone())
        .unwrap_or_else(|| provider.info().default_model.clone());

    // Initialize watcher engine (will be started/stopped via commands)
    let mut watcher_engine: Option<watcher::WatcherEngine> = None;

    // Initialize MCP integration
    let mut mcp_integration = mcp::McpIntegration::new()?;

    // REPL loop
    let stdin = io::stdin();
    let mut messages: Vec<Message> = vec![
        Message {
            role: Role::System,
            content: create_tool_system_prompt(),
            name: None,
        }
    ];

    loop {
        print!("nexus> ");
        io::stdout().flush()?;

        let mut input = String::new();
        stdin.lock().read_line(&mut input)?;
        let input = input.trim();

        match input {
            "/exit" | "/quit" => {
                println!("Goodbye!");
                break;
            }
            "/help" => {
                print_help();
                continue;
            }
            "/providers" => {
                list_configured_providers(&config_manager);
                continue;
            }
            "/models" => {
                let info = provider.info();
                let current_model = config_manager.get()
                    .providers.get(&provider_name)
                    .and_then(|p| p.default_model.clone());

                // Build list with current model marked
                let items: Vec<String> = info.available_models.iter().map(|model| {
                    if Some(model.clone()) == current_model {
                        format!("{} [CURRENT]", model)
                    } else {
                        model.clone()
                    }
                }).collect();

                // Find default selection
                let default_selection = current_model
                    .and_then(|cm| info.available_models.iter().position(|m| m == &cm))
                    .unwrap_or(0);

                let selection = Select::new()
                    .with_prompt("Select a model (use arrow keys, Enter to confirm)")
                    .items(&items)
                    .default(default_selection)
                    .interact()?;

                let selected_model = &info.available_models[selection];
                set_model(&mut config_manager, selected_model).await?;
                continue;
            }
            "/sandbox" | "/shadow" => {
                run_sandbox_command().await?;
                continue;
            }
            "/sandbox status" => {
                check_sandbox_status();
                continue;
            }
            "/config" => {
                show_config(&config_manager);
                continue;
            }
            "/edit" => {
                edit_provider(&mut config_manager).await?;
                continue;
            }
            cmd if cmd.starts_with("/model ") => {
                let model_name = cmd.trim_start_matches("/model ").trim();
                if model_name.is_empty() {
                    println!("Usage: /model <name>");
                    println!("Example: /model kimi-k2.5-free");
                } else {
                    set_model(&mut config_manager, model_name).await?;
                }
                continue;
            }
            "/current" | "/model" => {
                show_current_model(&config_manager);
                continue;
            }
            "/scan" => {
                let mut context_manager = context::ContextManager::new(std::env::current_dir()?);
                match context_manager.warm_handshake().await {
                    Ok(result) => {
                        println!("✓ Repository scanned: {} files, {} MB",
                            result.files_scanned,
                            result.total_size / 1_000_000);
                    }
                    Err(e) => {
                        eprintln!("Error scanning repository: {}", e);
                    }
                }
                continue;
            }
            "/status" => {
                let context_manager = context::ContextManager::new(std::env::current_dir()?);
                let stats = context_manager.get_stats();
                println!("Cache status:");
                println!("  Files: {}", stats.total_files);
                println!("  Size: {} MB", stats.total_size / 1_000_000);
                if let Some(last_sync) = stats.last_sync {
                    println!("  Last sync: {:?}", last_sync);
                } else {
                    println!("  Last sync: Never");
                }
                continue;
            }
            "/memory init" => {
                let working_dir = std::env::current_dir()?;
                let project_name = working_dir.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown");
                let memory_path = std::env::var("HOME")
                    .map(|h| std::path::PathBuf::from(h).join(".config/nexus/memory"))
                    .unwrap_or_else(|_| std::path::PathBuf::from("~/.config/nexus/memory"));

                match MemorySystem::new(memory_path.clone()) {
                    Ok(memory) => {
                        if let Err(e) = memory.init_project(&working_dir, project_name).await {
                            eprintln!("✗ Failed to initialize project in memory system: {}", e);
                        } else {
                            println!("✓ Memory system initialized for project '{}'", project_name);
                            println!("  Memory path: {:?}", memory_path.join(project_name));
                        }
                    }
                    Err(e) => {
                        eprintln!("✗ Failed to initialize memory system: {}", e);
                    }
                }
                continue;
            }
            "/memory stats" => {
                let working_dir = std::env::current_dir()?;
                let project_name = working_dir.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown");
                let memory_path = std::env::var("HOME")
                    .map(|h| std::path::PathBuf::from(h).join(".config/nexus/memory"))
                    .unwrap_or_else(|_| std::path::PathBuf::from("~/.config/nexus/memory"));

                match MemorySystem::new(memory_path) {
                    Ok(memory) => {
                        let stats = memory.get_stats();
                        println!("Memory statistics for '{}'", project_name);
                        println!("  Total memories: {}", stats.total_memories);
                        println!("  Size on disk: {} bytes", stats.size_bytes);
                        println!("  Last updated: {:?}", stats.last_updated);
                    }
                    Err(e) => {
                        eprintln!("✗ Failed to get memory stats: {}", e);
                        eprintln!("  Run '/memory init' first to initialize the memory system.");
                    }
                }
                continue;
            }
            "/memory consolidate" => {
                let working_dir = std::env::current_dir()?;
                let project_name = working_dir.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown");
                let memory_path = std::env::var("HOME")
                    .map(|h| std::path::PathBuf::from(h).join(".config/nexus/memory"))
                    .unwrap_or_else(|_| std::path::PathBuf::from("~/.config/nexus/memory"));

                match MemorySystem::new(memory_path) {
                    Ok(mut memory) => {
                        println!("Running memory consolidation for '{}'...", project_name);
                        match memory.consolidate().await {
                            Ok(_) => {
                                println!("✓ Memory consolidation completed successfully");
                            }
                            Err(e) => {
                                eprintln!("✗ Memory consolidation failed: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("✗ Failed to initialize memory system: {}", e);
                        eprintln!("  Run '/memory init' first to initialize the memory system.");
                    }
                }
                continue;
            }

            "/swarm" | "/parallel" => {
                // Check if we have a complex task to decompose
                let working_dir = std::env::current_dir()?;
                let task = if input == "/swarm" || input == "/parallel" {
                    dialoguer::Input::new()
                        .with_prompt("Enter complex task for swarm execution")
                        .interact_text()?
                } else {
                    input.to_string()
                };

                println!("\n[SWARM] Initializing parallel agent execution...");
                println!("Task: {}", task);

                // Get the model for swarm execution
                let info = provider.info();
                let model = config_manager.get()
                    .providers.get(&provider_name)
                    .and_then(|p| p.default_model.clone())
                    .unwrap_or(info.default_model.clone());

                // Create swarm config with default settings
                let swarm_config = swarm::SwarmConfig::default();

                // Create orchestrator with Arc-wrapped provider for swarm
                let provider_config = config_manager.get_provider(&provider_name)
                    .ok_or_else(|| anyhow::anyhow!("Provider not found: {}", provider_name))?
                    .clone();
                let provider_arc = providers::create_provider_arc(
                    &provider_config.provider_type,
                    &provider_config,
                )?;
                let orchestrator = swarm::SwarmOrchestrator::new(
                    swarm_config,
                    provider_arc,
                    model.clone(),
                )?;

                // Create swarm task
                let swarm_task = swarm::SwarmTask::new(&task, &working_dir);

                // Run the swarm
                match orchestrator.execute(swarm_task).await {
                    Ok(result) => {
                        println!("\n✓ Swarm execution completed!");
                        let completed = result.subtask_results.iter().filter(|r| r.success).count();
                        let failed = result.subtask_results.iter().filter(|r| !r.success).count();
                        println!("Results: {} subtasks completed", completed);
                        if failed > 0 {
                            println!("Failed: {} subtasks", failed);
                        }
                        if !result.conflicts.is_empty() {
                            println!("⚠ Conflicts detected during merge");
                            println!("Review needed: {} files", result.conflicts.len());
                        } else {
                            println!("✓ All changes merged successfully");
                        }
                    }
                    Err(e) => {
                        eprintln!("\n✗ Swarm execution failed: {}", e);
                    }
                }
                continue;
            }

            "/watch start" => {
                if watcher_engine.is_none() {
                    match watcher::WatcherEngine::new(
                        watcher::WatcherEngineConfig::default(),
                        memory.clone(),
                        provider_arc.clone(),
                        model.clone(),
                    ).await {
                        Ok(mut engine) => {
                            if let Err(e) = engine.start().await {
                                eprintln!("✗ Failed to start watcher: {}", e);
                            } else {
                                watcher_engine = Some(engine);
                                println!("✓ Self-healing watcher started");
                                println!("  Watching for file changes and errors...");
                            }
                        }
                        Err(e) => {
                            eprintln!("✗ Failed to create watcher engine: {}", e);
                        }
                    }
                } else {
                    println!("Watcher is already running");
                }
                continue;
            }

            "/watch stop" => {
                if let Some(mut engine) = watcher_engine.take() {
                    if let Err(e) = engine.stop().await {
                        eprintln!("✗ Error stopping watcher: {}", e);
                    } else {
                        println!("✓ Watcher stopped");
                    }
                } else {
                    println!("Watcher is not running");
                }
                continue;
            }

            "/watch status" => {
                if let Some(ref engine) = watcher_engine {
                    let status = engine.get_status().await;
                    println!("\nWatcher Status:");
                    println!("  Running: {}", status.is_running);
                    println!("  Projects watched: {}", status.watched_projects);
                    println!("  Active log sources: {}", status.active_log_sources);
                    println!("  Errors detected: {}", status.errors_detected);
                    println!("  Errors fixed: {}", status.errors_fixed);
                    println!("  Healing sessions: {} total, {} active",
                        status.healing_sessions_total, status.healing_sessions_active);
                    if let Some(start_time) = status.start_time {
                        let duration = chrono::Utc::now() - start_time;
                        println!("  Uptime: {} minutes", duration.num_minutes());
                    }
                } else {
                    println!("Watcher is not running");
                }
                continue;
            }

            "/watch add" => {
                let working_dir = std::env::current_dir()?;
                let project_name = working_dir.file_name()
                    .and_then(|n| n.to_str())
                    .map(|s| s.to_string());

                if let Some(ref mut engine) = watcher_engine {
                    match engine.add_project(working_dir, project_name).await {
                        Ok(project_id) => {
                            println!("✓ Project added to watcher: {}", project_id);
                        }
                        Err(e) => {
                            eprintln!("✗ Failed to add project: {}", e);
                        }
                    }
                } else {
                    println!("Watcher is not running. Start it first with /watch start");
                }
                continue;
            }

            cmd if cmd.starts_with("/heal ") => {
                let error_desc = cmd.trim_start_matches("/heal ").trim();
                if error_desc.is_empty() {
                    println!("Usage: /heal <error description>");
                    println!("Example: /heal build failed due to missing dependency");
                } else if let Some(ref engine) = watcher_engine {
                    match engine.manual_heal(error_desc.to_string(), None).await {
                        Ok(session_id) => {
                            println!("✓ Healing session started: {}", session_id);
                        }
                        Err(e) => {
                            eprintln!("✗ Failed to start healing: {}", e);
                        }
                    }
                } else {
                    println!("Watcher is not running. Start it first with /watch start");
                }
                continue;
            }

            // MCP commands
            cmd if cmd.starts_with("/mcp connect ") => {
                let server_name = cmd.trim_start_matches("/mcp connect ").trim();
                if server_name.is_empty() {
                    println!("Usage: /mcp connect <name>");
                    println!("Available servers: sqlite, postgres, github, filesystem");
                } else {
                    let configs = get_builtin_server_configs();
                    if let Some(config) = configs.into_iter().find(|c| c.name == server_name) {
                        match mcp_integration.connect_server(&config).await {
                            Ok(_) => println!("✓ Connected to MCP server: {}", server_name),
                            Err(e) => eprintln!("✗ Failed to connect: {}", e),
                        }
                    } else {
                        eprintln!("✗ Unknown MCP server: {}", server_name);
                        println!("Available servers: sqlite, postgres, github, filesystem");
                    }
                }
                continue;
            }

            "/mcp list" => {
                let status = mcp_integration.get_status();
                if status.connected_servers.is_empty() {
                    println!("No MCP servers connected");
                } else {
                    println!("Connected MCP servers:");
                    for server in &status.connected_servers {
                        println!("  • {}", server);
                    }
                }
                continue;
            }

            "/mcp tools" => {
                match mcp_integration.list_all_tools().await {
                    Ok(tools) => {
                        if tools.is_empty() {
                            println!("No MCP tools available");
                        } else {
                            println!("Available MCP tools:");
                            for (server, tool) in tools {
                                println!("  • [{}] {} - {}", server, tool.name, tool.description);
                            }
                        }
                    }
                    Err(e) => eprintln!("✗ Failed to list tools: {}", e),
                }
                continue;
            }

            cmd if cmd.starts_with("/mcp call ") => {
                let args: Vec<&str> = cmd.trim_start_matches("/mcp call ").trim().split_whitespace().collect();
                if args.len() < 2 {
                    println!("Usage: /mcp call <server> <tool>");
                    println!("Example: /mcp call nexus list_files");
                } else {
                    let server = args[0];
                    let tool = args[1];
                    match mcp_integration.execute_tool(server, tool, serde_json::json!({})).await {
                        Ok(result) => {
                            println!("✓ Tool executed successfully:");
                            for content in &result.content {
                                if let mcp::ToolContent::Text { text } = content {
                                    println!("{}", text);
                                }
                            }
                        }
                        Err(e) => eprintln!("✗ Failed to execute tool: {}", e),
                    }
                }
                continue;
            }

            "/mcp server start" => {
                match mcp_integration.start_server(3000).await {
                    Ok(_) => println!("✓ MCP server started on port 3000"),
                    Err(e) => eprintln!("✗ Failed to start MCP server: {}", e),
                }
                continue;
            }

            "" => continue,
            _ => {}
        }

        // Add user message
        messages.push(Message {
            role: Role::User,
            content: input.to_string(),
            name: None,
        });

        // Create agent and run the task
        let agent = agent::Agent::new(std::env::current_dir()?)?;
        let info = provider.info();
        let model = config_manager.get()
            .providers.get(&provider_name)
            .and_then(|p| p.default_model.clone())
            .unwrap_or(info.default_model.clone());

        match agent.run_task(&mut messages, &*provider, model).await {
            Ok(final_response) => {
                println!("\n{}", final_response);
            }
            Err(e) => {
                eprintln!("Error: {}", e);
            }
        }
    }

    Ok(())
}

async fn setup_wizard(config_manager: &mut ConfigManager) -> Result<()> {
    let available_providers = list_available_providers();

    let items: Vec<String> = available_providers
        .iter()
        .map(|p| format!("{} - {}", p.name, p.display_name))
        .collect();

    let selection = Select::new()
        .with_prompt("Select a provider to configure")
        .items(&items)
        .default(0)
        .interact()?;

    let selected = &available_providers[selection];
    let provider_name = selected.name.clone();

    // Get provider-specific configuration
    let provider_config = if selected.supports_oauth {
        configure_oauth_provider(&selected.name).await?
    } else {
        configure_api_key_provider(&selected.name).await?
    };

    // Save provider
    config_manager.add_provider(provider_name.clone(), provider_config)?;

    // Set as default if it's the first one
    if config_manager.get().default_provider.is_none() {
        config_manager.get_mut().default_provider = Some(provider_name.clone());
        config_manager.save()?;
    }

    println!("✓ Provider '{}' configured successfully!", provider_name);

    Ok(())
}

async fn configure_api_key_provider(name: &str) -> Result<ProviderConfig> {
    let api_key: String = Input::new()
        .with_prompt(format!("Enter your {} API key", name))
        .interact_text()?;

    let provider_type = match name {
        "opencode" => ProviderType::Opencode,
        "openrouter" => ProviderType::Openrouter,
        "google" => ProviderType::Google,
        "claude" => ProviderType::Claude,
        _ => ProviderType::Opencode,
    };

    Ok(ProviderConfig {
        provider_type,
        api_key: Some(api_key),
        oauth_token: None,
        oauth_client_id: None,
        oauth_client_secret: None,
        oauth_refresh_token: None,
        base_url: None,
        default_model: None,
        timeout_secs: Some(60),
    })
}

async fn configure_oauth_provider(name: &str) -> Result<ProviderConfig> {
    println!("OAuth configuration for {} not yet fully implemented.", name);
    println!("Falling back to API key authentication.");

    configure_api_key_provider(name).await
}

fn select_provider(config_manager: &ConfigManager) -> Result<String> {
    let providers = config_manager.list_providers();

    if providers.len() == 1 {
        return Ok(providers[0].clone());
    }

    let default = config_manager.get().default_provider.clone();

    let items: Vec<String> = providers
        .iter()
        .map(|p| {
            let is_default = default.as_ref() == Some(p);
            if is_default {
                format!("{} (default)", p)
            } else {
                p.to_string()
            }
        })
        .collect();

    let selection = Select::new()
        .with_prompt("Select provider")
        .items(&items)
        .default(0)
        .interact()?;

    Ok(providers[selection].clone())
}

fn print_help() {
    println!("\nNexus CLI Commands:");
    println!("  /help       - Show this help message");
    println!("  /providers  - List configured providers");
    println!("  /models     - List available models for current provider");
    println!("  /model <name>  - Set the active model (e.g., /model kimi-k2.5-free)");
    println!("  /current    - Show current model");
    println!("  /scan       - Scan repository and cache file tree");
    println!("  /status     - Show cache status");
    println!("  /memory init  - Initialize memory system for current project");
    println!("  /memory stats - Show memory statistics");
    println!("  /memory consolidate - Run memory consolidation");
    println!("  /swarm <task> - Execute complex task with parallel agents");
    println!("  /shadow <cmd>  - Run command in sandbox (Shadow Run)");
    println!("  /sandbox status - Check sandbox availability");
    println!("  /watch start   - Start the self-healing watcher");
    println!("  /watch stop    - Stop the self-healing watcher");
    println!("  /watch status  - Check watcher status");
    println!("  /watch add     - Add current project to watcher");
    println!("  /heal <desc>   - Manually trigger healing for an error");
    println!("  /mcp connect <name>  - Connect to MCP server");
    println!("  /mcp list    - List connected MCP servers");
    println!("  /mcp tools   - List available MCP tools");
    println!("  /mcp call <server> <tool>  - Call an MCP tool");
    println!("  /mcp server start  - Start MCP server");
    println!("  /config     - View current configuration");
    println!("  /edit       - Edit a provider's API key");
    println!("  /auto on|off - Toggle automatic shadow run mode");
    println!("  /exit or /quit  - Exit the CLI\n");
}

fn list_configured_providers(config_manager: &ConfigManager) {
    let providers = config_manager.list_providers();
    let default = config_manager.get().default_provider.clone();

    println!("\nConfigured providers:");
    for provider in providers {
        let is_default = default.as_ref() == Some(provider);
        if is_default {
            println!("  * {} (default)", provider);
        } else {
            println!("    {}", provider);
        }
    }
    println!();
}

async fn run_sandbox_command() -> Result<()> {
    let command: String = Input::new()
        .with_prompt("Enter command to run in sandbox")
        .interact_text()?;

    let working_dir = std::env::current_dir()?;
    let sandbox = SandboxManager::new();

    println!("Running in sandbox: {}", command);
    let result = sandbox.shadow_run(&command, &working_dir).await?;

    println!("\nSandbox Result:");
    println!("  Success: {}", result.success);
    println!("  Exit Code: {}", result.exit_code);
    println!("  Duration: {}ms", result.duration_ms);

    if !result.stdout.is_empty() {
        println!("\n  stdout:");
        for line in result.stdout.lines() {
            println!("    {}", line);
        }
    }

    if !result.stderr.is_empty() {
        println!("\n  stderr:");
        for line in result.stderr.lines() {
            println!("    {}", line);
        }
    }

    println!();
    Ok(())
}

fn check_sandbox_status() {
    println!("\nChecking sandbox/Docker availability...");

    let docker_available = std::process::Command::new("docker")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if docker_available {
        println!("  Docker: Available");

        let version = std::process::Command::new("docker")
            .arg("--version")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|_| "Unknown".to_string());

        println!("  Version: {}", version);
        println!("  Sandbox: Ready to use");
    } else {
        println!("  Docker: Not available");
        println!("  Sandbox: Cannot run without Docker");
        println!("  Install Docker to use Shadow Run feature");
    }

    println!();
}

fn show_config(config_manager: &ConfigManager) {
    println!("\nCurrent Configuration:");

    let config_path = config_manager.get_config_path().unwrap_or_else(|_| std::path::PathBuf::from("unknown"));
    println!("  Config path: {:?}", config_path);

    if config_path.exists() {
        match std::fs::read_to_string(&config_path) {
            Ok(contents) => {
                println!("\n{}", contents);
            }
            Err(e) => {
                eprintln!("Error reading config file: {}", e);
            }
        }
    } else {
        println!("  No config file exists yet.");
    }
    println!();
}

async fn edit_provider(config_manager: &mut ConfigManager) -> Result<()> {
    let providers = config_manager.list_providers();

    if providers.is_empty() {
        println!("No providers configured. Use the setup wizard to add one.");
        return Ok(());
    }

    let provider_names: Vec<String> = providers.iter().map(|p| p.to_string()).collect();

    let selection = Select::new()
        .with_prompt("Select provider to edit")
        .items(&provider_names)
        .default(0)
        .interact()?;

    let selected_name = provider_names[selection].clone();

    let new_api_key: String = Input::new()
        .with_prompt(format!("Enter new API key for {}", selected_name))
        .interact_text()?;

    // Update the provider config
    if let Some(provider_config) = config_manager.get_mut().providers.get_mut(&selected_name) {
        provider_config.api_key = Some(new_api_key);
        config_manager.save()?;
        println!("✓ API key updated for provider '{}'", selected_name);
    }

    println!();
    Ok(())
}

async fn set_model(config_manager: &mut ConfigManager, model_name: &str) -> Result<()> {
    let provider_name = config_manager.get()
        .default_provider
        .clone()
        .ok_or_else(|| anyhow::anyhow!("No default provider configured"))?;

    // Store model name as-is (plain format without prefix)
    let full_model_name = model_name.to_string();

    if let Some(provider_config) = config_manager.get_mut().providers.get_mut(&provider_name) {
        provider_config.default_model = Some(full_model_name);
        config_manager.save()?;
        println!("Model set to: {}", model_name);
    }

    Ok(())
}

fn show_current_model(config_manager: &ConfigManager) {
    let config = config_manager.get();

    match &config.default_provider {
        Some(provider_name) => {
            let model = config.providers.get(provider_name)
                .and_then(|p| p.default_model.clone())
                .unwrap_or_else(|| "Not set".to_string());

            println!("Current provider: {}", provider_name);
            println!("Current model: {}", model);
        }
        None => {
            println!("No default provider configured. Use /providers to configure one.");
        }
    }
    println!();
}
