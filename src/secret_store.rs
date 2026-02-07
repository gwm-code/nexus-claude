//! Secure secret storage for API keys and tokens.
//!
//! Uses the OS keyring (macOS Keychain, Linux Secret Service, Windows Credential Manager)
//! when available, with a graceful fallback to file-based storage with restricted permissions.

use crate::error::{NexusError, Result};

const SERVICE_NAME: &str = "nexus-cli";

/// Sentinel prefix stored in config.toml to indicate the key is in the secret store
const KEYRING_SENTINEL: &str = "<keyring:";

/// Store a secret in the OS keyring
pub fn store_secret(key_name: &str, secret: &str) -> Result<()> {
    let entry = keyring::Entry::new(SERVICE_NAME, key_name)
        .map_err(|e| NexusError::Keyring(format!("Failed to create keyring entry: {}", e)))?;

    entry
        .set_password(secret)
        .map_err(|e| NexusError::Keyring(format!("Failed to store secret '{}': {}", key_name, e)))?;

    Ok(())
}

/// Retrieve a secret from the OS keyring
pub fn get_secret(key_name: &str) -> Result<Option<String>> {
    let entry = keyring::Entry::new(SERVICE_NAME, key_name)
        .map_err(|e| NexusError::Keyring(format!("Failed to create keyring entry: {}", e)))?;

    match entry.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(NexusError::Keyring(format!(
            "Failed to retrieve secret '{}': {}",
            key_name, e
        ))),
    }
}

/// Delete a secret from the OS keyring
pub fn delete_secret(key_name: &str) -> Result<()> {
    let entry = keyring::Entry::new(SERVICE_NAME, key_name)
        .map_err(|e| NexusError::Keyring(format!("Failed to create keyring entry: {}", e)))?;

    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()), // Already gone
        Err(e) => Err(NexusError::Keyring(format!(
            "Failed to delete secret '{}': {}",
            key_name, e
        ))),
    }
}

/// Check if the keyring backend is available on this system
pub fn is_keyring_available() -> bool {
    let test_key = "__nexus_keyring_test__";
    let entry = match keyring::Entry::new(SERVICE_NAME, test_key) {
        Ok(e) => e,
        Err(_) => return false,
    };

    // Try to set and delete a test value
    match entry.set_password("test") {
        Ok(()) => {
            let _ = entry.delete_credential();
            true
        }
        Err(_) => false,
    }
}

/// Create a sentinel value for storing in config.toml
pub fn make_sentinel(key_name: &str) -> String {
    format!("{}{}>", KEYRING_SENTINEL, key_name)
}

/// Check if a value is a keyring sentinel and extract the key name
pub fn parse_sentinel(value: &str) -> Option<&str> {
    if value.starts_with(KEYRING_SENTINEL) && value.ends_with('>') {
        Some(&value[KEYRING_SENTINEL.len()..value.len() - 1])
    } else {
        None
    }
}

/// Resolve a config value: if it's a sentinel, look up the real secret;
/// otherwise return the value as-is (for backward compatibility with plaintext)
pub fn resolve_secret(value: &str) -> Result<String> {
    if let Some(key_name) = parse_sentinel(value) {
        match get_secret(key_name)? {
            Some(secret) => Ok(secret),
            None => Err(NexusError::Keyring(format!(
                "Secret '{}' referenced in config but not found in keyring",
                key_name
            ))),
        }
    } else {
        Ok(value.to_string())
    }
}

/// Migrate a plaintext secret to the keyring, returning the sentinel to store in config.
/// If the keyring is unavailable, returns None (keep plaintext).
pub fn migrate_secret(key_name: &str, plaintext: &str) -> Option<String> {
    if !is_keyring_available() {
        return None;
    }

    match store_secret(key_name, plaintext) {
        Ok(()) => Some(make_sentinel(key_name)),
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sentinel_roundtrip() {
        let sentinel = make_sentinel("provider.claude.api_key");
        assert_eq!(
            parse_sentinel(&sentinel),
            Some("provider.claude.api_key")
        );
    }

    #[test]
    fn test_parse_non_sentinel() {
        assert_eq!(parse_sentinel("sk-12345"), None);
        assert_eq!(parse_sentinel(""), None);
        assert_eq!(parse_sentinel("<keyring:"), None); // no closing >
    }

    #[test]
    fn test_resolve_plaintext() {
        // Non-sentinel values should pass through unchanged
        assert_eq!(resolve_secret("sk-12345").unwrap(), "sk-12345");
    }
}
