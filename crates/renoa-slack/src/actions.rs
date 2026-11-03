use std::sync::Arc;

use renoa_local::{PluginCredentialKind, SetupLink, SetupLinkKind};
use sha2::{Digest as _, Sha256};
use tokio_util::sync::CancellationToken;

use crate::{
    SlackError,
    api::{ApiError, SlackApi},
    ingress::Topic,
    store::{DeliveryState, Store},
};

pub(crate) struct Actions {
    pub(crate) api: Arc<SlackApi>,
    pub(crate) store: Store,
    pub(crate) topic: Topic,
    pub(crate) seq: i64,
    pub(crate) cancellation: CancellationToken,
}

pub(crate) struct Action {
    pub(crate) stage: &'static str,
    pub(crate) text: String,
    pub(crate) expires_at_ms: i64,
}

impl Action {
    /// The Slack message for a checked link. Slack shows only a link that
    /// says when it expires.
    pub(crate) fn new(link: &SetupLink) -> Result<Self, SlackError> {
        let title = match link.kind {
            SetupLinkKind::Credentials(PluginCredentialKind::OAuthClient) => {
                "Action needed: configure this connection\nEnter the app credentials on Renoa's secure page. After saving them, return here for a separate authorization message."
            }
            SetupLinkKind::Credentials(PluginCredentialKind::ApiToken) => {
                "Action needed: save the API credential\nEnter the API key on Renoa's secure page. Keep it out of chat. Connection setup will continue after it is saved."
            }
            SetupLinkKind::Authorization => {
                "Action needed: authorize access\nThe connection is ready for sign-in. Open the provider page below and approve access. This is separate from saving app credentials."
            }
        };
        let expires_at_ms = link.expires_at_ms.ok_or_else(|| {
            SlackError::Invalid(
                "This setup link has no expiry, so it was not sent. Ask Arcee to restart the connection setup."
                    .to_owned(),
            )
        })?;
        Ok(Self {
            stage: link.kind.as_str(),
            text: format!(
                "{title}\n\n{}\n\nThis link expires. If it expires, ask me to restart this connection's setup; saved credentials are reused.",
                link.url
            ),
            expires_at_ms,
        })
    }

    fn digest(&self) -> Vec<u8> {
        let mut hash = Sha256::new();
        hash.update(self.text.as_bytes());
        hash.update(self.expires_at_ms.to_le_bytes());
        hash.finalize().to_vec()
    }
}

impl Actions {
    pub(crate) async fn deliver(&self, call_id: &str, action: Action) -> Result<(), SlackError> {
        if !self.topic.channel.starts_with('D') {
            return Err(SlackError::Invalid(
                "Account setup needs a private conversation. Continue setup in a DM with Arcee."
                    .to_owned(),
            ));
        }
        loop {
            if self.cancellation.is_cancelled() {
                return Ok(());
            }
            if crate::service::now_ms()? >= action.expires_at_ms {
                return Err(SlackError::Invalid(
                    "This setup link expired. Ask Arcee to restart the connection setup."
                        .to_owned(),
                ));
            }
            match self.store.claim_action(self.seq, call_id.to_owned(), action.stage.to_owned(), action.digest()).await? {
                DeliveryState::Sent => return Ok(()),
                DeliveryState::Sending => {},
                _ => return Err(SlackError::Invalid("Setup message delivery could not be confirmed. Check this DM for the action message; if it is missing, ask Arcee to restart setup. Renoa will not blindly post a duplicate.".to_owned())),
            }
            let sent = self.api.post(&self.topic, &action.text).await;
            let (state, ts, error, retry) = match sent {
                Ok(sent) => (DeliveryState::Sent, Some(sent.ts), None, None),
                Err(ApiError::RateLimited(delay)) => {
                    (DeliveryState::Pending, None, None, Some(delay))
                }
                Err(error) => {
                    let state = if matches!(error, ApiError::Rejected(_)) {
                        DeliveryState::Failed
                    } else {
                        DeliveryState::Unknown
                    };
                    (state, None, Some(error.to_string()), None)
                }
            };
            self.store
                .action_state(
                    self.seq,
                    call_id.to_owned(),
                    action.stage.to_owned(),
                    state,
                    ts,
                    error,
                )
                .await?;
            if let Some(delay) = retry {
                tokio::select! { () = self.cancellation.cancelled() => return Ok(()), () = tokio::time::sleep(delay) => {} }
            } else if matches!(state, DeliveryState::Sent) {
                return Ok(());
            } else {
                return Err(SlackError::Invalid("Could not confirm delivery of the setup action message. Check this DM, then ask Arcee to restart setup if needed.".to_owned()));
            }
        }
    }
}
