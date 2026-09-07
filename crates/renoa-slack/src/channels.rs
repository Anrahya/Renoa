use crate::{
    SlackError,
    api::{ApiError, SlackApi},
    service::pause,
    store::Store,
};
use renoa_local::{BotSummary, LocalHost};
use std::{sync::Arc, time::Duration};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

mod api;
mod names;
mod store;
use store::{Provision, State};

pub(crate) struct Channels {
    pub(crate) host: LocalHost,
    pub(crate) store: Store,
    pub(crate) api: Arc<SlackApi>,
    pub(crate) bot: String,
    pub(crate) user: String,
    pub(crate) shutdown: CancellationToken,
    pub(crate) wake: Arc<Notify>,
}

impl Channels {
    pub(crate) async fn run(self) -> Result<(), SlackError> {
        loop {
            let mut cursor = None;
            loop {
                let page = self.host.list_bots(cursor).await?;
                for bot in page.bots {
                    if self.shutdown.is_cancelled() {
                        return Ok(());
                    }
                    self.provision(&bot).await?;
                }
                cursor = page.next_cursor;
                if cursor.is_none() {
                    break;
                }
            }
            tokio::select! {
                () = self.shutdown.cancelled() => return Ok(()),
                () = self.wake.notified() => {},
                () = tokio::time::sleep(Duration::from_mins(1)) => {},
            }
        }
    }

    pub(crate) async fn provision(&self, bot: &BotSummary) -> Result<(), SlackError> {
        let provision = self.store.channel_provision(bot).await?;
        let channel = match &provision.state {
            State::Ready => return self.label(bot).await,
            State::Inviting(id) => id.clone(),
            State::Pending => {
                if self.shutdown.is_cancelled() { return Ok(()); }
                self.store.channel_state(&provision.agent, "creating", None, None).await?;
                // Once this intent commits, a crash/unknown response is reconciled
                // by lookup, never by another create request.
                match self.api.create_private_channel(&provision.name).await {
                    Ok(channel) => {
                        if let Err(error) = channel.validate(&provision.name, &self.bot) {
                            return self.failed(&provision, "creating", None, error).await;
                        }
                        self.store.bind_channel(&provision.agent, &channel.id).await?;
                        channel.id
                    }
                    Err(error) => {
                        let state = match &error {
                            ApiError::RateLimited(_) => "pending",
                            ApiError::Rejected(code) if code != "name_taken" => "pending",
                            _ => "creating",
                        };
                        return self.failed(&provision, state, None, error).await;
                    }
                }
            }
            State::Creating => match self.api.find_created_channel(&provision.name, &self.bot).await {
                Ok(Some(channel)) => {
                    self.store.bind_channel(&provision.agent, &channel.id).await?;
                    channel.id
                }
                Ok(None) => return self.failed(&provision, "creating", None,
                    ApiError::Unknown("channel creation remains unresolved; no matching channel is visible. Inspect Slack before repairing the retained intent".to_owned())).await,
                Err(error) => return self.failed(&provision, "creating", None, error).await,
            },
        };
        if self.shutdown.is_cancelled() {
            return Ok(());
        }
        match self.api.invite_operator(&channel, &self.user).await {
            Ok(()) => {
                self.store
                    .channel_state(&provision.agent, "ready", Some(&channel), None)
                    .await?;
                self.label(bot).await
            }
            Err(error) => {
                self.failed(&provision, "inviting", Some(&channel), error)
                    .await
            }
        }
    }

    async fn failed(
        &self,
        provision: &Provision,
        state: &str,
        channel: Option<&str>,
        error: ApiError,
    ) -> Result<(), SlackError> {
        let detail = match &error {
            ApiError::Rejected(code) if code == "missing_scope" => "Slack needs groups:write and groups:read; update the app scopes and reinstall it in the workspace".to_owned(),
            _ => error.to_string(),
        };
        self.store
            .channel_state(&provision.agent, state, channel, Some(&detail))
            .await?;
        if let ApiError::RateLimited(delay) = error {
            pause(&self.shutdown, delay).await;
        }
        Ok(())
    }
}
