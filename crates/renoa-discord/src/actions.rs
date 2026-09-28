//! Plugin setup links reach the Discord operator by private message.
//!
//! The executing node delivers them directly, not through the RCP task
//! journal: a credential-setup link carries the key that keeps a relayed
//! credential unreadable to the coordinator. Only a digest of each link is
//! stored, and an unconfirmed delivery is never blindly repeated.

use crate::{
    DiscordError,
    api::{ApiError, DiscordApi},
    connection::Connection,
    snowflake::Snowflake,
    store::SurfaceStore,
};
use renoa_agent::{AgentEvent, AgentEventSink, BoxFuture, ContentBlock, ToolOutput};
use renoa_local::{PluginCredentialKind, PluginProgress, RenoaHome};
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

    /// A sink that delivers one command's setup links. It cancels
    /// `cancellation` when a link cannot be delivered, and keeps the reason.
    #[must_use]
    pub fn setup_delivery(
        &self,
        command_id: Uuid,
        cancellation: CancellationToken,
    ) -> Arc<SetupDelivery> {
        Arc::new(SetupDelivery {
            api: Arc::clone(&self.api),
            store: Arc::clone(&self.store),
            operator: self.operator.clone(),
            command: command_id.to_string(),
            cancellation,
            error: tokio::sync::Mutex::new(None),
        })
    }
}

/// Delivers the setup links one command's plugin tool asks for.
pub struct SetupDelivery {
    api: Arc<DiscordApi>,
    store: Arc<SurfaceStore>,
    operator: Snowflake,
    command: String,
    cancellation: CancellationToken,
    error: tokio::sync::Mutex<Option<String>>,
}

impl SetupDelivery {
    /// Why delivery stopped the command, if it did.
    pub async fn take_error(&self) -> Option<String> {
        self.error.lock().await.take()
    }
}

struct Action {
    stage: &'static str,
    url: String,
    expires: Option<i64>,
}

impl Action {
    fn parse(tool: &str, update: &ToolOutput) -> Option<Self> {
        if tool != "plugin_manage" || update.is_error {
            return None;
        }
        let [ContentBlock::Text { text }] = update.content.as_slice() else {
            return None;
        };
        let event: PluginProgress = serde_json::from_str(text).ok()?;
        let (stage, url, expires, kind) = match event {
            PluginProgress::AuthorizationRequired(event) => (
                "authorization",
                event.authorization_url,
                event.expires_at_ms,
                None,
            ),
            PluginProgress::CredentialRequired(event) => (
                "credentials",
                event.setup_url,
                Some(event.expires_at_ms),
                Some(event.credential_kind),
            ),
        };
        let parsed = url::Url::parse(&url).ok()?;
        if url.len() > 16 * 1024
            || url.chars().any(char::is_control)
            || parsed.scheme() != "https"
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || expires.is_some_and(|expiry| expiry <= 0)
        {
            return None;
        }
        match kind {
            None if parsed.fragment().is_some() => return None,
            Some(kind)
                if parsed.query().is_some()
                    || !parsed
                        .fragment()
                        .is_some_and(|fragment| valid_fragment(fragment, kind)) =>
            {
                return None;
            }
            _ => {}
        }
        Some(Self {
            stage,
            url: parsed.to_string(),
            expires,
        })
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

fn valid_fragment(fragment: &str, kind: PluginCredentialKind) -> bool {
    let mut values = std::collections::BTreeMap::new();
    for (key, value) in url::form_urlencoded::parse(fragment.as_bytes()) {
        if !matches!(key.as_ref(), "v" | "key" | "token" | "issuer")
            || values
                .insert(key.into_owned(), value.into_owned())
                .is_some()
        {
            return false;
        }
    }
    let secret = |name| {
        values.get(name).is_some_and(|value| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        })
    };
    values.get("v").is_some_and(|value| value == "1")
        && secret("key")
        && secret("token")
        && match kind {
            PluginCredentialKind::ApiToken => !values.contains_key("issuer"),
            PluginCredentialKind::OAuthClient => values.get("issuer").is_some_and(|value| {
                url::Url::parse(value).is_ok_and(|issuer| {
                    issuer.scheme() == "https"
                        && issuer.host_str().is_some()
                        && issuer.username().is_empty()
                        && issuer.password().is_none()
                        && issuer.query().is_none()
                        && issuer.fragment().is_none()
                })
            }),
        }
}

impl SetupDelivery {
    async fn deliver(&self, call: &str, action: Action) -> Result<(), DiscordError> {
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

impl AgentEventSink for SetupDelivery {
    fn emit(&self, event: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if let AgentEvent::ToolExecutionUpdate { call, update } = event {
                let result = if let Some(action) = Action::parse(&call.name, &update) {
                    self.deliver(&call.id, action).await
                } else if call.name == "plugin_manage"
                    && !update.is_error
                    && matches!(update.content.as_slice(), [ContentBlock::Text { text }] if serde_json::from_str::<PluginProgress>(text).is_ok())
                {
                    Err(DiscordError::Invalid("The Host returned an invalid setup link. Restart connection setup; no link was sent.".into()))
                } else {
                    Ok(())
                };
                if let Err(error) = result {
                    *self.error.lock().await = Some(error.to_string());
                    self.cancellation.cancel();
                }
            }
        })
    }
}

#[cfg(test)]
#[path = "actions/tests.rs"]
mod tests;
