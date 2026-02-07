use crate::error::{NexusError, Result};
use crate::secret_store;
use config::{Config, Environment, File};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthStatus {
    pub authorized: bool,
    pub provider: String,
    pub expires_at: Option<String>,
}

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

        // Restrict file permissions to owner-only (0600) on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o600);
            let _ = fs::set_permissions(&self.config_path, perms);
        }

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

    /// Add a provider with its API key stored securely in the OS keyring.
    /// The config file will contain a sentinel value instead of the plaintext key.
    pub fn add_provider_secure(&mut self, name: String, mut provider: ProviderConfig) -> Result<()> {
        // Migrate API key to keyring if possible
        if let Some(ref api_key) = provider.api_key {
            let key_name = format!("provider.{}.api_key", name);
            if let Some(sentinel) = secret_store::migrate_secret(&key_name, api_key) {
                provider.api_key = Some(sentinel);
            }
        }

        // Migrate OAuth tokens
        if let Some(ref token) = provider.oauth_token {
            let key_name = format!("provider.{}.oauth_token", name);
            if let Some(sentinel) = secret_store::migrate_secret(&key_name, token) {
                provider.oauth_token = Some(sentinel);
            }
        }
        if let Some(ref secret) = provider.oauth_client_secret {
            let key_name = format!("provider.{}.oauth_client_secret", name);
            if let Some(sentinel) = secret_store::migrate_secret(&key_name, secret) {
                provider.oauth_client_secret = Some(sentinel);
            }
        }
        if let Some(ref refresh) = provider.oauth_refresh_token {
            let key_name = format!("provider.{}.oauth_refresh_token", name);
            if let Some(sentinel) = secret_store::migrate_secret(&key_name, refresh) {
                provider.oauth_refresh_token = Some(sentinel);
            }
        }

        self.config.providers.insert(name, provider);
        self.save()
    }

    /// Store an API key securely for an existing provider
    pub fn set_api_key_secure(&mut self, provider_name: &str, api_key: &str) -> Result<()> {
        let provider = self.config.providers.get_mut(provider_name).ok_or_else(|| {
            NexusError::ProviderNotConfigured(provider_name.to_string())
        })?;

        let key_name = format!("provider.{}.api_key", provider_name);
        secret_store::store_secret(&key_name, api_key)?;
        provider.api_key = Some(secret_store::make_sentinel(&key_name));
        self.save()
    }

    pub fn get_provider(&self, name: &str) -> Option<&ProviderConfig> {
        self.config.providers.get(name)
    }

    /// Get a provider config with secrets resolved from keyring
    pub fn get_provider_resolved(&self, name: &str) -> Result<Option<ProviderConfig>> {
        match self.config.providers.get(name) {
            None => Ok(None),
            Some(provider) => {
                let mut resolved = provider.clone();
                if let Some(ref val) = resolved.api_key {
                    resolved.api_key = Some(secret_store::resolve_secret(val)?);
                }
                if let Some(ref val) = resolved.oauth_token {
                    resolved.oauth_token = Some(secret_store::resolve_secret(val)?);
                }
                if let Some(ref val) = resolved.oauth_client_secret {
                    resolved.oauth_client_secret = Some(secret_store::resolve_secret(val)?);
                }
                if let Some(ref val) = resolved.oauth_refresh_token {
                    resolved.oauth_refresh_token = Some(secret_store::resolve_secret(val)?);
                }
                Ok(Some(resolved))
            }
        }
    }

    pub fn list_providers(&self) -> Vec<&String> {
        self.config.providers.keys().collect()
    }

    pub fn get_config_path(&self) -> Result<PathBuf> {
        Ok(self.config_path.clone())
    }

    /// Migrate all plaintext secrets in config to the OS keyring.
    /// Returns the number of secrets migrated.
    pub fn migrate_secrets(&mut self) -> Result<usize> {
        let mut migrated = 0;

        let provider_names: Vec<String> = self.config.providers.keys().cloned().collect();
        for name in provider_names {
            let provider = self.config.providers.get_mut(&name).unwrap();

            if let Some(ref val) = provider.api_key {
                if secret_store::parse_sentinel(val).is_none() && !val.is_empty() {
                    let key_name = format!("provider.{}.api_key", name);
                    if let Some(sentinel) = secret_store::migrate_secret(&key_name, val) {
                        provider.api_key = Some(sentinel);
                        migrated += 1;
                    }
                }
            }

            if let Some(ref val) = provider.oauth_token {
                if secret_store::parse_sentinel(val).is_none() && !val.is_empty() {
                    let key_name = format!("provider.{}.oauth_token", name);
                    if let Some(sentinel) = secret_store::migrate_secret(&key_name, val) {
                        provider.oauth_token = Some(sentinel);
                        migrated += 1;
                    }
                }
            }

            if let Some(ref val) = provider.oauth_client_secret {
                if secret_store::parse_sentinel(val).is_none() && !val.is_empty() {
                    let key_name = format!("provider.{}.oauth_client_secret", name);
                    if let Some(sentinel) = secret_store::migrate_secret(&key_name, val) {
                        provider.oauth_client_secret = Some(sentinel);
                        migrated += 1;
                    }
                }
            }

            if let Some(ref val) = provider.oauth_refresh_token {
                if secret_store::parse_sentinel(val).is_none() && !val.is_empty() {
                    let key_name = format!("provider.{}.oauth_refresh_token", name);
                    if let Some(sentinel) = secret_store::migrate_secret(&key_name, val) {
                        provider.oauth_refresh_token = Some(sentinel);
                        migrated += 1;
                    }
                }
            }
        }

        if migrated > 0 {
            self.save()?;
        }

        Ok(migrated)
    }

    /// Store OAuth credentials (client ID and secret) in keyring
    pub fn set_oauth_credentials(&mut self, provider_name: &str, client_id: &str, client_secret: &str) -> Result<()> {
        let provider = self.config.providers.get_mut(provider_name)
            .ok_or_else(|| NexusError::Configuration(format!("Provider '{}' not found", provider_name)))?;

        // Store client_id and client_secret in keyring
        let client_id_key = format!("provider.{}.oauth_client_id", provider_name);
        let client_secret_key = format!("provider.{}.oauth_client_secret", provider_name);

        secret_store::store_secret(&client_id_key, client_id)?;
        secret_store::store_secret(&client_secret_key, client_secret)?;

        provider.oauth_client_id = Some(secret_store::make_sentinel(&client_id_key));
        provider.oauth_client_secret = Some(secret_store::make_sentinel(&client_secret_key));

        self.save()?;
        Ok(())
    }

    /// Get OAuth status for a provider
    pub fn get_oauth_status(&self, provider_name: &str) -> Result<OAuthStatus> {
        let provider = self.get_provider(provider_name)
            .ok_or_else(|| NexusError::Configuration(format!("Provider '{}' not found", provider_name)))?;

        let authorized = provider.oauth_token.is_some();

        // TODO: Parse expires_at from stored token metadata
        let expires_at = None;

        Ok(OAuthStatus {
            authorized,
            provider: provider_name.to_string(),
            expires_at,
        })
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
