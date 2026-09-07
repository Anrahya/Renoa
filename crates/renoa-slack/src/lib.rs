mod actions;
mod agents;
mod api;
mod channels;
mod commands;
mod config;
mod controls;
mod events;
mod routines;
mod service;
mod socket;
mod store;
mod surface_context;
mod worker;
pub use service::run;
mod ingress;
mod inspect;
pub use inspect::inspect;

pub use config::Config;

#[derive(Debug, thiserror::Error)]
pub enum SlackError {
    #[error("invalid Slack configuration or state: {0}")]
    Invalid(String),
    #[error("Slack storage I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("Slack database failed: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("Slack JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Slack task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
    #[error(transparent)]
    Host(#[from] renoa_local::LocalHostError),
    #[error(transparent)]
    Api(#[from] api::ApiError),
}

#[cfg(test)]
mod tests;
