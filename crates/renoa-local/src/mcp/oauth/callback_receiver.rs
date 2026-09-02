use tokio_util::sync::CancellationToken;

use super::{
    OAuthCoordinator,
    callback::{OAuthCallbackListener, ReceivedCallback as LoopbackReceivedCallback},
    relay::{RemoteCallbackData, RemoteOAuthCallback},
    store::OAuthCallbackIdentity,
};
use crate::mcp::{McpHostError, McpOAuthError};

pub(super) enum OAuthCallbackReceiver {
    Loopback(OAuthCallbackListener),
    Relay(RemoteOAuthCallback),
}

pub(super) enum ReceivedCallback {
    Loopback(LoopbackReceivedCallback),
    Relay(RemoteCallbackData),
}

impl OAuthCallbackReceiver {
    pub(super) async fn new(
        coordinator: &OAuthCoordinator,
        state: &str,
        cancellation: &CancellationToken,
    ) -> Result<Self, McpHostError> {
        match &coordinator.relay {
            Some(relay) => relay.create(state, cancellation).await.map(Self::Relay),
            None => OAuthCallbackListener::bind_new().await.map(Self::Loopback),
        }
    }

    pub(super) async fn resume(
        coordinator: &OAuthCoordinator,
        identity: OAuthCallbackIdentity,
        expires_at_ms: i64,
    ) -> Result<Self, McpHostError> {
        match identity {
            OAuthCallbackIdentity::Loopback(port) => {
                OAuthCallbackListener::resume(port, expires_at_ms)
                    .await
                    .map(Self::Loopback)
                    .map_err(|error| match error {
                        McpHostError::Io(_) => {
                            McpHostError::OAuth(McpOAuthError::CallbackUnavailable(
                                "saved callback port is unavailable; retry later or call extension_manage authorize with restart=true"
                                    .to_owned(),
                            ))
                        }
                        error => error,
                    })
            }
            OAuthCallbackIdentity::Relay(relay_id) => coordinator
                .relay
                .as_ref()
                .ok_or_else(|| {
                    McpHostError::OAuth(McpOAuthError::CallbackUnavailable(
                        "saved callback requires RENOA_OAUTH_RELAY_ORIGIN and its device credential"
                            .to_owned(),
                    ))
                })?
                .resume(relay_id, expires_at_ms)
                .map(Self::Relay),
        }
    }

    pub(super) const fn identity(&self) -> OAuthCallbackIdentity {
        match self {
            Self::Loopback(listener) => OAuthCallbackIdentity::Loopback(listener.port()),
            Self::Relay(relay) => OAuthCallbackIdentity::Relay(relay.relay_id()),
        }
    }

    pub(super) const fn expires_at_ms(&self) -> i64 {
        match self {
            Self::Loopback(listener) => listener.expires_at_ms(),
            Self::Relay(relay) => relay.expires_at_ms(),
        }
    }

    pub(super) fn redirect_uri(&self) -> String {
        match self {
            Self::Loopback(listener) => listener.redirect_uri(),
            Self::Relay(relay) => relay.redirect_uri().to_owned(),
        }
    }

    pub(super) const fn opens_local_browser(&self) -> bool {
        matches!(self, Self::Loopback(_))
    }

    pub(super) async fn receive(
        self,
        expected_state: &str,
        cancellation: &CancellationToken,
    ) -> Result<ReceivedCallback, McpHostError> {
        match self {
            Self::Loopback(listener) => listener
                .receive(expected_state, cancellation)
                .await
                .map(ReceivedCallback::Loopback),
            Self::Relay(receiver) => {
                let data = receiver.receive(cancellation).await?;
                Ok(ReceivedCallback::Relay(data))
            }
        }
    }
}

impl ReceivedCallback {
    pub(super) fn take_authorization_code(&mut self) -> Option<String> {
        match self {
            Self::Loopback(callback) => callback.take_authorization_code(),
            Self::Relay(data) => match data {
                RemoteCallbackData::Authorized {
                    authorization_code, ..
                } => Some(std::mem::take(authorization_code)),
                RemoteCallbackData::Rejected { .. } => None,
            },
        }
    }

    pub(super) fn take_issuer(&mut self) -> Option<String> {
        match self {
            Self::Loopback(callback) => callback.take_issuer(),
            Self::Relay(data) => match data {
                RemoteCallbackData::Authorized { issuer, .. } => issuer.take(),
                RemoteCallbackData::Rejected { .. } => None,
            },
        }
    }

    pub(super) fn take_rejection(&mut self) -> Option<String> {
        match self {
            Self::Loopback(callback) => callback.take_rejection(),
            Self::Relay(data) => match data {
                RemoteCallbackData::Authorized { .. } => None,
                RemoteCallbackData::Rejected { error } => Some(std::mem::take(error)),
            },
        }
    }

    pub(super) async fn acknowledge_browser(self) {
        match self {
            Self::Loopback(callback) => callback.acknowledge().await,
            Self::Relay(_) => {}
        }
    }
}
