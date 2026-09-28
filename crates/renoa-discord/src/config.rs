use std::{fs, path::PathBuf};

use renoa_control::DeviceCredentials;
use serde::Deserialize;

use crate::{DiscordError, connection::Connection};
use renoa_local::RenoaHome;

const CREDENTIAL_LIMIT: u64 = 16 * 1024;

/// The worker's launch: its trusted runtime file and the owner's connection.
pub(crate) struct Config {
    pub(crate) home: RenoaHome,
    pub(crate) connection: Connection,
    pub(crate) rcp: Rcp,
}

/// Where Discord reaches the Host's agents: the RCP coordinator and this
/// surface's enrolled device credential.
pub(crate) struct Rcp {
    pub(crate) endpoint: String,
    pub(crate) credentials: DeviceCredentials,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Runtime {
    #[serde(default)]
    home: PathBuf,
    rcp: RcpDocument,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RcpDocument {
    endpoint: String,
    credentials: PathBuf,
}

impl Config {
    /// Reads the runtime file, the surface's RCP credential, then the
    /// connection the owner committed.
    ///
    /// # Errors
    ///
    /// Returns malformed settings, a relative, shared, or unreadable credential
    /// file, a missing connection, or an invalid Renoa home.
    pub(crate) fn read(path: &std::path::Path) -> Result<Self, DiscordError> {
        let runtime: Runtime = serde_json::from_slice(&fs::read(path)?)?;
        let home = RenoaHome::resolve(Some(runtime.home))?;
        let credentials = read_credentials(&runtime.rcp.credentials)?;
        let connection = Connection::read(&home)?.ok_or_else(|| {
            DiscordError::Invalid("Connect Discord from the Control Room first".to_owned())
        })?;
        Ok(Self {
            home,
            connection,
            rcp: Rcp {
                endpoint: runtime.rcp.endpoint,
                credentials,
            },
        })
    }
}

fn read_credentials(path: &std::path::Path) -> Result<DeviceCredentials, DiscordError> {
    if !path.is_absolute() {
        return Err(DiscordError::Invalid(
            "The Discord RCP credential path must be absolute".to_owned(),
        ));
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.len() > CREDENTIAL_LIMIT {
        return Err(DiscordError::Invalid(
            "The Discord RCP credential must be a bounded regular file".to_owned(),
        ));
    }
    crate::connection::require_private(&metadata)?;
    serde_json::from_slice(&fs::read(path)?)
        .map_err(|_| DiscordError::Invalid("The Discord RCP credential is malformed".to_owned()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_deployment_example_is_a_valid_runtime() {
        let example = include_str!("../../../deploy/renoa-discord.config.example.json");
        serde_json::from_str::<super::Runtime>(example).expect("runtime example");
    }
}
