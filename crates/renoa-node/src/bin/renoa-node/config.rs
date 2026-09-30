use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use renoa_control::{DeviceCredential, DeviceCredentials, DeviceId};
use renoa_local::{
    LocalHost, LocalHostAdapters, LocalModelConfiguration, ModelProvider, validate_code_mode_worker,
};
use serde::Deserialize;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

use crate::{
    error::ServiceError,
    private_file::{read_config, read_secret, require_absolute},
};

/// The config document shape this runtime reads.
///
/// Version 2 renamed each target's required `profile` to `agentId`. Version 3
/// removed each target's `sessionId`. Version 4 removed `targets`: every agent
/// in the node's Host is advertised as a target. Version 5 added the required
/// `automationCredentials`: the node runs the Host's automation schedule. An
/// earlier document is refused by version instead of failing as an unknown
/// field.
const CONFIG_SCHEMA_VERSION: u32 = 5;

pub(crate) struct LoadedConfig {
    pub(crate) endpoint: String,
    pub(crate) credentials: DeviceCredentials,
    /// The surface credential that submits the Host's automation runs.
    pub(crate) automation_credentials: DeviceCredentials,
    pub(crate) host: Arc<LocalHost>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConfigDocument {
    schema_version: u32,
    endpoint: String,
    automation_credentials: PathBuf,
    model: ModelDocument,
    #[serde(default)]
    adapters: AdapterDocument,
}

/// The schema a config document declares, read without this runtime's target
/// shape so a document from another schema is refused by version instead of as
/// a field that changed with it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VersionDocument {
    schema_version: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ModelDocument {
    bridge: PathBuf,
    credential_store: PathBuf,
    providers: Vec<ModelProvider>,
    default_provider: ModelProvider,
    default_model: String,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AdapterDocument {
    mcp: Option<PathBuf>,
    code_mode_worker: Option<PathBuf>,
    mcp_registry: Option<PathBuf>,
    shared_plugin_registry: Option<String>,
    oauth_relay: Option<OAuthRelayDocument>,
}

/// The Host's OAuth callback relay, needed when a plugin authorizes through a
/// browser on another device.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OAuthRelayDocument {
    origin: String,
    credentials: PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CredentialDocument {
    device_id: DeviceId,
    credential: DeviceCredential,
}

pub(crate) fn load(
    config_path: &Path,
    credentials_path: &Path,
    state_directory: &Path,
) -> Result<LoadedConfig, ServiceError> {
    let config = decode_config(config_path)?;
    let credentials = decode_credentials(credentials_path)?;
    require_absolute(
        &config.automation_credentials,
        "automation surface credential",
    )?;
    let automation_credentials = decode_credentials(&config.automation_credentials)?;
    if automation_credentials.device_id == credentials.device_id {
        return Err(ServiceError::Configuration(
            "automationCredentials must name a surface credential enrolled for automations, \
             not the node's own credential"
                .to_owned(),
        ));
    }
    validate_model(&config.model)?;
    validate_adapters(&config.adapters)?;
    config
        .endpoint
        .clone()
        .into_client_request()
        .map_err(|error| {
            ServiceError::Configuration(format!("invalid coordinator endpoint: {error}"))
        })?;
    let state_directory = prepare_state_directory(state_directory)?;
    let mut adapters = LocalHostAdapters::new(config.adapters.mcp.as_deref())
        .with_code_mode_worker(config.adapters.code_mode_worker.as_deref())
        .with_mcp_registry(config.adapters.mcp_registry.as_deref())
        .with_shared_plugin_registry(config.adapters.shared_plugin_registry.as_deref());
    if let Some(relay) = &config.adapters.oauth_relay {
        adapters = adapters.with_oauth_relay(&relay.origin, &relay.credentials);
    }

    let host = Arc::new(LocalHost::new(
        &state_directory,
        LocalModelConfiguration::new(
            &config.model.bridge,
            config.model.providers,
            config.model.default_provider,
            config.model.default_model,
            &config.model.credential_store,
        ),
        adapters,
    )?);
    Ok(LoadedConfig {
        endpoint: config.endpoint,
        credentials,
        automation_credentials,
        host,
    })
}

fn decode_config(path: &Path) -> Result<ConfigDocument, ServiceError> {
    let bytes = read_config(path)?;
    let config: ConfigDocument = match serde_json::from_slice(&bytes) {
        Ok(config) => config,
        Err(source) => {
            if let Some(version) = declared_schema_version(&bytes)
                && version != CONFIG_SCHEMA_VERSION
            {
                return Err(ServiceError::Configuration(unsupported_schema(version)));
            }
            return Err(ServiceError::Json {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if config.schema_version != CONFIG_SCHEMA_VERSION {
        return Err(ServiceError::Configuration(unsupported_schema(
            config.schema_version,
        )));
    }
    if config.endpoint.is_empty() {
        return Err(ServiceError::Configuration(
            "coordinator endpoint must not be empty".to_owned(),
        ));
    }
    Ok(config)
}

fn declared_schema_version(bytes: &[u8]) -> Option<u32> {
    let document: VersionDocument = serde_json::from_slice(bytes).ok()?;
    Some(document.schema_version)
}

fn unsupported_schema(version: u32) -> String {
    let cutover = match version {
        1 => {
            "; version 1 selected each target with `profile`, and the current schema selects it \
             with `agentId`"
        }
        2 | 3 => {
            "; the current schema advertises every agent in the node's Host as a target: remove \
             `targets`"
        }
        4 => {
            "; the node now runs the Host's automation schedule: enroll a surface named \
             `automations` for the Host's owner, claim it with `renoa-node enroll`, and name its \
             credential file in `automationCredentials`"
        }
        _ => "",
    };
    format!("unsupported node config schema {version}; expected {CONFIG_SCHEMA_VERSION}{cutover}")
}

fn decode_credentials(path: &Path) -> Result<DeviceCredentials, ServiceError> {
    let bytes = read_secret(path)?;
    let document: CredentialDocument =
        serde_json::from_slice(&bytes).map_err(|source| ServiceError::Json {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(DeviceCredentials {
        device_id: document.device_id,
        credential: document.credential,
    })
}

fn prepare_state_directory(path: &Path) -> Result<PathBuf, ServiceError> {
    let home = renoa_local::RenoaHome::at(path)
        .map_err(|error| ServiceError::Configuration(error.to_string()))?;
    home.initialize()
        .map_err(|error| ServiceError::file("initialize", path, error))?;
    Ok(home.path().to_path_buf())
}

fn validate_model(model: &ModelDocument) -> Result<(), ServiceError> {
    require_regular_absolute(&model.bridge, "model bridge")?;
    require_regular_absolute(&model.credential_store, "model credential store")?;
    if model.default_model.trim().is_empty() {
        return Err(ServiceError::Configuration(
            "default model must not be empty".to_owned(),
        ));
    }
    Ok(())
}

fn validate_adapters(adapters: &AdapterDocument) -> Result<(), ServiceError> {
    if let Some(path) = &adapters.mcp {
        require_regular_absolute(path, "MCP adapter")?;
    }
    if let Some(path) = &adapters.code_mode_worker {
        validate_code_mode_worker(path).map_err(ServiceError::Configuration)?;
    }
    if let Some(path) = &adapters.mcp_registry {
        require_regular_absolute(path, "MCP Registry adapter")?;
    }
    if let Some(relay) = &adapters.oauth_relay {
        require_regular_absolute(&relay.credentials, "OAuth relay device credential")?;
    }
    Ok(())
}

fn require_regular_absolute(path: &Path, label: &str) -> Result<(), ServiceError> {
    require_absolute(path, label)?;
    let metadata =
        std::fs::metadata(path).map_err(|error| ServiceError::file("inspect", path, error))?;
    if !metadata.is_file() {
        return Err(ServiceError::Configuration(format!(
            "{label} `{}` must name a regular file",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
