use std::path::{Path, PathBuf};

use renoa_local::{
    LocalHost, LocalHostAdapters, LocalModelConfiguration, ModelProvider, ReasoningLevel,
    arcee_profile,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::SlackError;

/// Local launch settings. Secret values are loaded from private files, never JSON.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub(crate) data_directory: PathBuf,
    pub(crate) workspace: PathBuf,
    pub(crate) agent_id: Uuid,
    pub(crate) allowed_user_id: String,
    pub(crate) bot_token_file: PathBuf,
    pub(crate) app_token_file: PathBuf,
    model_bridge: PathBuf,
    providers: Vec<ModelProvider>,
    provider: ModelProvider,
    model: String,
    reasoning: Option<ReasoningLevel>,
    model_auth_store: PathBuf,
    mcp_adapter: Option<PathBuf>,
    mcp_registry_adapter: Option<PathBuf>,
    shared_plugin_registry: Option<String>,
    oauth_relay: Option<Relay>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Relay {
    origin: String,
    device_credential_file: PathBuf,
}

impl Config {
    /// Reads and validates a launch file without authenticating to Slack.
    ///
    /// # Errors
    /// Returns malformed settings or filesystem failures.
    pub fn read(path: &Path) -> Result<Self, SlackError> {
        let mut config: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        for path in [
            &config.data_directory,
            &config.workspace,
            &config.model_bridge,
            &config.model_auth_store,
            &config.bot_token_file,
            &config.app_token_file,
        ]
        .into_iter()
        .chain(config.mcp_adapter.iter())
        .chain(config.mcp_registry_adapter.iter())
        {
            if !path.is_absolute() {
                return Err(SlackError::Invalid(
                    "configured paths must be absolute".to_owned(),
                ));
            }
        }
        if !crate::ingress::valid_id(&config.allowed_user_id, b"UW") {
            return Err(SlackError::Invalid(
                "allowed_user_id must be a Slack member ID".to_owned(),
            ));
        }
        std::fs::create_dir_all(&config.data_directory)?;
        config.data_directory = std::fs::canonicalize(&config.data_directory)?;
        config.workspace = std::fs::canonicalize(&config.workspace)?;
        if !config.workspace.is_dir() {
            return Err(SlackError::Invalid(
                "workspace must be a directory".to_owned(),
            ));
        }
        Ok(config)
    }

    pub(crate) async fn preflight(&self) -> Result<(), SlackError> {
        if self.provider != ModelProvider::OpenCodeGo
            || !self.providers.contains(&ModelProvider::OpenCodeGo)
        {
            return Err(SlackError::Invalid(
                "Arcee requires opencode-go as its initial provider".to_owned(),
            ));
        }
        if !self.model_bridge.is_file() || !self.model_auth_store.is_file() {
            return Err(SlackError::Invalid(
                "model bridge and auth store must be existing files".to_owned(),
            ));
        }
        let models = renoa_local::discover_models(
            self.model_bridge.clone(),
            ModelProvider::OpenCodeGo,
            self.model_auth_store.clone(),
        )
        .await
        .map_err(renoa_local::LocalHostError::from)?;
        let model = models
            .iter()
            .find(|model| model.id() == self.model)
            .ok_or_else(|| {
                SlackError::Invalid(
                    "configured model is absent from the Arcee provider catalog".to_owned(),
                )
            })?;
        if let Some(reasoning) = self.reasoning
            && !model.reasoning_levels().contains(&reasoning)
        {
            return Err(SlackError::Invalid(
                "configured reasoning is unsupported by this model".to_owned(),
            ));
        }
        Ok(())
    }

    pub(crate) fn host(&self) -> Result<LocalHost, SlackError> {
        let mut models = LocalModelConfiguration::new(
            &self.model_bridge,
            self.providers.clone(),
            self.provider,
            &self.model,
            &self.model_auth_store,
        );
        if let Some(reasoning) = self.reasoning {
            models = models.with_initial_reasoning(reasoning);
        }
        let mut adapters = LocalHostAdapters::new(self.mcp_adapter.as_deref())
            .with_mcp_registry(self.mcp_registry_adapter.as_deref())
            .with_shared_plugin_registry(self.shared_plugin_registry.as_deref());
        if let Some(relay) = &self.oauth_relay {
            adapters = adapters.with_oauth_relay(&relay.origin, &relay.device_credential_file);
        }
        let profile =
            arcee_profile(&self.data_directory).map_err(renoa_local::LocalHostError::from)?;
        Ok(LocalHost::new(
            &self.data_directory,
            models,
            vec![profile],
            adapters,
        )?)
    }
}

pub(crate) fn read_token(path: &Path, prefix: &str) -> Result<String, SlackError> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > 4096 {
        return Err(SlackError::Invalid(
            "token must be a small regular file".to_owned(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(SlackError::Invalid(
                "token files must be private (chmod 600)".to_owned(),
            ));
        }
    }
    let token = std::fs::read_to_string(path)?
        .trim_end_matches(['\r', '\n'])
        .to_owned();
    if !token.starts_with(prefix)
        || token.len() <= prefix.len()
        || token.chars().any(char::is_whitespace)
    {
        return Err(SlackError::Invalid(format!(
            "token file must contain a {prefix} token"
        )));
    }
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn preflight_resolves_the_actual_catalog_and_inspection_needs_no_execution_files() {
        let directory = tempfile::tempdir().expect("directory");
        let bridge = directory.path().join("bridge.mjs");
        let auth = directory.path().join("auth.sqlite");
        std::fs::write(&bridge, include_str!("tests/bridge.mjs")).expect("bridge");
        std::fs::write(&auth, "").expect("auth");
        let config_path = directory.path().join("slack.json");
        std::fs::write(&config_path,serde_json::json!({
            "data_directory":directory.path(),"workspace":directory.path(),"agent_id":Uuid::nil(),
            "allowed_user_id":"U3","bot_token_file":directory.path().join("bot"),"app_token_file":directory.path().join("app"),
            "model_bridge":bridge,"providers":["opencode-go"],"provider":"opencode-go","model":"fixture","model_auth_store":auth
        }).to_string()).expect("config");
        let mut config = Config::read(&config_path).expect("read config");
        config.preflight().await.expect("real model catalog");
        config.model = "absent".to_owned();
        assert!(config.preflight().await.is_err());
        config.model = "fixture".to_owned();
        config.providers = vec![ModelProvider::Xai];
        assert!(config.preflight().await.is_err());
        config.providers = vec![ModelProvider::OpenCodeGo];
        let store = crate::store::Store::open(
            directory.path(),
            &crate::store::Binding {
                host_id: Uuid::nil(),
                agent_id: Uuid::nil(),
                team: "T1",
                bot: "U2",
                user: "U3",
                workspace: directory.path(),
            },
        )
        .expect("store");
        std::fs::remove_file(&bridge).expect("disable execution");
        assert!(config.preflight().await.is_err());
        let inspection = crate::inspect(&Config::read(&config_path).expect("read without model"))
            .expect("inspect live database without model or daemon lease");
        assert_eq!(inspection["requests"], serde_json::json!([]));
        drop(store);
    }

    #[cfg(unix)]
    #[test]
    fn token_files_reject_shared_permissions_and_wrong_token_types() {
        use std::os::unix::fs::PermissionsExt as _;
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("token");
        std::fs::write(&path, "xoxb-secret\n").expect("token");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .expect("public mode");
        assert!(read_token(&path, "xoxb-").is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .expect("private mode");
        assert_eq!(
            read_token(&path, "xoxb-").expect("private bot token"),
            "xoxb-secret"
        );
        assert!(read_token(&path, "xapp-").is_err());
    }
}
