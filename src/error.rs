use thiserror::Error;

#[derive(Error, Debug)]
pub enum NexusError {
    #[error("API request failed: {0}")]
    ApiRequest(String),

    #[error("Authentication failed: {0}")]
    Authentication(String),

    #[error("Configuration error: {0}")]
    Configuration(String),

    #[error("OAuth flow error: {0}")]
    OAuth(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Provider not configured: {0}")]
    ProviderNotConfigured(String),

    #[error("Keyring error: {0}")]
    Keyring(String),

    #[error("User cancelled")]
    UserCancelled,

    #[error("Regex error: {0}")]
    Regex(#[from] regex::Error),

    #[error("Dialog error: {0}")]
    Dialog(String),

    #[error(
        "File {path} has been modified since you last read it. Please re-read the file first."
    )]
    FileStale { path: String },
}

impl From<dialoguer::Error> for NexusError {
    fn from(err: dialoguer::Error) -> Self {
        NexusError::Dialog(err.to_string())
    }
}

pub type Result<T> = std::result::Result<T, NexusError>;
