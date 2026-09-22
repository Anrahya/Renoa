use std::{fs, path::PathBuf};

use serde::Deserialize;
use uuid::Uuid;

use crate::{DiscordError, snowflake::Snowflake};
use renoa_local::{LocalHost, LocalHostAdapters, LocalModelConfiguration, ModelProvider};

const TOKEN_LIMIT: u64 = 4096;

pub(crate) struct Config {
    pub(crate) data_directory: PathBuf,
    pub(crate) guild_id: Snowflake,
    pub(crate) operator_user_id: Snowflake,
    pub(crate) agent_id: Uuid,
    pub(crate) token: String,
    pub(crate) workspace: PathBuf,
    pub(crate) model_bridge: PathBuf,
    pub(crate) model_auth_store: PathBuf,
    pub(crate) model: String,
    pub(crate) provider: ModelProvider,
    pub(crate) mcp_adapter: Option<PathBuf>,
    pub(crate) code_mode_worker: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchFile {
    data_directory: PathBuf,
    guild_id: String,
    operator_user_id: String,
    agent_id: String,
    bot_token_file: PathBuf,
    workspace: PathBuf,
    model_bridge: PathBuf,
    model_auth_store: PathBuf,
    model: String,
    provider: ModelProvider,
    mcp_adapter: Option<PathBuf>,
    code_mode_worker: Option<PathBuf>,
}

impl Config {
    /// Reads a Discord launch file and checks the bot token file.
    ///
    /// `agent_id` names an existing Host agent. This surface does not create
    /// that agent. The token stays in memory for the Gateway and is not written
    /// to the surface database.
    ///
    /// # Errors
    ///
    /// Returns malformed settings, a relative path, an invalid snowflake or
    /// agent id, or a token file that is missing, empty, or readable by other users.
    pub(crate) fn read(path: &std::path::Path) -> Result<Self, DiscordError> {
        let file: LaunchFile = serde_json::from_slice(&fs::read(path)?)?;
        if !file.data_directory.is_absolute() || !file.bot_token_file.is_absolute() {
            return Err(DiscordError::Invalid(
                "data_directory and bot_token_file must be absolute paths".to_owned(),
            ));
        }
        let guild_id = Snowflake::parse(&file.guild_id)?;
        let operator_user_id = Snowflake::parse(&file.operator_user_id)?;
        let agent_id = Uuid::parse_str(&file.agent_id).map_err(|_| {
            DiscordError::Invalid("agent_id must be the UUID of an existing agent".to_owned())
        })?;
        for path in [&file.workspace, &file.model_bridge, &file.model_auth_store]
            .into_iter()
            .chain(file.mcp_adapter.iter())
            .chain(file.code_mode_worker.iter())
        {
            if !path.is_absolute() {
                return Err(DiscordError::Invalid(
                    "workspace, model_bridge, and model_auth_store must be absolute paths"
                        .to_owned(),
                ));
            }
        }
        if file.model.is_empty() {
            return Err(DiscordError::Invalid("model must not be empty".to_owned()));
        }
        let token = validate_token_file(&file.bot_token_file)?;
        Ok(Self {
            data_directory: file.data_directory,
            guild_id,
            operator_user_id,
            agent_id,
            token,
            workspace: file.workspace,
            model_bridge: file.model_bridge,
            model_auth_store: file.model_auth_store,
            model: file.model,
            provider: file.provider,
            mcp_adapter: file.mcp_adapter,
            code_mode_worker: file.code_mode_worker,
        })
    }

    pub(crate) fn open_host(&self) -> Result<LocalHost, DiscordError> {
        if !self.workspace.is_dir()
            || !self.model_bridge.is_file()
            || !self.model_auth_store.is_file()
        {
            return Err(DiscordError::Invalid(
                "workspace must be a directory and the model bridge and auth store must be files"
                    .to_owned(),
            ));
        }
        LocalHost::new(
            &self.data_directory,
            LocalModelConfiguration::new(
                &self.model_bridge,
                vec![self.provider],
                self.provider,
                &self.model,
                &self.model_auth_store,
            ),
            LocalHostAdapters::new(self.mcp_adapter.as_deref())
                .with_code_mode_worker(self.code_mode_worker.as_deref()),
        )
        .map_err(DiscordError::from)
    }
}

fn validate_token_file(path: &std::path::Path) -> Result<String, DiscordError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(DiscordError::Invalid(
            "bot_token_file must name a regular file".to_owned(),
        ));
    }
    if metadata.len() == 0 || metadata.len() > TOKEN_LIMIT {
        return Err(DiscordError::Invalid(
            "bot_token_file has an invalid size".to_owned(),
        ));
    }
    require_private(&metadata)?;
    let token = fs::read_to_string(path)?;
    let token = token.trim_end_matches(['\r', '\n']);
    if token.is_empty() || token.chars().any(char::is_whitespace) {
        return Err(DiscordError::Invalid(
            "bot_token_file must contain one token".to_owned(),
        ));
    }
    Ok(token.to_owned())
}

#[cfg(unix)]
fn require_private(metadata: &fs::Metadata) -> Result<(), DiscordError> {
    use std::os::unix::fs::PermissionsExt as _;

    if metadata.permissions().mode().trailing_zeros() >= 6 {
        Ok(())
    } else {
        Err(DiscordError::Invalid(
            "bot_token_file must not be accessible by group or other users".to_owned(),
        ))
    }
}

#[cfg(not(unix))]
fn require_private(_metadata: &fs::Metadata) -> Result<(), DiscordError> {
    Ok(())
}
