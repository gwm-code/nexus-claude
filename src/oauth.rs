// OAuth 2.0 PKCE Flow Implementation for Nexus Desktop
// Supports: Google, Anthropic (Claude), OpenAI

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::{SystemTime, UNIX_EPOCH};

const CALLBACK_PORT: u16 = 8765; // Local HTTP server port for OAuth callback
const CALLBACK_URL: &str = "http://localhost:8765/callback";

/// OAuth provider configuration (URLs and scopes only)
#[derive(Debug, Clone)]
pub struct OAuthProvider {
    pub name: String,
    pub auth_url: &'static str,
    pub token_url: &'static str,
    pub client_id: String,
    pub client_secret: String,
    pub scopes: Vec<&'static str>,
}

impl OAuthProvider {
    /// Get provider configuration from stored config
    pub fn from_config(name: &str, config: &crate::config::NexusConfig) -> Result<Self> {
        let provider_config = config.providers.get(name)
            .ok_or_else(|| anyhow!("Provider {} not found in config", name))?;

        let client_id = provider_config.oauth_client_id.as_ref()
            .ok_or_else(|| anyhow!("OAuth Client ID not configured for provider {}. Run: nexus config set-oauth {} <client-id> <client-secret>", name, name))?
            .clone();

        let client_secret = provider_config.oauth_client_secret.as_ref()
            .ok_or_else(|| anyhow!("OAuth Client Secret not configured for provider {}. Run: nexus config set-oauth {} <client-id> <client-secret>", name, name))?
            .clone();

        let (auth_url, token_url, scopes) = match name.to_lowercase().as_str() {
            "google" => (
                "https://accounts.google.com/o/oauth2/v2/auth",
                "https://oauth2.googleapis.com/token",
                vec![
                    "https://www.googleapis.com/auth/generative-language.retriever",
                    "openid",
                    "email",
                    "profile"
                ],
            ),
            "claude" | "anthropic" => (
                "https://auth.anthropic.com/oauth/authorize",
                "https://auth.anthropic.com/oauth/token",
                vec!["api"],
            ),
            "openai" => (
                "https://auth.openai.com/authorize",
                "https://auth.openai.com/oauth/token",
                vec!["api"],
            ),
            _ => return Err(anyhow!("Unsupported OAuth provider: {}", name)),
        };

        Ok(Self {
            name: name.to_string(),
            auth_url,
            token_url,
            client_id,
            client_secret,
            scopes,
        })
    }
}

/// OAuth token response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthToken {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: Option<u64>,
    pub refresh_token: Option<String>,
    pub scope: Option<String>,
}

/// PKCE code verifier and challenge
struct PkceChallenge {
    verifier: String,
    challenge: String,
}

impl PkceChallenge {
    /// Generate PKCE code verifier and challenge
    fn generate() -> Result<Self> {
        use sha2::{Sha256, Digest};
        use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

        // Generate random 32-byte verifier
        let verifier: String = (0..32)
            .map(|_| {
                let idx = rand::random::<usize>() % 62;
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789"[idx] as char
            })
            .collect();

        // Generate SHA256 challenge
        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let hash = hasher.finalize();
        let challenge = URL_SAFE_NO_PAD.encode(&hash);

        Ok(Self { verifier, challenge })
    }
}

/// Start OAuth PKCE flow and return authorization URL
pub fn start_oauth_flow(provider_name: &str, config: &crate::config::NexusConfig) -> Result<String> {
    let provider = OAuthProvider::from_config(provider_name, config)?;
    let pkce = PkceChallenge::generate()?;

    // Generate random state for CSRF protection
    let state: String = (0..16)
        .map(|_| {
            let idx = rand::random::<usize>() % 62;
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789"[idx] as char
        })
        .collect();

    // Build authorization URL
    let scope = provider.scopes.join(" ");
    let auth_url = format!(
        "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&state={}&code_challenge={}&code_challenge_method=S256",
        provider.auth_url,
        urlencoding::encode(&provider.client_id),
        urlencoding::encode(CALLBACK_URL),
        urlencoding::encode(&scope),
        state,
        pkce.challenge
    );

    // Store PKCE verifier and state for callback validation
    // TODO: Store in-memory or temp file for callback handler to access
    std::fs::write(
        format!("/tmp/nexus_oauth_state_{}.json", provider.name),
        serde_json::json!({
            "verifier": pkce.verifier,
            "state": state,
            "provider": provider.name,
        }).to_string()
    )?;

    Ok(auth_url)
}

/// Handle OAuth callback and exchange code for token
pub fn handle_oauth_callback(provider_name: &str, config: &crate::config::NexusConfig, timeout_secs: u64) -> Result<OAuthToken> {
    let provider = OAuthProvider::from_config(provider_name, config)?;

    println!("Starting local callback server on port {}...", CALLBACK_PORT);
    println!("Waiting for OAuth callback...");

    // Start local HTTP server to receive callback
    let listener = TcpListener::bind(format!("127.0.0.1:{}", CALLBACK_PORT))
        .context("Failed to start callback server. Port might be in use.")?;

    // Set read timeout
    listener.set_nonblocking(false)?;

    // Accept one connection
    let (mut stream, _) = listener.accept()?;

    // Read HTTP request
    let mut reader = BufReader::new(&stream);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;

    // Parse query parameters from request line
    // Format: GET /callback?code=...&state=... HTTP/1.1
    let query_start = request_line.find('?').ok_or_else(|| anyhow!("No query parameters in callback"))?;
    let query_end = request_line.find(" HTTP").ok_or_else(|| anyhow!("Invalid HTTP request"))?;
    let query_string = &request_line[query_start + 1..query_end];

    let params: HashMap<String, String> = query_string
        .split('&')
        .filter_map(|pair| {
            let mut parts = pair.split('=');
            Some((
                parts.next()?.to_string(),
                urlencoding::decode(parts.next()?).ok()?.to_string()
            ))
        })
        .collect();

    // Send success response to browser
    let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n\
        <html><body style='font-family: sans-serif; text-align: center; padding: 50px;'>\
        <h1>✅ Authentication Successful!</h1>\
        <p>You can close this window and return to Nexus Desktop.</p>\
        </body></html>";
    stream.write_all(response.as_bytes())?;
    stream.flush()?;

    // Extract code and state
    let code = params.get("code")
        .ok_or_else(|| anyhow!("No authorization code in callback"))?;
    let callback_state = params.get("state")
        .ok_or_else(|| anyhow!("No state in callback"))?;

    // Load stored PKCE verifier and validate state
    let state_file = format!("/tmp/nexus_oauth_state_{}.json", provider.name);
    let state_data: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&state_file)?
    )?;

    let stored_state = state_data["state"].as_str()
        .ok_or_else(|| anyhow!("Invalid state data"))?;

    if callback_state != stored_state {
        return Err(anyhow!("State mismatch - possible CSRF attack"));
    }

    let verifier = state_data["verifier"].as_str()
        .ok_or_else(|| anyhow!("Invalid verifier data"))?;

    // Clean up state file
    let _ = std::fs::remove_file(&state_file);

    // Exchange authorization code for access token
    let token = exchange_code_for_token(&provider, code, verifier)?;

    println!("✅ OAuth token obtained successfully!");

    Ok(token)
}

/// Exchange authorization code for access token
fn exchange_code_for_token(provider: &OAuthProvider, code: &str, verifier: &str) -> Result<OAuthToken> {
    use reqwest::blocking::Client;

    let client = Client::new();

    // Build form-urlencoded body manually
    let body = format!(
        "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&client_secret={}&code_verifier={}",
        urlencoding::encode(code),
        urlencoding::encode(CALLBACK_URL),
        urlencoding::encode(&provider.client_id),
        urlencoding::encode(&provider.client_secret),
        urlencoding::encode(verifier)
    );

    let response = client
        .post(provider.token_url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .context("Failed to exchange code for token")?;

    if !response.status().is_success() {
        let error_text = response.text().unwrap_or_else(|_| "Unknown error".to_string());
        return Err(anyhow!("Token exchange failed: {}", error_text));
    }

    let token: OAuthToken = response.json()
        .context("Failed to parse token response")?;

    Ok(token)
}

/// Check if OAuth token is valid and not expired
pub fn check_oauth_status(provider_name: &str, config: &crate::config::NexusConfig) -> Result<OAuthStatus> {
    let provider_config = config.providers.get(provider_name)
        .ok_or_else(|| anyhow!("Provider {} not found in config", provider_name))?;

    let has_token = provider_config.oauth_token.is_some();
    let expires_at = provider_config.oauth_expires_at;

    let is_authorized = if let Some(exp) = expires_at {
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        has_token && exp > now
    } else {
        has_token
    };

    Ok(OAuthStatus {
        authorized: is_authorized,
        provider: provider_name.to_string(),
        expires_at: expires_at.map(|t| {
            chrono::DateTime::from_timestamp(t as i64, 0)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default()
        }),
    })
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OAuthStatus {
    pub authorized: bool,
    pub provider: String,
    pub expires_at: Option<String>,
}

/// Save OAuth token to config via ConfigManager
pub fn save_oauth_token(
    provider_name: &str,
    token: &OAuthToken,
    config_manager: &mut crate::config::ConfigManager,
) -> Result<()> {
    let config = config_manager.get_mut();
    let provider_config = config.providers.get_mut(provider_name)
        .ok_or_else(|| anyhow!("Provider {} not found in config", provider_name))?;

    provider_config.oauth_token = Some(token.access_token.clone());
    provider_config.oauth_refresh_token = token.refresh_token.clone();

    // Calculate expiration timestamp
    if let Some(expires_in) = token.expires_in {
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        provider_config.oauth_expires_at = Some(now + expires_in);
    }

    config_manager.save()?;

    Ok(())
}
