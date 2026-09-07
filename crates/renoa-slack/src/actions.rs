use std::sync::Arc;

use renoa_agent::{ContentBlock, ToolOutput};
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
    pub(crate) fn parse(update: &ToolOutput) -> Option<Self> {
        #[derive(serde::Deserialize)]
        struct Update {
            status: String,
            setup_url: Option<String>,
            credential_kind: Option<String>,
            authorization_url: Option<String>,
            expires_at_ms: i64,
        }
        if update.is_error {
            return None;
        }
        let [ContentBlock::Text { text }] = update.content.as_slice() else {
            return None;
        };
        let input: Update = serde_json::from_str(text).ok()?;
        let (stage, url, title) = match input.status.as_str() {
            "credential_required" => (
                "credentials",
                input.setup_url?,
                match input.credential_kind.as_deref() {
                    Some("oauth_client") => {
                        "Action needed: configure this connection\nEnter the app credentials on Renoa's secure page. After saving them, return here for a separate authorization message."
                    }
                    Some("api_token") => {
                        "Action needed: save the API credential\nEnter the API key on Renoa's secure page. Keep it out of chat. Connection setup will continue after it is saved."
                    }
                    _ => return None,
                },
            ),
            "authorization_required" => (
                "authorization",
                input.authorization_url?,
                "Action needed: authorize access\nThe connection is ready for sign-in. Open the provider page below and approve access. This is separate from saving app credentials.",
            ),
            _ => return None,
        };
        let parsed = url::Url::parse(&url).ok()?;
        if parsed.scheme() != "https"
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || url.len() > 16 * 1024
            || input.expires_at_ms <= 0
            || (stage == "authorization" && parsed.fragment().is_some())
        {
            return None;
        }
        Some(Self {
            stage,
            text: format!(
                "{title}\n\n{url}\n\nThis link expires. If it expires, ask me to restart this connection's setup; saved credentials are reused."
            ),
            expires_at_ms: input.expires_at_ms,
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
