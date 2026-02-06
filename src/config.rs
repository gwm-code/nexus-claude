use crate::error::{NexusError, Result};
use config::{Config, Environment, File};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NexusConfig {
    #[serde(default)]
    pub default_provider: Option<String>,

    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,

    #[serde(default)]
    pub ui: UiConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub provider_type: ProviderType,
    pub api_key: Option<String>,
    pub oauth_token: Option<String>,
    pub oauth_client_id: Option<String>,
    pub oauth_client_secret: Option<String>,
    pub oauth_refresh_token: Option<String>,
    pub base_url: Option<String>,
    pub default_model: Option<String>,
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderType {
    Opencode,
    Openrouter,
    Google,
    Claude,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    #[serde(default = "default_true")]
    pub show_diff_preview: bool,
    #[serde(default = "default_true")]
    pub confirm_dangerous_commands: bool,
    #[serde(default = "default_timeout")]
    pub command_timeout_secs: u64,
}

fn default_true() -> bool {
    true
}

fn default_timeout() -> u64 {
    30
}

impl Default for NexusConfig {
    fn default() -> Self {
        Self {
            default_provider: None,
            providers: HashMap::new(),
            ui: UiConfig::default(),
        }
    }
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            show_diff_preview: true,
            confirm_dangerous_commands: true,
            command_timeout_secs: 30,
        }
    }
}

pub struct ConfigManager {
    config: NexusConfig,
    config_path: PathBuf,
}

impl ConfigManager {
    pub fn new() -> Result<Self> {
        let config_path = Self::get_config_path_internal()?;
        let config = Self::load_or_default(&config_path)?;

        Ok(Self {
            config,
            config_path,
        })
    }

    pub fn load() -> Result<NexusConfig> {
        let config_path = Self::get_config_path_internal()?;
        Self::load_or_default(&config_path)
    }

    pub fn save(&self) -> Result<()> {
        let toml = toml::to_string_pretty(&self.config)
            .map_err(|e| NexusError::Configuration(format!("Failed to serialize config: {}", e)))?;

        fs::write(&self.config_path, toml)
            .map_err(|e| NexusError::Configuration(format!("Failed to write config: {}", e)))?;

        Ok(())
    }

    pub fn get(&self) -> &NexusConfig {
        &self.config
    }

    pub fn get_mut(&mut self) -> &mut NexusConfig {
        &mut self.config
    }

    pub fn add_provider(&mut self, name: String, provider: ProviderConfig) -> Result<()> {
        self.config.providers.insert(name, provider);
        self.save()
    }

    pub fn get_provider(&self, name: &str) -> Option<&ProviderConfig> {
        self.config.providers.get(name)
    }

    pub fn list_providers(&self) -> Vec<&String> {
        self.config.providers.keys().collect()
    }

    pub fn get_config_path(&self) -> Result<PathBuf> {
        Ok(self.config_path.clone())
    }

    fn get_config_path_internal() -> Result<PathBuf> {
        let project_dirs = ProjectDirs::from("com", "nexus", "nexus").ok_or_else(|| {
            NexusError::Configuration("Could not determine config directory".to_string())
        })?;

        let config_dir = project_dirs.config_dir();
        fs::create_dir_all(config_dir)?;

        Ok(config_dir.join("config.toml"))
    }

    fn load_or_default(path: &PathBuf) -> Result<NexusConfig> {
        if !path.exists() {
            return Ok(NexusConfig::default());
        }

        let s = Config::builder()
            .add_source(File::from(path.clone()))
            .add_source(Environment::with_prefix("NEXUS"))
            .build()
            .map_err(|e| NexusError::Configuration(format!("Failed to build config: {}", e)))?;

        let config: NexusConfig = s.try_deserialize().map_err(|e| {
            NexusError::Configuration(format!("Failed to deserialize config: {}", e))
        })?;

        Ok(config)
    }
}
