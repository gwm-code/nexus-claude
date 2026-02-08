mod agent;
mod config;
mod context;
mod daemon;
mod error;
mod executor;
mod hierarchy;
mod memory;
mod mcp;
mod oauth;
mod providers;
mod sandbox;
mod secret_store;
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
    /// Manage configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Manage background daemon for proactive tasks
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },
    /// Manage model hierarchy and escalation
    Hierarchy {
        #[command(subcommand)]
        action: HierarchyAction,
    },
    /// OAuth authentication flow (PKCE)
    #[command(name = "oauth")]
    OAuth {
        #[command(subcommand)]
        action: OAuthAction,
    },
}

#[derive(Subcommand)]
enum HierarchyAction {
    /// Show current hierarchy configuration
    Show,
    /// Set hierarchy from preset (balanced, budget, premium, speed, claude-only)
    SetPreset {
        preset: String,
    },
    /// Set model for a specific category and tier
    SetModel {
        /// Category: heartbeat, daily, planning, coding, review
        category: String,
        /// Tier index (0 = first tier)
        tier: usize,
        /// Model ID
        model_id: String,
    },
    /// Show escalation policy
    ShowPolicy,
    /// Update escalation policy
    UpdatePolicy {
        /// Enable/disable escalation
        #[arg(long)]
        enabled: Option<bool>,
        /// Max escalation steps
        #[arg(long)]
        max_escalations: Option<usize>,
        /// Daily budget limit in USD
        #[arg(long)]
        budget_limit: Option<f64>,
    },
}

#[derive(Subcommand)]
enum DaemonAction {
    /// Start the background daemon
    Start {
        /// Heartbeat interval in hours (0-24, 0 = disabled)
        #[arg(short, long, default_value = "24")]
        interval: u8,
    },
    /// Stop the background daemon
    Stop,
    /// Show daemon status
    Status,
    /// Run proactive tasks manually (without daemon)
    RunTasks,
}

#[derive(Subcommand)]
enum OAuthAction {
    /// Start OAuth authorization flow (opens browser)
    Authorize {
        /// Provider name (google, claude, openai)
        provider: String,
    },
    /// Get OAuth authorization URL (desktop use - doesn't block)
    GetUrl {
        /// Provider name (google, claude, openai)
        provider: String,
    },
    /// Wait for OAuth callback (desktop use - starts server and blocks)
    WaitCallback {
        /// Provider name
        provider: String,
    },
    /// Check OAuth authorization status
    Status {
        /// Provider name
        provider: String,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Get a config value
    Get {
        /// Key to get (e.g., "provider", "model", "all")
        key: String,
    },
    /// Set a config value
    Set {
        /// Key to set (e.g., "provider", "model", "base-url")
        key: String,
        /// Value to set
        value: String,
    },
    /// Set API key securely (stored in OS keyring)
    SetApiKey {
        /// Provider name
        provider: String,
        /// API key value
        key: String,
    },
    /// List available models for a provider
    ListModels {
        /// Provider name
        provider: String,
    },
    /// Test connection to a provider
    TestConnection {
        /// Provider name
        provider: String,
    },
    /// Migrate plaintext secrets to keyring
    MigrateSecrets,
    /// Set OAuth credentials (client ID and secret)
    SetOAuth {
        /// Provider name (google, claude, openai)
        provider: String,
        /// OAuth Client ID
        client_id: String,
        /// OAuth Client Secret
        client_secret: String,
    },
    /// Start OAuth authorization flow
    OAuthAuthorize {
        /// Provider name (google, claude, openai)
        provider: String,
    },
    /// Check OAuth authorization status
    OAuthStatus {
        /// Provider name (google, claude, openai)
        provider: String,
    },
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
    // Check for --json flag before initializing logging
    let json_mode = std::env::args().any(|arg| arg == "--json");

    // Initialize structured logging
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("nexus=info"));

    if json_mode {
        // In JSON mode: send logs to stderr with no ANSI colors
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_target(false)
            .with_ansi(false)
            .with_writer(std::io::stderr)
            .compact()
            .init();
    } else if std::env::var("NEXUS_LOG_JSON").is_ok() {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(env_filter)
            .with_target(true)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_target(false)
            .compact()
            .init();
    }

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
        Commands::Config { action } => {
            let mut config_manager = ConfigManager::new()?;
            match action {
                ConfigAction::Get { key } => {
                    match key.as_str() {
                        "all" => {
                            let config = config_manager.get();
                            // Build a sanitized view: mask raw API keys
                            let mut providers_view = serde_json::Map::new();
                            for (name, prov) in &config.providers {
                                let api_key_display = match &prov.api_key {
                                    Some(val) if secret_store::parse_sentinel(val).is_some() => {
                                        "****configured**** (keyring)".to_string()
                                    }
                                    Some(_) => "****configured****".to_string(),
                                    None => "not set".to_string(),
                                };
                                providers_view.insert(name.clone(), serde_json::json!({
                                    "provider_type": format!("{:?}", prov.provider_type),
                                    "api_key": api_key_display,
                                    "base_url": prov.base_url,
                                    "default_model": prov.default_model,
                                    "timeout_secs": prov.timeout_secs,
                                }));
                            }
                            let data = serde_json::json!({
                                "default_provider": config.default_provider,
                                "providers": providers_view,
                                "ui": {
                                    "show_diff_preview": config.ui.show_diff_preview,
                                    "confirm_dangerous_commands": config.ui.confirm_dangerous_commands,
                                    "command_timeout_secs": config.ui.command_timeout_secs,
                                },
                            });
                            if json_mode {
                                println!("{}", json_output(true, data, None));
                            } else {
                                println!("Current configuration:");
                                println!("  Default provider: {}", config.default_provider.as_deref().unwrap_or("not set"));
                                println!("  Providers:");
                                for (name, prov) in &config.providers {
                                    let api_key_display = match &prov.api_key {
                                        Some(val) if secret_store::parse_sentinel(val).is_some() => {
                                            "****configured**** (keyring)"
                                        }
                                        Some(_) => "****configured****",
                                        None => "not set",
                                    };
                                    println!("    {}:", name);
                                    println!("      type: {:?}", prov.provider_type);
                                    println!("      api_key: {}", api_key_display);
                                    println!("      base_url: {}", prov.base_url.as_deref().unwrap_or("default"));
                                    println!("      default_model: {}", prov.default_model.as_deref().unwrap_or("not set"));
                                    println!("      timeout_secs: {}", prov.timeout_secs.map(|t| t.to_string()).unwrap_or_else(|| "default".to_string()));
                                }
                                println!("  UI:");
                                println!("    show_diff_preview: {}", config.ui.show_diff_preview);
                                println!("    confirm_dangerous_commands: {}", config.ui.confirm_dangerous_commands);
                                println!("    command_timeout_secs: {}", config.ui.command_timeout_secs);
                            }
                        }
                        "provider" => {
                            let default = config_manager.get().default_provider.clone();
                            if json_mode {
                                println!("{}", json_output(true, serde_json::json!({
                                    "provider": default,
                                }), None));
                            } else {
                                println!("{}", default.as_deref().unwrap_or("not set"));
                            }
                        }
                        "model" => {
                            let config = config_manager.get();
                            let model = config.default_provider.as_ref()
                                .and_then(|p| config.providers.get(p))
                                .and_then(|pc| pc.default_model.clone());
                            if json_mode {
                                println!("{}", json_output(true, serde_json::json!({
                                    "model": model,
                                }), None));
                            } else {
                                println!("{}", model.as_deref().unwrap_or("not set"));
                            }
                        }
                        other => {
                            let msg = format!("Unknown config key: '{}'. Valid keys: all, provider, model", other);
                            if json_mode {
                                println!("{}", json_output(false, serde_json::Value::Null, Some(&msg)));
                            } else {
                                eprintln!("{}", msg);
                            }
                            std::process::exit(1);
                        }
                    }
                }
                ConfigAction::Set { key, value } => {
                    match key.as_str() {
                        "provider" => {
                            // Validate the provider exists in config
                            if config_manager.get_provider(&value).is_none() {
                                let msg = format!("Provider '{}' is not configured. Use 'nexus providers' to list configured providers.", value);
                                if json_mode {
                                    println!("{}", json_output(false, serde_json::Value::Null, Some(&msg)));
                                } else {
                                    eprintln!("{}", msg);
                                }
                                std::process::exit(1);
                            }
                            config_manager.get_mut().default_provider = Some(value.clone());
                            config_manager.save()?;
                            if json_mode {
                                println!("{}", json_output(true, serde_json::json!({
                                    "provider": value,
                                }), None));
                            } else {
                                println!("Default provider set to: {}", value);
                            }
                        }
                        "model" => {
                            let provider_name = config_manager.get().default_provider.clone()
                                .ok_or_else(|| anyhow::anyhow!("No default provider configured. Set one first with: nexus config set provider <name>"))?;
                            if let Some(provider_config) = config_manager.get_mut().providers.get_mut(&provider_name) {
                                provider_config.default_model = Some(value.clone());
                                config_manager.save()?;
                                if json_mode {
                                    println!("{}", json_output(true, serde_json::json!({
                                        "provider": provider_name,
                                        "model": value,
                                    }), None));
                                } else {
                                    println!("Default model for '{}' set to: {}", provider_name, value);
                                }
                            } else {
                                let msg = format!("Provider '{}' not found in configuration", provider_name);
                                if json_mode {
                                    println!("{}", json_output(false, serde_json::Value::Null, Some(&msg)));
                                } else {
                                    eprintln!("{}", msg);
                                }
                                std::process::exit(1);
                            }
                        }
                        "base-url" => {
                            let provider_name = config_manager.get().default_provider.clone()
                                .ok_or_else(|| anyhow::anyhow!("No default provider configured. Set one first with: nexus config set provider <name>"))?;
                            if let Some(provider_config) = config_manager.get_mut().providers.get_mut(&provider_name) {
                                provider_config.base_url = Some(value.clone());
                                config_manager.save()?;
                                if json_mode {
                                    println!("{}", json_output(true, serde_json::json!({
                                        "provider": provider_name,
                                        "base_url": value,
                                    }), None));
                                } else {
                                    println!("Base URL for '{}' set to: {}", provider_name, value);
                                }
                            } else {
                                let msg = format!("Provider '{}' not found in configuration", provider_name);
                                if json_mode {
                                    println!("{}", json_output(false, serde_json::Value::Null, Some(&msg)));
                                } else {
                                    eprintln!("{}", msg);
                                }
                                std::process::exit(1);
                            }
                        }
                        other => {
                            let msg = format!("Unknown config key: '{}'. Valid keys: provider, model, base-url", other);
                            if json_mode {
                                println!("{}", json_output(false, serde_json::Value::Null, Some(&msg)));
                            } else {
                                eprintln!("{}", msg);
                            }
                            std::process::exit(1);
                        }
                    }
                }
                ConfigAction::SetApiKey { provider, key } => {
                    match config_manager.set_api_key_secure(&provider, &key) {
                        Ok(()) => {
                            if json_mode {
                                println!("{}", json_output(true, serde_json::json!({
                                    "provider": provider,
                                    "status": "api_key_stored_in_keyring",
                                }), None));
                            } else {
                                println!("API key for '{}' stored securely in OS keyring.", provider);
                            }
                        }
                        Err(e) => {
                            let msg = format!("Failed to store API key: {}", e);
                            if json_mode {
                                println!("{}", json_output(false, serde_json::Value::Null, Some(&msg)));
                            } else {
                                eprintln!("{}", msg);
                            }
                            std::process::exit(1);
                        }
                    }
                }
                ConfigAction::ListModels { provider } => {
                    // Load provider config with resolved secrets
                    let provider_config = match config_manager.get_provider_resolved(&provider) {
                        Ok(Some(cfg)) => cfg,
                        Ok(None) => {
                            let msg = format!("Provider '{}' is not configured. Run: nexus config set-provider {}", provider, provider);
                            if json_mode {
                                println!("{}", json_output(false, serde_json::Value::Null, Some(&msg)));
                            } else {
                                eprintln!("{}", msg);
                            }
                            std::process::exit(1);
                        }
                        Err(e) => {
                            let msg = format!("Failed to resolve provider secrets: {}", e);
                            if json_mode {
                                println!("{}", json_output(false, serde_json::Value::Null, Some(&msg)));
                            } else {
                                eprintln!("{}", msg);
                            }
                            std::process::exit(1);
                        }
                    };

                    // Create provider instance
                    let prov = match create_provider(&provider_config.provider_type, &provider_config) {
                        Ok(p) => p,
                        Err(e) => {
                            let msg = format!("Failed to create provider: {}", e);
                            if json_mode {
                                println!("{}", json_output(false, serde_json::Value::Null, Some(&msg)));
                            } else {
                                eprintln!("{}", msg);
                            }
                            std::process::exit(1);
                        }
                    };

                    // Try to fetch models dynamically from provider API
                    let models_result = match prov.list_available_models().await {
                        Ok(models) => models,
                        Err(e) => {
                            if !json_mode {
                                eprintln!("Warning: Could not fetch models from provider API: {}", e);
                                eprintln!("Using static model list as fallback...");
                            }
                            // Fallback to static info
                            prov.info().available_models.into_iter()
                                .map(|id| providers::ModelInfo {
                                    id: id.clone(),
                                    name: id,
                                    description: None,
                                    context_length: None,
                                    pricing: None,
                                    supports_vision: false,
                                    supports_streaming: false,
                                    supports_function_calling: false,
                                })
                                .collect()
                        }
                    };

                    if json_mode {
                        println!("{}", json_output(true, serde_json::json!({
                            "provider": provider,
                            "models": models_result,
                        }), None));
                    } else {
                        println!("Available models for '{}':", provider);
                        println!();
                        for model in &models_result {
                            println!("  {} - {}", model.id, model.name);
                            if let Some(desc) = &model.description {
                                println!("    {}", desc);
                            }
                            if let Some(ctx_len) = model.context_length {
                                println!("    Context: {} tokens", ctx_len);
                            }
                            if let Some(pricing) = &model.pricing {
                                if let (Some(p), Some(c)) = (pricing.prompt, pricing.completion) {
                                    println!("    Pricing: ${:.2}/M input, ${:.2}/M output", p, c);
                                }
                            }
                            println!();
                        }
                    }
                }
                ConfigAction::TestConnection { provider } => {
                    // Load the provider config with resolved secrets
                    let provider_config = match config_manager.get_provider_resolved(&provider) {
                        Ok(Some(cfg)) => cfg,
                        Ok(None) => {
                            let msg = format!("Provider '{}' is not configured.", provider);
                            if json_mode {
                                println!("{}", json_output(false, serde_json::Value::Null, Some(&msg)));
                            } else {
                                eprintln!("{}", msg);
                            }
                            std::process::exit(1);
                        }
                        Err(e) => {
                            let msg = format!("Failed to resolve provider secrets: {}", e);
                            if json_mode {
                                println!("{}", json_output(false, serde_json::Value::Null, Some(&msg)));
                            } else {
                                eprintln!("{}", msg);
                            }
                            std::process::exit(1);
                        }
                    };

                    let mut prov = match create_provider(&provider_config.provider_type, &provider_config) {
                        Ok(p) => p,
                        Err(e) => {
                            let msg = format!("Failed to create provider: {}", e);
                            if json_mode {
                                println!("{}", json_output(false, serde_json::Value::Null, Some(&msg)));
                            } else {
                                eprintln!("{}", msg);
                            }
                            std::process::exit(1);
                        }
                    };

                    if !prov.is_authenticated() {
                        if let Err(e) = prov.authenticate().await {
                            let msg = format!("Authentication failed: {}", e);
                            if json_mode {
                                println!("{}", json_output(false, serde_json::Value::Null, Some(&msg)));
                            } else {
                                eprintln!("{}", msg);
                            }
                            std::process::exit(1);
                        }
                    }

                    let model = provider_config.default_model
                        .unwrap_or_else(|| prov.info().default_model.clone());

                    let request = providers::CompletionRequest {
                        model: model.clone(),
                        messages: vec![
                            Message {
                                role: Role::User,
                                content: "Say hello in one word".to_string(),
                                name: None,
                            },
                        ],
                        temperature: Some(0.0),
                        max_tokens: Some(10),
                        stream: Some(false),
                tools: None,
                        extra_params: None,
                    };

                    match prov.complete(request).await {
                        Ok(response) => {
                            if json_mode {
                                println!("{}", json_output(true, serde_json::json!({
                                    "provider": provider,
                                    "model": model,
                                    "status": "connected",
                                    "response": response.content,
                                }), None));
                            } else {
                                println!("Connection to '{}' successful!", provider);
                                println!("  Model: {}", model);
                                println!("  Response: {}", response.content);
                            }
                        }
                        Err(e) => {
                            let msg = format!("Connection test failed: {}", e);
                            if json_mode {
                                println!("{}", json_output(false, serde_json::json!({
                                    "provider": provider,
                                    "model": model,
                                    "status": "failed",
                                }), Some(&msg)));
                            } else {
                                eprintln!("Connection to '{}' failed: {}", provider, e);
                            }
                            std::process::exit(1);
                        }
                    }
                }
                ConfigAction::MigrateSecrets => {
                    match config_manager.migrate_secrets() {
                        Ok(count) => {
                            if json_mode {
                                println!("{}", json_output(true, serde_json::json!({
                                    "migrated": count,
                                }), None));
                            } else {
                                if count == 0 {
                                    println!("No plaintext secrets found to migrate.");
                                } else {
                                    println!("Migrated {} secret(s) to OS keyring.", count);
                                }
                            }
                        }
                        Err(e) => {
                            let msg = format!("Secret migration failed: {}", e);
                            if json_mode {
                                println!("{}", json_output(false, serde_json::Value::Null, Some(&msg)));
                            } else {
                                eprintln!("{}", msg);
                            }
                            std::process::exit(1);
                        }
                    }
                }
                ConfigAction::SetOAuth { provider, client_id, client_secret } => {
                    match config_manager.set_oauth_credentials(&provider, &client_id, &client_secret) {
                        Ok(()) => {
                            if json_mode {
                                println!("{}", json_output(true, serde_json::json!({
                                    "provider": provider,
                                    "status": "oauth_credentials_stored",
                                }), None));
                            } else {
                                println!("OAuth credentials for '{}' stored securely.", provider);
                            }
                        }
                        Err(e) => {
                            let msg = format!("Failed to store OAuth credentials: {}", e);
                            if json_mode {
                                println!("{}", json_output(false, serde_json::Value::Null, Some(&msg)));
                            } else {
                                eprintln!("{}", msg);
                            }
                            std::process::exit(1);
                        }
                    }
                }
                ConfigAction::OAuthAuthorize { provider } => {
                    match run_oauth_flow(&provider, &config_manager).await {
                        Ok(auth_url) => {
                            if json_mode {
                                println!("{}", json_output(true, serde_json::json!({
                                    "provider": provider,
                                    "auth_url": auth_url,
                                    "status": "authorization_complete",
                                }), None));
                            } else {
                                println!("OAuth authorization complete for '{}'!", provider);
                            }
                        }
                        Err(e) => {
                            let msg = format!("OAuth authorization failed: {}", e);
                            if json_mode {
                                println!("{}", json_output(false, serde_json::Value::Null, Some(&msg)));
                            } else {
                                eprintln!("{}", msg);
                            }
                            std::process::exit(1);
                        }
                    }
                }
                ConfigAction::OAuthStatus { provider } => {
                    match config_manager.get_oauth_status(&provider) {
                        Ok(status) => {
                            if json_mode {
                                println!("{}", json_output(true, serde_json::json!(status), None));
                            } else {
                                if status.authorized {
                                    println!("OAuth Status for '{}':", provider);
                                    println!("  Authorized: Yes");
                                    if let Some(expires) = status.expires_at {
                                        println!("  Expires: {}", expires);
                                    }
                                } else {
                                    println!("OAuth Status for '{}':", provider);
                                    println!("  Authorized: No");
                                    println!("  Run 'nexus config oauth-authorize {}' to authorize", provider);
                                }
                            }
                        }
                        Err(e) => {
                            let msg = format!("Failed to check OAuth status: {}", e);
                            if json_mode {
                                println!("{}", json_output(false, serde_json::Value::Null, Some(&msg)));
                            } else {
                                eprintln!("{}", msg);
                            }
                            std::process::exit(1);
                        }
                    }
                }
            }
        }
        Commands::Daemon { action } => {
            let daemon_manager = daemon::DaemonManager::new()?;
            match action {
                DaemonAction::Start { interval } => {
                    match daemon_manager.start(interval) {
                        Ok(()) => {
                            if json_mode {
                                println!("{}", json_output(true, serde_json::json!({
                                    "status": "started",
                                    "interval_hours": interval,
                                }), None));
                            } else {
                                println!("Daemon started with {}-hour heartbeat interval", interval);
                                println!("Proactive tasks will run every {} hours in the background", interval);
                            }
                        }
                        Err(e) => {
                            let msg = format!("Failed to start daemon: {}", e);
                            if json_mode {
                                println!("{}", json_output(false, serde_json::Value::Null, Some(&msg)));
                            } else {
                                eprintln!("{}", msg);
                            }
                            std::process::exit(1);
                        }
                    }
                }
                DaemonAction::Stop => {
                    match daemon_manager.stop() {
                        Ok(()) => {
                            if json_mode {
                                println!("{}", json_output(true, serde_json::json!({
                                    "status": "stopped",
                                }), None));
                            } else {
                                println!("Daemon stopped");
                            }
                        }
                        Err(e) => {
                            let msg = format!("Failed to stop daemon: {}", e);
                            if json_mode {
                                println!("{}", json_output(false, serde_json::Value::Null, Some(&msg)));
                            } else {
                                eprintln!("{}", msg);
                            }
                            std::process::exit(1);
                        }
                    }
                }
                DaemonAction::Status => {
                    match daemon_manager.status() {
                        Ok(status) => {
                            if json_mode {
                                println!("{}", json_output(true, serde_json::to_value(&status)?, None));
                            } else {
                                if status.running {
                                    println!("Daemon Status: Running");
                                    if let Some(pid) = status.pid {
                                        println!("  PID: {}", pid);
                                    }
                                    if let Some(interval) = status.interval_hours {
                                        println!("  Heartbeat Interval: {} hours", interval);
                                    }
                                    if let Some(last_run) = status.last_run {
                                        println!("  Last Run: {}", last_run);
                                    }
                                    if let Some(next_run) = status.next_run {
                                        println!("  Next Run: {}", next_run);
                                    }
                                } else {
                                    println!("Daemon Status: Not Running");
                                }
                            }
                        }
                        Err(e) => {
                            let msg = format!("Failed to get daemon status: {}", e);
                            if json_mode {
                                println!("{}", json_output(false, serde_json::Value::Null, Some(&msg)));
                            } else {
                                eprintln!("{}", msg);
                            }
                            std::process::exit(1);
                        }
                    }
                }
                DaemonAction::RunTasks => {
                    // Run tasks manually (blocking)
                    if json_mode {
                        println!("{}", json_output(true, serde_json::json!({
                            "status": "running_tasks",
                        }), None));
                    } else {
                        println!("Running proactive tasks...");
                    }

                    match daemon::run_proactive_tasks().await {
                        Ok(()) => {
                            if json_mode {
                                println!("{}", json_output(true, serde_json::json!({
                                    "status": "completed",
                                }), None));
                            } else {
                                println!("Tasks completed successfully");
                            }
                        }
                        Err(e) => {
                            let msg = format!("Tasks failed: {}", e);
                            if json_mode {
                                println!("{}", json_output(false, serde_json::Value::Null, Some(&msg)));
                            } else {
                                eprintln!("{}", msg);
                            }
                            std::process::exit(1);
                        }
                    }

                    // Update last run timestamp if daemon is running
                    if let Ok(status) = daemon_manager.status() {
                        if status.running {
                            let _ = daemon_manager.update_last_run();
                        }
                    }
                }
            }
        }
        Commands::Hierarchy { action } => {
            use hierarchy::{ModelHierarchy, EscalationPolicy, TaskCategory};

            let config_dir = std::env::var("HOME")
                .map(|h| std::path::PathBuf::from(h).join(".config/nexus"))
                .unwrap_or_else(|_| std::path::PathBuf::from("~/.config/nexus"));

            match action {
                HierarchyAction::Show => {
                    let hierarchy = ModelHierarchy::load(&config_dir)?;
                    if json_mode {
                        println!("{}", json_output(true, serde_json::to_value(&hierarchy)?, None));
                    } else {
                        println!("Model Hierarchy:");
                        println!("  Heartbeat: {:?}", hierarchy.heartbeat.iter().map(|t| &t.model_id).collect::<Vec<_>>());
                        println!("  Daily: {:?}", hierarchy.daily.iter().map(|t| &t.model_id).collect::<Vec<_>>());
                        println!("  Planning: {:?}", hierarchy.planning.iter().map(|t| &t.model_id).collect::<Vec<_>>());
                        println!("  Coding: {:?}", hierarchy.coding.iter().map(|t| &t.model_id).collect::<Vec<_>>());
                        println!("  Review: {:?}", hierarchy.review.iter().map(|t| &t.model_id).collect::<Vec<_>>());
                    }
                }
                HierarchyAction::SetPreset { preset } => {
                    let hierarchy = ModelHierarchy::from_preset(&preset)
                        .ok_or_else(|| anyhow::anyhow!("Unknown preset: '{}'. Valid: balanced, budget, premium, speed, claude-only", preset))?;

                    hierarchy.save(&config_dir)?;

                    if json_mode {
                        println!("{}", json_output(true, serde_json::json!({
                            "preset": preset,
                            "hierarchy": hierarchy,
                        }), None));
                    } else {
                        println!("Hierarchy set to '{}' preset", preset);
                    }
                }
                HierarchyAction::SetModel { category, tier, model_id } => {
                    let mut hierarchy = ModelHierarchy::load(&config_dir)?;
                    let task_category = TaskCategory::from_str(&category)
                        .ok_or_else(|| anyhow::anyhow!("Unknown category: '{}'. Valid: heartbeat, daily, planning, coding, review", category))?;

                    let tiers = match task_category {
                        TaskCategory::Heartbeat => &mut hierarchy.heartbeat,
                        TaskCategory::Daily => &mut hierarchy.daily,
                        TaskCategory::Planning => &mut hierarchy.planning,
                        TaskCategory::Coding => &mut hierarchy.coding,
                        TaskCategory::Review => &mut hierarchy.review,
                    };

                    // Ensure tier exists
                    while tiers.len() <= tier {
                        tiers.push(hierarchy::ModelTier {
                            model_id: "".to_string(),
                            max_tokens: None,
                            max_cost_per_request: None,
                        });
                    }

                    tiers[tier].model_id = model_id.clone();
                    hierarchy.save(&config_dir)?;

                    if json_mode {
                        println!("{}", json_output(true, serde_json::json!({
                            "category": category,
                            "tier": tier,
                            "model_id": model_id,
                        }), None));
                    } else {
                        println!("Set {} tier {} to: {}", category, tier, model_id);
                    }
                }
                HierarchyAction::ShowPolicy => {
                    let policy = EscalationPolicy::load(&config_dir)?;
                    if json_mode {
                        println!("{}", json_output(true, serde_json::to_value(&policy)?, None));
                    } else {
                        println!("Escalation Policy:");
                        println!("  Enabled: {}", policy.enabled);
                        println!("  Max escalations: {}", policy.max_escalations);
                        println!("  Daily budget limit: ${:.2}", policy.daily_budget_limit);
                        println!("  Escalate on error: {}", policy.escalate_on_error);
                        println!("  Escalate on refusal: {}", policy.escalate_on_refusal);
                        println!("  Escalate on syntax error: {}", policy.escalate_on_syntax_error);
                    }
                }
                HierarchyAction::UpdatePolicy { enabled, max_escalations, budget_limit } => {
                    let mut policy = EscalationPolicy::load(&config_dir)?;

                    if let Some(e) = enabled {
                        policy.enabled = e;
                    }
                    if let Some(m) = max_escalations {
                        policy.max_escalations = m;
                    }
                    if let Some(b) = budget_limit {
                        policy.daily_budget_limit = b;
                    }

                    policy.save(&config_dir)?;

                    if json_mode {
                        println!("{}", json_output(true, serde_json::to_value(&policy)?, None));
                    } else {
                        println!("Escalation policy updated");
                    }
                }
            }
        }
        Commands::OAuth { action } => {
            match action {
                OAuthAction::GetUrl { provider } => {
                    // Just get the auth URL without starting callback server
                    let config_manager = ConfigManager::new()?;
                    let auth_url = match oauth::start_oauth_flow(&provider, config_manager.get()) {
                        Ok(url) => url,
                        Err(e) => {
                            let msg = format!("Failed to generate OAuth URL: {}", e);
                            if json_mode {
                                println!("{}", json_output(false, serde_json::Value::Null, Some(&msg)));
                            } else {
                                eprintln!("{}", msg);
                            }
                            std::process::exit(1);
                        }
                    };

                    if json_mode {
                        println!("{}", json_output(true, serde_json::json!({
                            "auth_url": auth_url,
                            "provider": provider,
                            "callback_port": 8765,
                        }), None));
                    } else {
                        println!("OAuth URL: {}", auth_url);
                        println!("After opening the URL, run: nexus oauth wait-callback {}", provider);
                    }
                }
                OAuthAction::WaitCallback { provider } => {
                    // Start callback server and wait
                    let config_manager = ConfigManager::new()?;
                    let token = match oauth::handle_oauth_callback(&provider, config_manager.get(), 300) {
                        Ok(t) => t,
                        Err(e) => {
                            let msg = format!("OAuth callback failed: {}", e);
                            if json_mode {
                                println!("{}", json_output(false, serde_json::Value::Null, Some(&msg)));
                            } else {
                                eprintln!("{}", msg);
                            }
                            std::process::exit(1);
                        }
                    };

                    // Save token
                    let mut config_manager = ConfigManager::new()?;
                    if let Err(e) = oauth::save_oauth_token(&provider, &token, &mut config_manager) {
                        let msg = format!("Failed to save OAuth token: {}", e);
                        if json_mode {
                            println!("{}", json_output(false, serde_json::Value::Null, Some(&msg)));
                        } else {
                            eprintln!("{}", msg);
                        }
                        std::process::exit(1);
                    }

                    if json_mode {
                        println!("{}", json_output(true, serde_json::json!({
                            "provider": provider,
                            "message": "OAuth token saved successfully"
                        }), None));
                    } else {
                        println!("✅ OAuth authorization completed for '{}'", provider);
                    }
                }
                OAuthAction::Authorize { provider } => {
                    let config_manager = ConfigManager::new()?;
                    // Start OAuth flow
                    let auth_url = match oauth::start_oauth_flow(&provider, config_manager.get()) {
                        Ok(url) => url,
                        Err(e) => {
                            let msg = format!("Failed to start OAuth flow: {}", e);
                            if json_mode {
                                println!("{}", json_output(false, serde_json::Value::Null, Some(&msg)));
                            } else {
                                eprintln!("{}", msg);
                            }
                            std::process::exit(1);
                        }
                    };

                    if json_mode {
                        println!("{}", json_output(true, serde_json::json!({
                            "auth_url": auth_url,
                            "provider": provider,
                            "callback_port": 8765,
                        }), None));
                    } else {
                        println!("Opening browser for OAuth authorization...");
                        println!("If the browser doesn't open, visit: {}", auth_url);
                    }

                    // Open browser
                    if let Err(e) = open::that(&auth_url) {
                        eprintln!("Failed to open browser: {}", e);
                        println!("Please manually open: {}", auth_url);
                    }

                    // Handle callback and get token
                    let token = match oauth::handle_oauth_callback(&provider, config_manager.get(), 300) {
                        Ok(t) => t,
                        Err(e) => {
                            let msg = format!("OAuth authorization failed: {}", e);
                            if json_mode {
                                println!("{}", json_output(false, serde_json::Value::Null, Some(&msg)));
                            } else {
                                eprintln!("{}", msg);
                            }
                            std::process::exit(1);
                        }
                    };

                    // Save token to config
                    let mut config_manager = ConfigManager::new()?;
                    if let Err(e) = oauth::save_oauth_token(&provider, &token, &mut config_manager) {
                        let msg = format!("Failed to save OAuth token: {}", e);
                        if json_mode {
                            println!("{}", json_output(false, serde_json::Value::Null, Some(&msg)));
                        } else {
                            eprintln!("{}", msg);
                        }
                        std::process::exit(1);
                    }

                    if json_mode {
                        println!("{}", json_output(true, serde_json::json!({
                            "status": "authorized",
                            "provider": provider,
                            "expires_in": token.expires_in,
                        }), None));
                    } else {
                        println!("✅ OAuth authorization successful for {}!", provider);
                        if let Some(exp) = token.expires_in {
                            println!("Token expires in {} seconds", exp);
                        }
                    }
                }
                OAuthAction::Status { provider } => {
                    let config_manager = ConfigManager::new()?;
                    let status = match oauth::check_oauth_status(&provider, config_manager.get()) {
                        Ok(s) => s,
                        Err(e) => {
                            let msg = format!("Failed to check OAuth status: {}", e);
                            if json_mode {
                                println!("{}", json_output(false, serde_json::Value::Null, Some(&msg)));
                            } else {
                                eprintln!("{}", msg);
                            }
                            std::process::exit(1);
                        }
                    };

                    if json_mode {
                        println!("{}", json_output(true, serde_json::to_value(&status)?, None));
                    } else {
                        if status.authorized {
                            println!("✅ Provider {} is authorized", provider);
                            if let Some(exp) = status.expires_at {
                                println!("Expires at: {}", exp);
                            }
                        } else {
                            println!("❌ Provider {} is not authorized", provider);
                            println!("Run: nexus oauth authorize {}", provider);
                        }
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

            // Load memory context
            let memory_path = std::env::var("HOME")
                .map(|h| std::path::PathBuf::from(h).join(".config/nexus/memory"))
                .unwrap_or_else(|_| std::path::PathBuf::from("~/.config/nexus/memory"));

            let memory_context = if let Ok(mem) = MemorySystem::new(memory_path.clone()) {
                match mem.get_context_for_query(&message).await {
                    Ok(context) => format!("\n\n{}", context.format_for_llm()),
                    Err(_) => String::new(),
                }
            } else {
                String::new()
            };

            let system_prompt = if memory_context.is_empty() {
                create_tool_system_prompt()
            } else {
                format!("{}{}", create_tool_system_prompt(), memory_context)
            };

            let mut messages = vec![
                Message {
                    role: Role::System,
                    content: system_prompt,
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
        oauth_expires_at: None,
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

// ============================================================================
// OAuth Flow Implementation
// ============================================================================

async fn run_oauth_flow(provider: &str, config_manager: &ConfigManager) -> Result<String> {
    // For Google, use the gemini-cli compatible OAuth flow from oauth.rs
    match provider {
        "google" => {
            let auth_url = oauth::start_oauth_flow(provider, config_manager.get())?;
            Ok(auth_url)
        }
        "claude" => {
            let provider_config = config_manager.get_provider(provider)
                .ok_or_else(|| anyhow::anyhow!("Provider '{}' not configured", provider))?;
            let claude_provider = providers::claude::ClaudeProvider::new(provider_config);
            let auth_url = claude_provider.generate_auth_url()?;
            Ok(auth_url)
        }
        _ => Err(anyhow::anyhow!("Provider '{}' does not support OAuth", provider)),
    }
}
