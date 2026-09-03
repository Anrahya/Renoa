use std::{collections::HashSet, env, path::PathBuf};

use renoa_local::{
    ARCEE_PROFILE_ID, AgentProfileId, LocalHost, LocalHostAdapters, LocalModelConfiguration,
    ModelProvider, ReasoningLevel, arcee_profile,
};

use crate::TelegramServiceError;

const TOKEN_LIMIT: u64 = 4096;

pub struct Config {
    pub(crate) host: LocalHost,
    pub(crate) profile_id: AgentProfileId,
    pub(crate) data_directory: PathBuf,
    pub(crate) workspace: PathBuf,
    pub(crate) bot_token: String,
    pub(crate) allowed_user_id: i64,
    pub(crate) telegram_ipv4_only: bool,
}

struct ProviderSettings {
    bridge: PathBuf,
    providers: Vec<ModelProvider>,
    default_provider: ModelProvider,
    model: String,
    initial_reasoning: Option<ReasoningLevel>,
    credential_store: PathBuf,
}

impl Config {
    /// Builds Arcee and its Telegram surface from explicit service configuration.
    ///
    /// # Errors
    ///
    /// Rejects missing, ambiguous, relative, or unusable service settings.
    pub async fn from_environment() -> Result<Self, TelegramServiceError> {
        let data_directory = canonical_directory("RENOA_DATA_DIR", true)?;
        let workspace = canonical_directory("RENOA_TELEGRAM_WORKSPACE", false)?;
        let allowed_user_id = required("RENOA_TELEGRAM_ALLOWED_USER_ID")?
            .parse::<i64>()
            .map_err(|_| configuration("RENOA_TELEGRAM_ALLOWED_USER_ID must be an integer"))?;
        if allowed_user_id <= 0 {
            return Err(configuration(
                "RENOA_TELEGRAM_ALLOWED_USER_ID must be positive",
            ));
        }
        let bot_token = read_token(required_path("RENOA_TELEGRAM_BOT_TOKEN_FILE")?).await?;
        let telegram_ipv4_only = optional_boolean("RENOA_TELEGRAM_IPV4_ONLY")?;
        let settings = ProviderSettings::from_environment()?;
        let shared_plugin_registry = optional("RENOA_SHARED_PLUGIN_REGISTRY")?;
        let oauth_relay = oauth_relay_settings(
            optional("RENOA_OAUTH_RELAY_ORIGIN")?,
            optional_path("RENOA_OAUTH_RELAY_DEVICE_CREDENTIAL_FILE"),
        )?;
        let mcp_adapter = optional_path("RENOA_MCP_ADAPTER");
        let mcp_registry_adapter = optional_path("RENOA_MCP_REGISTRY_ADAPTER");
        let profile = arcee_profile(&data_directory).map_err(renoa_local::LocalHostError::from)?;
        let profile_id = profile.id().clone();
        debug_assert_eq!(profile_id.as_str(), ARCEE_PROFILE_ID);
        let mut adapters = LocalHostAdapters::new(mcp_adapter.as_deref())
            .with_mcp_registry(mcp_registry_adapter.as_deref())
            .with_shared_plugin_registry(shared_plugin_registry.as_deref());
        if let Some((origin, credentials)) = oauth_relay.as_ref() {
            adapters = adapters.with_oauth_relay(origin, credentials);
        }
        let mut model_configuration = LocalModelConfiguration::new(
            settings.bridge,
            settings.providers,
            settings.default_provider,
            settings.model,
            settings.credential_store,
        );
        if let Some(reasoning) = settings.initial_reasoning {
            model_configuration = model_configuration.with_initial_reasoning(reasoning);
        }
        let host = LocalHost::new(
            &data_directory,
            model_configuration,
            vec![profile],
            adapters,
        )?;
        Ok(Self {
            host,
            profile_id,
            data_directory,
            workspace,
            bot_token,
            allowed_user_id,
            telegram_ipv4_only,
        })
    }
}

fn oauth_relay_settings(
    origin: Option<String>,
    credentials: Option<PathBuf>,
) -> Result<Option<(String, PathBuf)>, TelegramServiceError> {
    match (origin, credentials) {
        (Some(origin), Some(credentials)) => Ok(Some((origin, credentials))),
        (None, None) => Ok(None),
        (Some(_), None) | (None, Some(_)) => Err(configuration(
            "RENOA_OAUTH_RELAY_ORIGIN and RENOA_OAUTH_RELAY_DEVICE_CREDENTIAL_FILE must be set together",
        )),
    }
}

impl ProviderSettings {
    fn from_environment() -> Result<Self, TelegramServiceError> {
        let default_provider = required_provider("RENOA_MODEL_PROVIDER")?;
        Ok(Self {
            bridge: required_path("RENOA_MODEL_BRIDGE")?,
            providers: enabled_providers(default_provider)?,
            default_provider,
            model: required("RENOA_MODEL")?,
            initial_reasoning: optional_reasoning("RENOA_MODEL_REASONING")?,
            credential_store: required_path("RENOA_MODEL_AUTH_STORE")?,
        })
    }
}

async fn read_token(path: PathBuf) -> Result<String, TelegramServiceError> {
    let metadata = tokio::fs::symlink_metadata(&path).await?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(configuration(
            "RENOA_TELEGRAM_BOT_TOKEN_FILE must name a regular file",
        ));
    }
    if metadata.len() == 0 || metadata.len() > TOKEN_LIMIT {
        return Err(configuration(
            "RENOA_TELEGRAM_BOT_TOKEN_FILE has an invalid size",
        ));
    }
    require_private_token_file(&path, &metadata)?;
    let token = tokio::fs::read_to_string(path).await?;
    let token = token.trim_end_matches(['\r', '\n']).to_owned();
    if token.is_empty() || token.chars().any(char::is_whitespace) || !token.contains(':') {
        return Err(configuration(
            "RENOA_TELEGRAM_BOT_TOKEN_FILE does not contain one Bot API token",
        ));
    }
    Ok(token)
}

#[cfg(unix)]
fn require_private_token_file(
    path: &std::path::Path,
    metadata: &std::fs::Metadata,
) -> Result<(), TelegramServiceError> {
    let credentials_directory = env::var_os("CREDENTIALS_DIRECTORY").map(PathBuf::from);
    require_private_token_file_in(path, metadata, credentials_directory.as_deref())
}

#[cfg(unix)]
fn require_private_token_file_in(
    path: &std::path::Path,
    metadata: &std::fs::Metadata,
    credentials_directory: Option<&std::path::Path>,
) -> Result<(), TelegramServiceError> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let mode = metadata.permissions().mode() & 0o777;
    if mode.trailing_zeros() >= 6 {
        Ok(())
    } else if mode == 0o440 {
        let Some(directory) =
            credentials_directory.filter(|directory| path.parent() == Some(*directory))
        else {
            return Err(private_token_error());
        };
        let directory_metadata = std::fs::symlink_metadata(directory)?;
        let directory_mode = directory_metadata.permissions().mode() & 0o777;
        if directory_metadata.file_type().is_dir()
            && !directory_metadata.file_type().is_symlink()
            && directory_mode == 0o550
            && directory_metadata.uid() == metadata.uid()
            && directory_metadata.gid() == metadata.gid()
        {
            return Ok(());
        }
        Err(private_token_error())
    } else {
        Err(private_token_error())
    }
}

#[cfg(not(unix))]
fn require_private_token_file(
    _path: &std::path::Path,
    _metadata: &std::fs::Metadata,
) -> Result<(), TelegramServiceError> {
    Ok(())
}

#[cfg(unix)]
fn private_token_error() -> TelegramServiceError {
    configuration("RENOA_TELEGRAM_BOT_TOKEN_FILE must not be accessible by group or other users")
}

fn canonical_directory(name: &str, create: bool) -> Result<PathBuf, TelegramServiceError> {
    let path = required_path(name)?;
    if !path.is_absolute() {
        return Err(configuration(format!("{name} must be an absolute path")));
    }
    if create {
        std::fs::create_dir_all(&path)?;
    }
    let path = std::fs::canonicalize(path)?;
    if !path.is_dir() {
        return Err(configuration(format!("{name} must name a directory")));
    }
    Ok(path)
}

fn enabled_providers(default: ModelProvider) -> Result<Vec<ModelProvider>, TelegramServiceError> {
    let configured = env::var("RENOA_MODEL_PROVIDERS")
        .ok()
        .filter(|value| !value.is_empty());
    parse_enabled_providers(default, configured.as_deref())
}

fn parse_enabled_providers(
    default: ModelProvider,
    configured: Option<&str>,
) -> Result<Vec<ModelProvider>, TelegramServiceError> {
    let Some(configured) = configured else {
        return Ok(vec![default]);
    };
    let providers = configured
        .split(',')
        .map(str::trim)
        .map(|provider| {
            ModelProvider::from_id(provider).ok_or_else(|| {
                configuration(format!(
                    "RENOA_MODEL_PROVIDERS contains unsupported provider {provider}"
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if providers.is_empty()
        || providers.iter().copied().collect::<HashSet<_>>().len() != providers.len()
    {
        return Err(configuration(
            "RENOA_MODEL_PROVIDERS must be a nonempty unique provider list",
        ));
    }
    if !providers.contains(&default) {
        return Err(configuration(format!(
            "RENOA_MODEL_PROVIDER {default} is absent from RENOA_MODEL_PROVIDERS"
        )));
    }
    Ok(providers)
}

fn required_provider(name: &str) -> Result<ModelProvider, TelegramServiceError> {
    let value = required(name)?;
    ModelProvider::from_id(&value)
        .ok_or_else(|| configuration(format!("{name} contains unsupported provider {value}")))
}

fn required(name: &str) -> Result<String, TelegramServiceError> {
    env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| configuration(format!("{name} must be set")))
}

fn required_path(name: &str) -> Result<PathBuf, TelegramServiceError> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| configuration(format!("{name} must be set")))
}

fn optional_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn optional(name: &str) -> Result<Option<String>, TelegramServiceError> {
    match env::var(name) {
        Ok(value) if value.is_empty() => Ok(None),
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => {
            Err(configuration(format!("{name} must be valid Unicode")))
        }
    }
}

fn optional_boolean(name: &str) -> Result<bool, TelegramServiceError> {
    let configured = optional(name)?;
    parse_optional_boolean(name, configured.as_deref())
}

fn optional_reasoning(name: &str) -> Result<Option<ReasoningLevel>, TelegramServiceError> {
    let configured = optional(name)?;
    parse_optional_reasoning(name, configured.as_deref())
}

fn parse_optional_reasoning(
    name: &str,
    configured: Option<&str>,
) -> Result<Option<ReasoningLevel>, TelegramServiceError> {
    configured
        .map(|value| {
            ReasoningLevel::from_id(value).ok_or_else(|| {
                configuration(format!(
                    "{name} must be off, minimal, low, medium, high, xhigh, or max"
                ))
            })
        })
        .transpose()
}

fn parse_optional_boolean(
    name: &str,
    configured: Option<&str>,
) -> Result<bool, TelegramServiceError> {
    match configured {
        None | Some("0") => Ok(false),
        Some("1") => Ok(true),
        Some(_) => Err(configuration(format!("{name} must be 0 or 1"))),
    }
}

fn configuration(message: impl Into<String>) -> TelegramServiceError {
    TelegramServiceError::Configuration(message.into())
}

#[cfg(test)]
mod tests {
    use super::{
        ModelProvider, ReasoningLevel, oauth_relay_settings, parse_enabled_providers,
        parse_optional_boolean, parse_optional_reasoning,
    };
    #[cfg(unix)]
    use super::{read_token, require_private_token_file_in};

    #[test]
    fn provider_selection_is_explicit_unique_and_contains_the_default() {
        assert_eq!(
            parse_enabled_providers(ModelProvider::Xai, None).expect("default provider"),
            vec![ModelProvider::Xai]
        );
        assert_eq!(
            parse_enabled_providers(ModelProvider::OpenCodeGo, Some("xai,opencode-go"))
                .expect("provider list"),
            vec![ModelProvider::Xai, ModelProvider::OpenCodeGo]
        );
        for invalid in ["", "xai,xai", "xai,"] {
            assert!(parse_enabled_providers(ModelProvider::OpenCodeGo, Some(invalid)).is_err());
        }
    }

    #[test]
    fn oauth_relay_origin_and_device_credential_are_atomic_configuration() {
        let credentials = std::path::PathBuf::from("/run/credentials/relay-device");
        assert!(
            oauth_relay_settings(None, None)
                .expect("disabled relay")
                .is_none()
        );
        assert!(oauth_relay_settings(Some("https://renoa.live".to_owned()), None).is_err());
        assert!(oauth_relay_settings(None, Some(credentials.clone())).is_err());
        assert_eq!(
            oauth_relay_settings(Some("https://renoa.live".to_owned()), Some(credentials))
                .expect("complete relay configuration"),
            Some((
                "https://renoa.live".to_owned(),
                std::path::PathBuf::from("/run/credentials/relay-device")
            ))
        );
    }

    #[test]
    fn telegram_ip_family_override_is_strict() {
        assert!(!parse_optional_boolean("IP_FAMILY", None).expect("default address families"));
        assert!(!parse_optional_boolean("IP_FAMILY", Some("0")).expect("dual stack"));
        assert!(parse_optional_boolean("IP_FAMILY", Some("1")).expect("IPv4 only"));
        assert!(parse_optional_boolean("IP_FAMILY", Some("true")).is_err());
    }

    #[test]
    fn initial_reasoning_is_optional_and_strict() {
        assert_eq!(
            parse_optional_reasoning("RENOA_MODEL_REASONING", None)
                .expect("default model reasoning"),
            None
        );
        assert_eq!(
            parse_optional_reasoning("RENOA_MODEL_REASONING", Some("xhigh"))
                .expect("configured model reasoning"),
            Some(ReasoningLevel::Xhigh)
        );
        assert!(parse_optional_reasoning("RENOA_MODEL_REASONING", Some("extra-high")).is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bot_token_file_must_be_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().expect("temporary token directory");
        let path = directory.path().join("telegram-token");
        std::fs::write(&path, "123:secret\n").expect("write token");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .expect("make token public");
        assert!(read_token(path.clone()).await.is_err());

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .expect("make token private");
        assert_eq!(
            read_token(path).await.expect("read private token"),
            "123:secret"
        );
    }

    #[cfg(unix)]
    #[test]
    fn systemd_credential_mount_permissions_are_private() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().expect("temporary credential root");
        let directory = root.path().join("renoa-telegram.service");
        std::fs::create_dir(&directory).expect("create credential directory");
        let path = directory.join("telegram-bot-token");
        std::fs::write(&path, "123:secret\n").expect("write credential");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o440))
            .expect("set systemd credential permissions");
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o550))
            .expect("set credential directory permissions");
        let metadata = std::fs::symlink_metadata(&path).expect("read credential metadata");

        require_private_token_file_in(&path, &metadata, Some(&directory))
            .expect("accept private systemd credential mount");
        assert!(require_private_token_file_in(&path, &metadata, None).is_err());
    }
}
