use std::{
    collections::{BTreeSet, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use renoa_control::{DeviceCredential, DeviceCredentials, DeviceId};
use renoa_local::{
    ALPHA_PROFILE_ID, ARCEE_PROFILE_ID, AgentProfile, AgentProfileId, LocalHost, LocalHostAdapters,
    LocalModelConfiguration, ModelProvider, alpha_profile, arcee_profile,
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

const CONFIG_SCHEMA_VERSION: u32 = 1;

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
    mcp_registry: Option<PathBuf>,
    shared_plugin_registry: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TargetDocument {
    target: String,
    profile: AgentProfileId,
    session_id: Uuid,
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
    let profile_ids = profile_ids_for(&config.targets)?;
    validate_target_uniqueness(&config.targets)?;
    let targets = build_targets(config.targets)?;
    let state_directory = prepare_state_directory(state_directory)?;
    let profiles = build_profiles(profile_ids, &state_directory)?;

    let host = Arc::new(LocalHost::new(
        state_directory.join("host"),
        LocalModelConfiguration::new(
            &config.model.bridge,
            config.model.providers,
            config.model.default_provider,
            config.model.default_model,
            &config.model.credential_store,
        ),
        profiles,
        LocalHostAdapters::new(config.adapters.mcp.as_deref())
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
    let config: ConfigDocument =
        serde_json::from_slice(&bytes).map_err(|source| ServiceError::Json {
            path: path.to_path_buf(),
            source,
        })?;
    if config.schema_version != CONFIG_SCHEMA_VERSION {
        return Err(ServiceError::Configuration(format!(
            "node config schemaVersion must be {CONFIG_SCHEMA_VERSION}"
        )));
    }
    if config.endpoint.is_empty() {
        return Err(ServiceError::Configuration(
            "coordinator endpoint must not be empty".to_owned(),
        ));
    }
    Ok(config)
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
    require_absolute(path, "state directory")?;
    std::fs::create_dir_all(path).map_err(|error| ServiceError::file("create", path, error))?;
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| ServiceError::file("inspect", path, error))?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(ServiceError::Configuration(
            "state directory must name a real directory, not a symbolic link".to_owned(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| ServiceError::file("protect", path, error))?;
    }
    let path =
        std::fs::canonicalize(path).map_err(|error| ServiceError::file("resolve", path, error))?;
    if !path.is_dir() {
        return Err(ServiceError::Configuration(
            "state directory must name a directory".to_owned(),
        ));
    }
    Ok(path)
}

fn profile_ids_for(targets: &[TargetDocument]) -> Result<Vec<AgentProfileId>, ServiceError> {
    if targets.is_empty() {
        return Err(ServiceError::Configuration(
            "at least one Host target must be configured".to_owned(),
        ));
    }
    let profile_ids = targets
        .iter()
        .map(|target| target.profile.clone())
        .collect::<BTreeSet<_>>();
    for profile_id in &profile_ids {
        if !matches!(profile_id.as_str(), ALPHA_PROFILE_ID | ARCEE_PROFILE_ID) {
            return Err(ServiceError::Configuration(format!(
                "node target references unsupported built-in profile `{profile_id}`"
            )));
        }
    }
    Ok(profile_ids.into_iter().collect())
}

fn build_profiles(
    profile_ids: Vec<AgentProfileId>,
    state_directory: &Path,
) -> Result<Vec<AgentProfile>, ServiceError> {
    let mut profiles = Vec::with_capacity(profile_ids.len());
    for profile_id in profile_ids {
        match profile_id.as_str() {
            ALPHA_PROFILE_ID => profiles.push(alpha_profile()),
            ARCEE_PROFILE_ID => profiles
                .push(arcee_profile(state_directory).map_err(renoa_local::LocalHostError::from)?),
            _ => {
                return Err(ServiceError::Configuration(format!(
                    "node target references unsupported built-in profile `{profile_id}`"
                )));
            }
        }
    }
    Ok(profiles)
}

fn validate_target_uniqueness(targets: &[TargetDocument]) -> Result<(), ServiceError> {
    let mut target_names = HashSet::new();
    let mut sessions = HashSet::new();
    for target in targets {
        if !target_names.insert(target.target.as_str()) {
            return Err(ServiceError::Configuration(format!(
                "Host target `{}` is configured more than once",
                target.target
            )));
        }
        if !sessions.insert(target.session_id) {
            return Err(ServiceError::Configuration(format!(
                "Host session {} is configured for more than one target",
                target.session_id
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
                target.profile,
                target.session_id,
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
mod tests {
    use serde_json::json;

    use super::*;

    #[cfg(unix)]
    fn private(path: &Path) {
        use std::os::unix::fs::PermissionsExt as _;

        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .expect("protect test file");
    }

    #[test]
    fn config_is_versioned_strict_and_references_known_profiles() {
        let files = tempfile::tempdir().expect("temporary directory");
        let bridge = files.path().join("bridge.mjs");
        let credential_store = files.path().join("model.sqlite");
        let workspace = files.path().join("workspace");
        std::fs::write(&bridge, "").expect("write bridge");
        std::fs::write(&credential_store, "").expect("write model store");
        std::fs::create_dir(&workspace).expect("create workspace");
        let base = json!({
            "schemaVersion": 1,
            "endpoint": "ws://127.0.0.1:9/connect",
            "model": {
                "bridge": bridge,
                "credentialStore": credential_store,
                "providers": ["xai"],
                "defaultProvider": "xai",
                "defaultModel": "fixture-model"
            },
            "targets": [{
                "target": "workspace:test",
                "profile": ALPHA_PROFILE_ID,
                "sessionId": Uuid::new_v4(),
                "workspace": workspace
            }]
        });
        let path = files.path().join("node.json");
        std::fs::write(&path, serde_json::to_vec(&base).expect("encode config"))
            .expect("write config");
        #[cfg(unix)]
        private(&path);
        let decoded = decode_config(&path).expect("decode strict config");
        assert_eq!(decoded.targets[0].profile.as_str(), ALPHA_PROFILE_ID);

        let mut unsupported_profile = base.clone();
        unsupported_profile["targets"][0]["profile"] = json!("renoa.unknown.v1");
        std::fs::write(
            &path,
            serde_json::to_vec(&unsupported_profile).expect("encode config"),
        )
        .expect("write unsupported profile config");
        let unsupported_profile = decode_config(&path).expect("decode profile ID");
        assert!(profile_ids_for(&unsupported_profile.targets).is_err());

        let mut unknown = base.clone();
        unknown["unexpected"] = json!(true);
        std::fs::write(&path, serde_json::to_vec(&unknown).expect("encode config"))
            .expect("write unknown config");
        assert!(decode_config(&path).is_err());

        let mut wrong_version = base;
        wrong_version["schemaVersion"] = json!(2);
        std::fs::write(
            &path,
            serde_json::to_vec(&wrong_version).expect("encode config"),
        )
        .expect("write wrong version config");
        assert!(decode_config(&path).is_err());
    }

    #[test]
    fn credential_document_rejects_unknown_fields() {
        let files = tempfile::tempdir().expect("temporary directory");
        let path = files.path().join("device.json");
        let credential = json!({
            "deviceId": Uuid::new_v4(),
            "credential": "00".repeat(32),
            "unexpected": true
        });
        std::fs::write(
            &path,
            serde_json::to_vec(&credential).expect("encode credential"),
        )
        .expect("write credential");
        #[cfg(unix)]
        private(&path);

        assert!(decode_credentials(&path).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn state_directory_symlink_is_rejected_without_changing_its_target() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let files = tempfile::tempdir().expect("temporary directory");
        let real = files.path().join("real");
        let linked = files.path().join("linked");
        std::fs::create_dir(&real).expect("create real state directory");
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o755))
            .expect("set initial mode");
        symlink(&real, &linked).expect("link state directory");

        assert!(prepare_state_directory(&linked).is_err());
        assert_eq!(
            std::fs::metadata(&real)
                .expect("real directory metadata")
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
    }
}
