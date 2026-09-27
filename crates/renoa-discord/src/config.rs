use std::{fs, path::PathBuf};

use serde::Deserialize;

use crate::{DiscordError, connection::Connection};
use renoa_local::{LocalHost, LocalHostAdapters, LocalModelConfiguration, RenoaHome};

/// The worker's launch: its trusted runtime file and the owner's connection.
pub(crate) struct Config {
    pub(crate) home: RenoaHome,
    pub(crate) connection: Connection,
    pub(crate) runtime: Runtime,
}

#[derive(Deserialize)]
struct OAuthRelay {
    origin: String,
    credentials: PathBuf,
}

/// How this Host runs agents. Discord identity comes from the connection.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Runtime {
    #[serde(default)]
    home: PathBuf,
    models: LocalModelConfiguration,
    mcp_adapter: Option<PathBuf>,
    code_mode_worker: Option<PathBuf>,
    mcp_registry_adapter: Option<PathBuf>,
    shared_plugin_registry: Option<String>,
    oauth_relay: Option<OAuthRelay>,
}

impl Config {
    /// Reads the runtime file, then the connection the owner committed.
    ///
    /// # Errors
    ///
    /// Returns malformed settings, a relative adapter path, a missing or
    /// shared connection file, or an invalid Renoa home.
    pub(crate) fn read(path: &std::path::Path) -> Result<Self, DiscordError> {
        let runtime: Runtime = serde_json::from_slice(&fs::read(path)?)?;
        let home = RenoaHome::resolve(Some(runtime.home.clone()))?;
        let adapters = runtime
            .mcp_adapter
            .iter()
            .chain(&runtime.code_mode_worker)
            .chain(&runtime.mcp_registry_adapter)
            .chain(runtime.oauth_relay.iter().map(|relay| &relay.credentials));
        for adapter in adapters {
            if !adapter.is_absolute() {
                return Err(DiscordError::Invalid(
                    "Discord adapter and credential paths must be absolute".to_owned(),
                ));
            }
        }
        let connection = Connection::read(&home)?.ok_or_else(|| {
            DiscordError::Invalid("Connect Discord from the Control Room first".to_owned())
        })?;
        Ok(Self {
            home,
            connection,
            runtime,
        })
    }
}

impl Runtime {
    pub(crate) fn open_host(self, home: &RenoaHome) -> Result<LocalHost, DiscordError> {
        let mut adapters = LocalHostAdapters::new(self.mcp_adapter.as_deref())
            .with_code_mode_worker(self.code_mode_worker.as_deref())
            .with_mcp_registry(self.mcp_registry_adapter.as_deref())
            .with_shared_plugin_registry(self.shared_plugin_registry.as_deref());
        if let Some(relay) = &self.oauth_relay {
            adapters = adapters.with_oauth_relay(&relay.origin, &relay.credentials);
        }
        LocalHost::new(home.path(), self.models, adapters).map_err(DiscordError::from)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_deployment_example_is_a_valid_runtime() {
        let example = include_str!("../../../deploy/renoa-discord.config.example.json");
        serde_json::from_str::<super::Runtime>(example).expect("runtime example");
    }
}
