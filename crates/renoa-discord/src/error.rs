use thiserror::Error;

#[derive(Debug, Error)]
pub enum DiscordError {
    #[error("invalid Discord surface configuration or state: {0}")]
    Invalid(String),
    #[error("Discord surface I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("Discord surface database failed: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("Discord surface configuration file is invalid: {0}")]
    Config(#[from] serde_json::Error),
    #[error("{0}")]
    Api(String),
    #[error("Discord surface task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
    #[error(transparent)]
    Host(#[from] renoa_local::LocalHostError),
}
