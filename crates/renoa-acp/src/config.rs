use std::{collections::HashSet, env, path::PathBuf};

use renoa_local::{
    LocalHost, LocalHostAdapters, LocalHostError, ModelChoice, ModelProvider, discover_models,
};
use serde::Serialize;

use crate::ServerError;

const GITHUB_INTEGRATION_ID: &str = "github";
const GITHUB_CONNECTION_ID: &str = "github";
const GITHUB_ENDPOINT: &str = "https://api.githubcopilot.com/mcp/readonly";
const GITHUB_HOSTNAME: &str = "github.com";

/// Process configuration for the local ACP adapter.
pub struct Config {
    host: LocalHost,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalog {
    models: Vec<CatalogModel>,
}

#[derive(Serialize)]
pub struct GitHubMcpInstallation {
    connection_id: &'static str,
    endpoint: &'static str,
    account: String,
    tool_count: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CatalogModel {
    id: String,
    name: String,
    is_default: bool,
    reasoning_levels: Vec<CatalogReasoningLevel>,
    default_reasoning: &'static str,
}

#[derive(Serialize)]
struct CatalogReasoningLevel {
    id: &'static str,
    name: &'static str,
}

struct ProviderSettings {
    bridge: PathBuf,
    providers: Vec<ModelProvider>,
    default_provider: ModelProvider,
    model: String,
    credential_store: PathBuf,
}

impl Config {
    /// Reads the local runtime and durable data paths from the process environment.
    ///
    /// # Errors
    ///
    /// Returns an error when required provider settings or a usable data directory are absent.
    pub fn from_environment() -> Result<Self, ServerError> {
        let data_directory = data_directory()?;
        let settings = ProviderSettings::from_environment()?;
        Ok(Self {
            host: LocalHost::new(
                data_directory,
                settings.bridge,
                settings.providers,
                settings.default_provider,
                settings.model,
                settings.credential_store,
                LocalHostAdapters::new(optional_path("RENOA_MCP_ADAPTER").as_deref())
                    .with_mcp_registry(optional_path("RENOA_MCP_REGISTRY_ADAPTER").as_deref()),
            )?,
        })
    }

    pub(crate) const fn host(&self) -> &LocalHost {
        &self.host
    }
}

impl ProviderSettings {
    fn from_environment() -> Result<Self, ServerError> {
        let default_provider = required_provider("RENOA_MODEL_PROVIDER")?;
        Ok(Self {
            bridge: required_path("RENOA_MODEL_BRIDGE")?,
            providers: enabled_providers(default_provider)?,
            default_provider,
            model: required("RENOA_MODEL")?,
            credential_store: required_path("RENOA_MODEL_AUTH_STORE")?,
        })
    }
}

impl ModelCatalog {
    fn from_models(
        models: Vec<ModelChoice>,
        default_provider: ModelProvider,
        configured_model: &str,
    ) -> Result<Self, ServerError> {
        if !models
            .iter()
            .any(|model| model.provider() == default_provider && model.id() == configured_model)
        {
            return Err(ServerError::Configuration(format!(
                "configured {default_provider}/{configured_model} model is not available from the authenticated provider"
            )));
        }
        let models = models
            .into_iter()
            .map(|model| {
                let default_reasoning = model.default_reasoning().ok_or_else(|| {
                    ServerError::Configuration(format!(
                        "{} has no supported reasoning level",
                        model.id()
                    ))
                })?;
                Ok(CatalogModel {
                    id: model.selection_id(),
                    name: format!("{} ({})", model.name(), model.provider().name()),
                    is_default: model.provider() == default_provider
                        && model.id() == configured_model,
                    reasoning_levels: model
                        .reasoning_levels()
                        .iter()
                        .map(|level| CatalogReasoningLevel {
                            id: level.as_str(),
                            name: level.name(),
                        })
                        .collect(),
                    default_reasoning: default_reasoning.as_str(),
                })
            })
            .collect::<Result<Vec<_>, ServerError>>()?;
        Ok(Self { models })
    }
}

/// Discovers the configured provider's current model catalog without creating
/// a Host session or touching Renoa's durable session directory.
///
/// # Errors
///
/// Returns an error when provider settings, authentication, or the catalog are invalid.
pub async fn configured_model_catalog() -> Result<ModelCatalog, ServerError> {
    let settings = ProviderSettings::from_environment()?;
    let mut models = Vec::new();
    for provider in &settings.providers {
        models.extend(
            discover_models(
                settings.bridge.clone(),
                *provider,
                settings.credential_store.clone(),
            )
            .await
            .map_err(LocalHostError::from)?,
        );
    }
    ModelCatalog::from_models(models, settings.default_provider, &settings.model)
}

/// Registers, authenticates, discovers, and enables Renoa's GitHub MCP connection.
///
/// The Host persists only the exact `gh` hostname and account reference. The
/// credential itself crosses only the MCP adapter's standard input.
///
/// # Errors
///
/// Returns configuration, credential, discovery, catalog, or selection failures.
pub async fn install_github_mcp(account: &str) -> Result<GitHubMcpInstallation, ServerError> {
    let config = Config::from_environment()?;
    config
        .host
        .register_gh_cli_mcp_connection(
            GITHUB_INTEGRATION_ID,
            GITHUB_CONNECTION_ID,
            GITHUB_ENDPOINT,
            GITHUB_HOSTNAME,
            account,
        )
        .await?;
    let catalog = config
        .host
        .refresh_mcp_catalog(GITHUB_CONNECTION_ID)
        .await?;
    config
        .host
        .enable_alpha_mcp_connection(GITHUB_CONNECTION_ID)
        .await?;
    Ok(GitHubMcpInstallation {
        connection_id: GITHUB_CONNECTION_ID,
        endpoint: GITHUB_ENDPOINT,
        account: account.to_owned(),
        tool_count: catalog.tools().len(),
    })
}

fn enabled_providers(default: ModelProvider) -> Result<Vec<ModelProvider>, ServerError> {
    let configured = env::var("RENOA_MODEL_PROVIDERS")
        .ok()
        .filter(|value| !value.is_empty());
    parse_enabled_providers(default, configured.as_deref())
}

fn parse_enabled_providers(
    default: ModelProvider,
    configured: Option<&str>,
) -> Result<Vec<ModelProvider>, ServerError> {
    let Some(configured) = configured else {
        return Ok(vec![default]);
    };
    let providers = configured
        .split(',')
        .map(str::trim)
        .map(|provider| {
            ModelProvider::from_id(provider).ok_or_else(|| {
                ServerError::Configuration(format!(
                    "RENOA_MODEL_PROVIDERS contains unsupported provider {provider}"
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if providers.iter().copied().collect::<HashSet<_>>().len() != providers.len() {
        return Err(ServerError::Configuration(
            "RENOA_MODEL_PROVIDERS must not repeat a provider".to_owned(),
        ));
    }
    if !providers.contains(&default) {
        return Err(ServerError::Configuration(format!(
            "RENOA_MODEL_PROVIDER {default} is absent from RENOA_MODEL_PROVIDERS"
        )));
    }
    Ok(providers)
}

fn required_provider(name: &str) -> Result<ModelProvider, ServerError> {
    let value = required(name)?;
    ModelProvider::from_id(&value).ok_or_else(|| {
        ServerError::Configuration(format!("{name} contains unsupported provider {value}"))
    })
}

fn required(name: &str) -> Result<String, ServerError> {
    env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ServerError::Configuration(format!("{name} must be set")))
}

fn required_path(name: &str) -> Result<PathBuf, ServerError> {
    required(name).map(PathBuf::from)
}

fn optional_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn data_directory() -> Result<PathBuf, ServerError> {
    if let Some(path) = env::var_os("RENOA_DATA_DIR").filter(|path| !path.is_empty()) {
        return absolute(PathBuf::from(path));
    }
    #[cfg(target_os = "macos")]
    {
        home_directory().map(|home| home.join("Library/Application Support/Renoa"))
    }
    #[cfg(target_os = "windows")]
    {
        return env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .map(|path| path.join("Renoa"))
            .ok_or_else(|| ServerError::Configuration("LOCALAPPDATA must be set".to_owned()));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(path) = env::var_os("XDG_DATA_HOME").filter(|path| !path.is_empty()) {
            return absolute(PathBuf::from(path).join("renoa"));
        }
        home_directory().map(|home| home.join(".local/share/renoa"))
    }
}

fn home_directory() -> Result<PathBuf, ServerError> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| ServerError::Configuration("HOME must be set".to_owned()))
}

fn absolute(path: PathBuf) -> Result<PathBuf, ServerError> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(env::current_dir()?.join(path))
    }
}

#[cfg(test)]
mod tests {
    use super::{ModelProvider, parse_enabled_providers};

    #[test]
    fn absent_provider_set_keeps_the_default_as_the_only_provider() {
        assert_eq!(
            parse_enabled_providers(ModelProvider::Xai, None).expect("default provider"),
            vec![ModelProvider::Xai]
        );
    }

    #[test]
    fn provider_set_is_ordered_unique_and_contains_the_default() {
        assert_eq!(
            parse_enabled_providers(ModelProvider::OpenCodeGo, Some("xai, opencode-go"),)
                .expect("two enabled providers"),
            vec![ModelProvider::Xai, ModelProvider::OpenCodeGo]
        );
        for invalid in ["xai,xai", "xai", "xai,"] {
            assert!(
                parse_enabled_providers(ModelProvider::OpenCodeGo, Some(invalid)).is_err(),
                "accepted invalid provider set {invalid}"
            );
        }
    }
}
