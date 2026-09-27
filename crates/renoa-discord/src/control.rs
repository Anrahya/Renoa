use std::{
    path::Path,
    sync::{Arc, OnceLock},
};

use renoa_local::{LocalHost, RenoaHome};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    DiscordError,
    api::DiscordApi,
    connection::{Connection, already_connected, validate_token},
    discovery::{self, DiscordChannel, DiscordInspection},
    snowflake::Snowflake,
    store::SurfaceStore,
};

/// The owner's one-time Discord connection. The bot token never leaves the Host.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscordConnectRequest {
    pub operation_id: Uuid,
    pub bot_token: String,
    pub guild_id: String,
    pub agent_id: Uuid,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscordBindingRequest {
    pub operation_id: Uuid,
    pub channel_id: String,
    pub agent_id: Uuid,
    pub expected_revision: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscordBinding {
    pub channel_id: String,
    pub agent_id: Uuid,
    pub channel_name: String,
    pub revision: i64,
}

/// Saved Discord setup. A connection records what Discord confirmed when the
/// owner connected; it does not assert that the worker is running now.
#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DiscordStatus {
    SetupRequired,
    Connected {
        bot_name: String,
        guild_name: String,
        default_agent_id: Uuid,
        bindings: Vec<DiscordBinding>,
    },
}

/// Owner controls share surface storage, without acquiring worker ownership.
/// The authenticated Host adapter owns who may invoke these controls.
#[derive(Clone)]
pub struct DiscordControl {
    home: RenoaHome,
    origin: String,
    connected: Arc<OnceLock<Connected>>,
}

struct Connected {
    connection: Connection,
    api: DiscordApi,
}

impl DiscordControl {
    /// Opens Discord controls for one Renoa home. Nothing is read or written yet.
    /// # Errors
    /// Rejects an invalid Renoa home.
    pub fn open(home: &Path) -> Result<Self, DiscordError> {
        Ok(Self::with_origin(
            RenoaHome::at(home)?,
            crate::api::API.to_owned(),
        ))
    }

    fn with_origin(home: RenoaHome, origin: String) -> Self {
        Self {
            home,
            origin,
            connected: Arc::default(),
        }
    }

    /// Reports the saved connection and bindings, without contacting Discord.
    /// # Errors
    /// Returns an unreadable connection, incompatible identity, or unavailable storage.
    pub fn status(&self) -> Result<DiscordStatus, DiscordError> {
        let Some(connected) = self.connected()? else {
            return Ok(DiscordStatus::SetupRequired);
        };
        let connection = &connected.connection;
        let bindings = if self.database_exists() {
            self.store(connection)?.bindings()?
        } else {
            Vec::new()
        };
        Ok(DiscordStatus::Connected {
            bot_name: connection.bot_name.clone(),
            guild_name: connection.guild_name.clone(),
            default_agent_id: connection.agent_id,
            bindings,
        })
    }

    /// Reads what a pasted token can reach. Nothing is saved.
    /// # Errors
    /// Rejects a malformed or refused token and an application without
    /// Message Content Intent; Discord outages are unavailable errors.
    pub async fn inspect(&self, bot_token: String) -> Result<DiscordInspection, DiscordError> {
        validate_token(&bot_token)?;
        discovery::inspect(&DiscordApi::with_origin(bot_token, self.origin.clone())?).await
    }

    /// Commits the Host's only Discord connection after Discord confirms the
    /// bot is an Administrator of the chosen server. An exact retry returns
    /// the committed connection before contacting Discord again.
    /// # Errors
    /// Rejects a missing agent, a server without the bot or its Administrator
    /// role, and any connection other than a retry of the committed one.
    pub async fn connect(
        &self,
        host: &LocalHost,
        request: DiscordConnectRequest,
    ) -> Result<DiscordStatus, DiscordError> {
        validate_token(&request.bot_token)?;
        let guild_id = Snowflake::parse(&request.guild_id)?;
        if request.operation_id.is_nil() {
            return Err(DiscordError::Invalid(
                "Invalid Discord connection identity".into(),
            ));
        }
        self.require_agent(host, request.agent_id).await?;
        if let Some(connected) = self.connected()? {
            return if connected.connection.answers(&request) {
                self.status()
            } else {
                Err(already_connected())
            };
        }
        let api = DiscordApi::with_origin(request.bot_token.clone(), self.origin.clone())?;
        let identity = discovery::identify(&api).await?;
        let guild = api
            .guilds()
            .await?
            .into_iter()
            .find(|guild| guild.id == guild_id)
            .ok_or_else(|| {
                DiscordError::Invalid("Invite the bot to this server, then retry".into())
            })?;
        if !guild.administrator {
            return Err(DiscordError::Invalid(format!(
                "Give the bot's role Administrator in {}, then retry",
                guild.name
            )));
        }
        let connection = Connection {
            operation_id: request.operation_id,
            bot_name: identity.bot_name,
            guild_id,
            guild_name: guild.name,
            operator_user_id: identity.operator_user_id,
            agent_id: request.agent_id,
            bot_token: request.bot_token.clone(),
        }
        .publish(&self.home, &request)?;
        self.adopt(connection)?;
        self.status()
    }

    /// Lists the connected server's chat channels in Discord's sidebar order.
    /// # Errors
    /// Rejects an unconnected Host; Discord failures are unavailable errors.
    pub async fn channels(&self) -> Result<Vec<DiscordChannel>, DiscordError> {
        let connected = self.require_connected()?;
        discovery::channels(&connected.api, &connected.connection.guild_id).await
    }

    /// Verifies a channel belongs to the connected server before committing it.
    /// Exact retries recover the receipt before another Discord request.
    /// # Errors
    /// Rejects missing agents, foreign/non-text channels, stale revisions and conflicts.
    pub async fn bind(
        &self,
        host: &LocalHost,
        request: DiscordBindingRequest,
    ) -> Result<DiscordBinding, DiscordError> {
        let connected = self.require_connected()?;
        Snowflake::parse(&request.channel_id)?;
        if request.operation_id.is_nil() || request.expected_revision < 0 {
            return Err(DiscordError::Invalid(
                "Invalid channel binding identity or revision".into(),
            ));
        }
        self.require_agent(host, request.agent_id).await?;
        let revision = if self.database_exists() {
            let store = self.store(&connected.connection)?;
            if let Some(result) = store.binding_receipt(&request)? {
                return Ok(result);
            }
            store
                .channel_binding(&request.channel_id)?
                .map_or(0, |binding| binding.revision)
        } else {
            0
        };
        if revision != request.expected_revision {
            return Err(DiscordError::Invalid(
                "This channel binding changed. Refresh before saving.".into(),
            ));
        }
        let channel = connected.api.channel(&request.channel_id).await?;
        if channel.id.as_str() != request.channel_id
            || channel.guild_id.as_ref() != Some(&connected.connection.guild_id)
            || !channel.is_text()
        {
            return Err(DiscordError::Invalid(
                "Choose a text channel in the connected Discord server".into(),
            ));
        }
        self.store(&connected.connection)?
            .bind_channel(&request, channel.display_name())
    }

    fn connected(&self) -> Result<Option<&Connected>, DiscordError> {
        if let Some(connected) = self.connected.get() {
            return Ok(Some(connected));
        }
        Connection::read(&self.home)?
            .map(|connection| self.adopt(connection))
            .transpose()
    }

    fn require_connected(&self) -> Result<&Connected, DiscordError> {
        self.connected()?
            .ok_or_else(|| DiscordError::Invalid("Connect Discord before choosing channels".into()))
    }

    /// The committed file never changes while a process runs, so the first
    /// adopted record serves every later request.
    fn adopt(&self, connection: Connection) -> Result<&Connected, DiscordError> {
        let api = DiscordApi::with_origin(connection.bot_token.clone(), self.origin.clone())?;
        Ok(self.connected.get_or_init(|| Connected { connection, api }))
    }

    async fn require_agent(&self, host: &LocalHost, agent: Uuid) -> Result<(), DiscordError> {
        if host.host_id().await? != renoa_local::HostObserver::open(self.home.path())?.host_id() {
            return Err(DiscordError::Invalid(
                "Discord controls belong to a different Host".into(),
            ));
        }
        if agent.is_nil()
            || host
                .agent_definition(renoa_kernel::AgentId::from_uuid(agent))
                .await?
                .is_none()
        {
            return Err(DiscordError::Invalid(
                "The selected agent does not exist on this Host".into(),
            ));
        }
        Ok(())
    }

    fn store(&self, connection: &Connection) -> Result<SurfaceStore, DiscordError> {
        let store = SurfaceStore::control(self.home.path())?;
        store.bind_identity(
            &connection.guild_id,
            &connection.operator_user_id,
            connection.agent_id,
        )?;
        Ok(store)
    }

    fn database_exists(&self) -> bool {
        self.home
            .path()
            .join("state/surfaces/discord/discord.sqlite3")
            .exists()
    }
}

#[cfg(test)]
#[path = "control/tests.rs"]
mod tests;
