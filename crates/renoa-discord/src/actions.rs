//! Plugin setup links reach the Discord operator by private message.
//!
//! The executing node checks each link and delivers it here, not through the
//! RCP task journal: a credential-setup link carries the key that keeps a
//! relayed credential unreadable to the coordinator. Only a digest of each
//! link is stored, and an unconfirmed delivery is never blindly repeated.

use crate::{
    DiscordError,
    api::{ApiError, DiscordApi},
    connection::Connection,
    snowflake::Snowflake,
    store::SurfaceStore,
};
use renoa_local::{RenoaHome, SetupLink};
use sha2::{Digest as _, Sha256};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// The Host's private channel to its Discord operator.
pub struct OperatorChannel {
    api: Arc<DiscordApi>,
    store: Arc<SurfaceStore>,
    operator: Snowflake,
}

impl OperatorChannel {
    /// Opens the channel from the Host's committed Discord connection, or
    /// returns `None` while Discord is not connected.
    ///
    /// # Errors
    ///
    /// Returns an unreadable connection or surface store.
    pub fn open(home: &RenoaHome) -> Result<Option<Self>, DiscordError> {
        let Some(connection) = Connection::read(home)? else {
            return Ok(None);
        };
        Ok(Some(Self {
            api: Arc::new(DiscordApi::new(connection.bot_token)?),
            store: Arc::new(SurfaceStore::control(home.path())?),
            operator: connection.operator_user_id,
        }))
    }

    /// The delivery of one command's setup links. Delivery stops sending once
    /// `cancellation` is cancelled.
    #[must_use]
    pub fn setup_delivery(
        &self,
        command_id: Uuid,
        cancellation: CancellationToken,
    ) -> SetupDelivery {
        SetupDelivery {
            api: Arc::clone(&self.api),
            store: Arc::clone(&self.store),
            operator: self.operator.clone(),
            command: command_id.to_string(),
            cancellation,
        }
    }
}

/// Delivers the setup links one command's plugin tool asks for.
pub struct SetupDelivery {
    api: Arc<DiscordApi>,
    store: Arc<SurfaceStore>,
    operator: Snowflake,
    command: String,
    cancellation: CancellationToken,
}

struct Action {
    stage: &'static str,
    url: String,
    expires: Option<i64>,
}

impl Action {
    fn new(link: &SetupLink) -> Self {
        Self {
            stage: link.kind.as_str(),
            url: link.url.to_string(),
            expires: link.expires_at_ms,
        }
    }

    fn require_unexpired(&self) -> Result<(), DiscordError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| DiscordError::Invalid("Clock precedes the Unix epoch".into()))?
            .as_millis();
        if self
            .expires
            .is_some_and(|expiry| u128::try_from(expiry).map_or(true, |expiry| now >= expiry))
        {
            return Err(DiscordError::Invalid(
                "Setup link expired. Restart this connection’s setup.".into(),
            ));
        }
        Ok(())
    }

    fn digest(&self) -> Vec<u8> {
        let mut hash = Sha256::new();
        hash.update(self.url.as_bytes());
        hash.update(self.expires.unwrap_or(0).to_le_bytes());
        hash.finalize().to_vec()
    }

    fn body(&self) -> String {
        let text = if self.stage == "credentials" {
            "Enter the credential on Renoa’s secure setup page. Keep it out of chat."
        } else {
            "Open the provider page and approve access."
        };
        format!(
            "Plugin setup needs your action.\n{text}\n\n<{}>\n\nIf this link expires, restart the connection setup.",
            self.url
        )
    }
}

impl SetupDelivery {
    /// Sends `link` for tool call `call` to the operator's DM once. Returns
    /// without sending once the command is cancelled.
    ///
    /// # Errors
    ///
    /// Returns why the link was not delivered, or why its delivery is
    /// unconfirmed; the command stops with it.
    pub async fn deliver(&self, call: &str, link: &SetupLink) -> Result<(), DiscordError> {
        let action = Action::new(link);
        if action.body().chars().count() > 2000 {
            return Err(DiscordError::Invalid("This setup link exceeds Discord’s message limit. Continue setup through another private surface.".into()));
        }
        loop {
            if self.cancellation.is_cancelled() {
                return Ok(());
            }
            action.require_unexpired()?;
            // Opening a DM is idempotent and does not deliver the sensitive link.
            let channel = match self.api.direct_channel(self.operator.as_str()).await {
                Ok(channel) => channel,
                Err(ApiError::RateLimited(delay)) => { tokio::select! { () = self.cancellation.cancelled() => return Ok(()), () = tokio::time::sleep(delay) => continue } },
                Err(_) => return Err(DiscordError::Invalid("Discord could not open a private conversation with the configured operator. Enable DMs and retry setup.".into())),
            };
            let channel = Snowflake::parse(&channel)?;
            if self.cancellation.is_cancelled() {
                return Ok(());
            }
            action.require_unexpired()?;
            if !self
                .store
                .claim_action(&self.command, call, action.stage, &action.digest())?
            {
                return Ok(());
            }
            match self
                .api
                .create_message(channel.as_str(), &action.body(), None)
                .await
            {
                Ok(reply) => {
                    if Snowflake::parse(&reply).is_err() {
                        self.store
                            .action_state(&self.command, call, action.stage, "unknown")?;
                        return Err(DiscordError::Invalid("Discord returned an unreadable setup delivery receipt. Check your DM before restarting setup.".into()));
                    }
                    self.store
                        .action_state(&self.command, call, action.stage, "sent")?;
                    return Ok(());
                }
                Err(ApiError::RateLimited(delay)) => {
                    self.store
                        .release_action(&self.command, call, action.stage)?;
                    tokio::select! { () = self.cancellation.cancelled() => return Ok(()), () = tokio::time::sleep(delay) => {} }
                }
                Err(error) => {
                    let state = if matches!(error, ApiError::Rejected(_) | ApiError::Unauthorized) {
                        "failed"
                    } else {
                        "unknown"
                    };
                    self.store
                        .action_state(&self.command, call, action.stage, state)?;
                    return Err(DiscordError::Invalid("Setup message delivery could not be confirmed. Check your Discord DM and restart setup if needed.".into()));
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "actions/tests.rs"]
mod tests;
