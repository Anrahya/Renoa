use std::{env, path::PathBuf};

use renoa_local::{LocalHost, LocalHostError, PiModelOption, discover_pi_models};
use serde::Serialize;

use crate::ServerError;

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
    provider: String,
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
                data_directory.join("sessions"),
                settings.bridge,
                settings.provider,
                settings.model,
                settings.credential_store,
            )?,
        })
    }

    pub(crate) const fn host(&self) -> &LocalHost {
        &self.host
    }
}

impl ProviderSettings {
    fn from_environment() -> Result<Self, ServerError> {
        Ok(Self {
            bridge: required_path("RENOA_PI_BRIDGE")?,
            provider: required("RENOA_PI_PROVIDER")?,
            model: required("RENOA_PI_MODEL")?,
            credential_store: required_path("RENOA_PI_AUTH_STORE")?,
        })
    }
}

impl ModelCatalog {
    fn from_models(
        models: Vec<PiModelOption>,
        configured_model: &str,
    ) -> Result<Self, ServerError> {
        if !models.iter().any(|model| model.id() == configured_model) {
            return Err(ServerError::Configuration(format!(
                "configured {configured_model} model is not available from the authenticated provider"
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
                    id: model.id().to_owned(),
                    name: model.name().to_owned(),
                    is_default: model.id() == configured_model,
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
    let models = discover_pi_models(
        settings.bridge,
        settings.provider,
        settings.credential_store,
    )
    .await
    .map_err(LocalHostError::from)?;
    ModelCatalog::from_models(models, &settings.model)
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
