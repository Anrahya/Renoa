use std::{fs, io::Write as _};

use renoa_local::RenoaHome;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{DiscordConnectRequest, DiscordError, snowflake::Snowflake};

const RECORD_LIMIT: u64 = 16 * 1024;
const TOKEN_LIMIT: usize = 512;

/// The Host's one Discord bot, committed once by the owner.
///
/// The record holds the bot token, so it stays in an owner-only file and is
/// never returned by an HTTP response.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Connection {
    pub(crate) operation_id: Uuid,
    pub(crate) bot_name: String,
    pub(crate) guild_id: Snowflake,
    pub(crate) guild_name: String,
    pub(crate) operator_user_id: Snowflake,
    pub(crate) agent_id: Uuid,
    pub(crate) bot_token: String,
}

impl Connection {
    /// Reads the committed connection, or `None` before the owner connects.
    pub(crate) fn read(home: &RenoaHome) -> Result<Option<Self>, DiscordError> {
        let path = home.discord_connection();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if !metadata.file_type().is_file() || metadata.len() > RECORD_LIMIT {
            return Err(DiscordError::Invalid(
                "The Discord connection must be a bounded regular file".into(),
            ));
        }
        require_private(&metadata)?;
        let connection: Self = serde_json::from_slice(&fs::read(&path)?)
            .map_err(|_| DiscordError::Invalid("The Discord connection is malformed".into()))?;
        connection.validate()?;
        Ok(Some(connection))
    }

    /// Commits this connection unless the Host already has one.
    ///
    /// A hard link publishes the fully written file, so a concurrent connect
    /// can never replace or observe a partial record. An existing record that
    /// answers `request` is adopted; any other is a conflict.
    pub(crate) fn publish(
        self,
        home: &RenoaHome,
        request: &DiscordConnectRequest,
    ) -> Result<Self, DiscordError> {
        self.validate()?;
        let path = home.discord_connection();
        let directory = path.parent().ok_or_else(|| {
            DiscordError::Invalid("The Discord connection has no directory".into())
        })?;
        let temporary = directory.join(format!(".discord-{}.tmp", Uuid::new_v4()));
        let linked = (|| {
            let mut options = fs::OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            let mut file = options.open(&temporary)?;
            file.write_all(&serde_json::to_vec(&self)?)?;
            file.sync_all()?;
            match fs::hard_link(&temporary, &path) {
                Ok(()) => {
                    fs::File::open(directory)?.sync_all()?;
                    Ok(true)
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
                Err(error) => Err(DiscordError::from(error)),
            }
        })();
        let removed = match fs::remove_file(&temporary) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        };
        if linked? {
            removed?;
            return Ok(self);
        }
        removed?;
        match Self::read(home)? {
            Some(existing) if existing.answers(request) => Ok(existing),
            _ => Err(already_connected()),
        }
    }

    /// Whether this record is the committed outcome of `request`.
    pub(crate) fn answers(&self, request: &DiscordConnectRequest) -> bool {
        self.operation_id == request.operation_id
            && self.bot_token == request.bot_token
            && self.guild_id.as_str() == request.guild_id
            && self.agent_id == request.agent_id
    }

    fn validate(&self) -> Result<(), DiscordError> {
        validate_token(&self.bot_token)?;
        if self.operation_id.is_nil() || self.agent_id.is_nil() {
            return Err(DiscordError::Invalid(
                "Discord connection identities must not be nil".into(),
            ));
        }
        Ok(())
    }
}

pub(crate) fn already_connected() -> DiscordError {
    DiscordError::Invalid("Discord is already connected on this Host".into())
}

/// Discord bot tokens are dot-separated base64url segments.
pub(crate) fn validate_token(token: &str) -> Result<(), DiscordError> {
    if token.is_empty()
        || token.len() > TOKEN_LIMIT
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(DiscordError::Invalid(
            "Paste only the bot token from the Developer Portal's Bot page".into(),
        ));
    }
    Ok(())
}

#[cfg(unix)]
pub(crate) fn require_private(metadata: &fs::Metadata) -> Result<(), DiscordError> {
    use std::os::unix::fs::PermissionsExt as _;

    if metadata.permissions().mode().trailing_zeros() >= 6 {
        Ok(())
    } else {
        Err(DiscordError::Invalid(
            "Discord connection and credential files must not be accessible by group or other users".into(),
        ))
    }
}

#[cfg(not(unix))]
pub(crate) fn require_private(_metadata: &fs::Metadata) -> Result<(), DiscordError> {
    Ok(())
}

#[cfg(all(test, unix))]
#[path = "connection/tests.rs"]
mod tests;
