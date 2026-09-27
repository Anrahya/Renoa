use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
};

use renoa_control::{DeviceCredential, DeviceCredentials, DeviceId};
use renoa_kernel::AgentId;
use renoa_local::{
    LocalHost, LocalHostAdapters, LocalModelConfiguration, ModelProvider, validate_code_mode_worker,
};
use renoa_node::HostTarget;
use renoa_protocol::TargetRef;
use serde::Deserialize;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use uuid::Uuid;

use crate::{
    error::ServiceError,
    private_file::{read_config, read_secret, require_absolute},
};

/// The config document shape this runtime reads.
///
/// Version 2 renamed each target's required `profile` to `agentId`. Version 3
/// removed each target's `sessionId`: every task opened on a target receives
/// its own Host session, recorded in the node ledger. An earlier document is
/// refused by version instead of failing as an unknown field.
const CONFIG_SCHEMA_VERSION: u32 = 3;

pub(crate) struct LoadedConfig {
    pub(crate) endpoint: String,
    pub(crate) credentials: DeviceCredentials,
    pub(crate) host: Arc<LocalHost>,
    pub(crate) targets: Vec<HostTarget>,
    pub(crate) state_directory: PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConfigDocument {
    schema_version: u32,
    endpoint: String,
    model: ModelDocument,
    #[serde(default)]
    adapters: AdapterDocument,
    targets: Vec<TargetDocument>,
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
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TargetDocument {
    target: String,
    agent_id: Uuid,
    workspace: PathBuf,
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
    validate_model(&config.model)?;
    validate_adapters(&config.adapters)?;
    config
        .endpoint
        .clone()
        .into_client_request()
        .map_err(|error| {
            ServiceError::Configuration(format!("invalid coordinator endpoint: {error}"))
        })?;
    validate_target_uniqueness(&config.targets)?;
    let targets = build_targets(config.targets)?;
    let state_directory = prepare_state_directory(state_directory)?;

    let host = Arc::new(LocalHost::new(
        &state_directory,
        LocalModelConfiguration::new(
            &config.model.bridge,
            config.model.providers,
            config.model.default_provider,
            config.model.default_model,
            &config.model.credential_store,
        ),
        LocalHostAdapters::new(config.adapters.mcp.as_deref())
            .with_code_mode_worker(config.adapters.code_mode_worker.as_deref())
            .with_mcp_registry(config.adapters.mcp_registry.as_deref())
            .with_shared_plugin_registry(config.adapters.shared_plugin_registry.as_deref()),
    )?);
    Ok(LoadedConfig {
        endpoint: config.endpoint,
        credentials,
        host,
        targets,
        state_directory,
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
        2 => {
            "; version 2 bound each target to one `sessionId`, and the current schema gives every \
             task its own Host session: remove each target's `sessionId`"
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

fn validate_target_uniqueness(targets: &[TargetDocument]) -> Result<(), ServiceError> {
    if targets.is_empty() {
        return Err(ServiceError::Configuration(
            "at least one Host target must be configured".to_owned(),
        ));
    }
    let mut target_names = HashSet::new();
    for target in targets {
        if !target_names.insert(target.target.as_str()) {
            return Err(ServiceError::Configuration(format!(
                "Host target `{}` is configured more than once",
                target.target
            )));
        }
    }
    Ok(())
}

fn build_targets(targets: Vec<TargetDocument>) -> Result<Vec<HostTarget>, ServiceError> {
    targets
        .into_iter()
        .map(|target| {
            HostTarget::new(
                &TargetRef::new(target.target),
                AgentId::from_uuid(target.agent_id),
                target.workspace,
            )
            .map_err(ServiceError::from)
        })
        .collect()
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
