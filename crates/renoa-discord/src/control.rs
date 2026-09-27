use crate::{
    DiscordError, api::DiscordApi, config::Config, snowflake::Snowflake, store::SurfaceStore,
};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use uuid::Uuid;

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

/// Channel controls share surface storage, without acquiring worker ownership.
/// The authenticated Host adapter owns who may invoke these controls.
#[derive(Clone)]
pub struct DiscordControl {
    home: PathBuf,
    guild: Snowflake,
    operator: Snowflake,
    default_agent: Uuid,
    api: Arc<DiscordApi>,
}

impl DiscordControl {
    /// Pins the configured Discord account to this Host. Token files stay local.
    /// # Errors
    /// Rejects invalid configuration or a launch file naming a different home.
    pub fn open(config: &Path, home: &Path) -> Result<Self, DiscordError> {
        let config = Config::read(config)?;
        if config.data_directory != home {
            return Err(DiscordError::Invalid(
                "Discord configuration belongs to another Renoa home".into(),
            ));
        }
        Ok(Self {
            home: config.data_directory,
            guild: config.guild_id,
            operator: config.operator_user_id,
            default_agent: config.agent_id,
            api: Arc::new(DiscordApi::new(config.token)?),
        })
    }

    fn store(&self) -> Result<SurfaceStore, DiscordError> {
        let store = SurfaceStore::control(&self.home)?;
        store.bind_identity(&self.guild, &self.operator, self.default_agent)?;
        Ok(store)
    }

    /// The Host identity owning this surface configuration.
    /// # Errors
    /// Returns a missing or incompatible Host catalog.
    pub fn host_id(&self) -> Result<Uuid, DiscordError> {
        Ok(renoa_local::HostObserver::open(&self.home)?.host_id())
    }

    /// Lists saved bindings. Saved metadata does not assert current Discord health.
    /// # Errors
    /// Returns incompatible surface identity or unavailable storage.
    pub fn bindings(&self) -> Result<Vec<DiscordBinding>, DiscordError> {
        if !self.database_exists() {
            return Ok(Vec::new());
        }
        self.store()?.bindings()
    }

    /// Verifies a channel belongs to the configured guild before committing it.
    /// Exact retries recover the receipt before another Discord request.
    /// # Errors
    /// Rejects missing agents, foreign/non-text channels, stale revisions and conflicts.
    pub async fn bind(
        &self,
        host: &renoa_local::LocalHost,
        request: DiscordBindingRequest,
    ) -> Result<DiscordBinding, DiscordError> {
        if host.host_id().await? != self.host_id()? {
            return Err(DiscordError::Invalid(
                "Discord controls belong to a different Host".into(),
            ));
        }
        Snowflake::parse(&request.channel_id)?;
        if request.operation_id.is_nil()
            || request.agent_id.is_nil()
            || request.expected_revision < 0
        {
            return Err(DiscordError::Invalid(
                "Invalid channel binding identity or revision".into(),
            ));
        }
        if host
            .agent_definition(renoa_kernel::AgentId::from_uuid(request.agent_id))
            .await?
            .is_none()
        {
            return Err(DiscordError::Invalid(
                "The selected agent does not exist on this Host".into(),
            ));
        }
        let revision = if self.database_exists() {
            let store = self.store()?;
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
        let channel = self.api.channel(&request.channel_id).await?;
        if channel.id != request.channel_id
            || channel.guild_id.as_deref() != Some(self.guild.as_str())
            || !matches!(channel.kind, 0 | 5)
        {
            return Err(DiscordError::Invalid(
                "Choose a text channel in the configured Discord server".into(),
            ));
        }
        self.store()?.bind_channel(
            &request,
            channel.name.as_deref().unwrap_or("Discord channel"),
        )
    }

    fn database_exists(&self) -> bool {
        self.home
            .join("state/surfaces/discord/discord.sqlite3")
            .exists()
    }
}

#[cfg(test)]
#[path = "control/tests.rs"]
mod tests;
