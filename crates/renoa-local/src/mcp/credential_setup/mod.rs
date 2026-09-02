use std::{path::Path, time::Duration};

use renoa_agent::ToolUpdates;
use renoa_credential_relay_protocol::{CredentialRelayKind, CredentialRelayStatus};
use tokio_util::sync::CancellationToken;

use super::{McpConnectionAuth, McpCredentialError, McpHostError, oauth::PrivateSecretStore};

mod client;
mod crypto;
mod state;
mod support;

use client::{CredentialRelayClient, remaining};
use state::{CredentialSetupState, CredentialSetupStateStore, state_id};

const POLL_INTERVAL: Duration = Duration::from_secs(1);
const MAX_STORED_SECRET_BYTES: usize = 64 * 1024;

#[derive(Clone)]
pub(super) struct CredentialSetupCoordinator {
    client: CredentialRelayClient,
    secrets: PrivateSecretStore,
    states: CredentialSetupStateStore,
}

impl CredentialSetupCoordinator {
    pub(super) fn new(
        origin: &str,
        relay_credentials: &Path,
        secrets: PrivateSecretStore,
        states_directory: std::path::PathBuf,
    ) -> Result<Self, McpHostError> {
        Ok(Self {
            client: CredentialRelayClient::new(origin, relay_credentials)?,
            secrets,
            states: CredentialSetupStateStore::initialize(states_directory)?,
        })
    }

    pub(super) async fn ensure(
        &self,
        reference: &McpConnectionAuth,
        operation_id: &str,
        updates: Option<&ToolUpdates>,
        cancellation: CancellationToken,
    ) -> Result<(), McpHostError> {
        let Some((credential_id, kind)) = setup_requirement(reference) else {
            return Ok(());
        };
        let state_id = state_id(operation_id, credential_id, kind);
        let saved_state = self.states.load(&state_id).await?;
        if saved_state.is_none() && self.secret_exists(credential_id).await? {
            return Ok(());
        }
        let Some(updates) = updates else {
            return Ok(());
        };
        let mut state = if let Some(state) = saved_state {
            state
        } else {
            let state = CredentialSetupState::new(credential_id.to_owned(), kind)?;
            self.states.store(&state_id, &state).await?;
            state
        };
        state.expires_at_ms = self.client.reserve(&state, &cancellation).await?;
        self.states.store(&state_id, &state).await?;

        if self.secret_exists(credential_id).await? {
            return self.finish_existing(&state_id, &state, cancellation).await;
        }

        let setup_url = self.client.setup_url(&state)?;
        support::emit_required(
            updates,
            credential_id,
            kind_name(kind),
            setup_url.as_str(),
            state.expires_at_ms,
        )
        .await;
        loop {
            let wait = remaining(state.expires_at_ms)?;
            if cancellation.is_cancelled() {
                return Err(McpCredentialError::Cancelled.into());
            }
            match self.client.status(state.relay_id, &cancellation).await? {
                CredentialRelayStatus::Pending { .. } => {
                    tokio::select! {
                        biased;
                        () = cancellation.cancelled() => {
                            return Err(McpCredentialError::Cancelled.into());
                        }
                        () = tokio::time::sleep(POLL_INTERVAL.min(wait)) => {}
                    }
                }
                CredentialRelayStatus::Submitted {
                    nonce, ciphertext, ..
                } => {
                    let mut secret = crypto::decrypt_and_validate(&state, &nonce, &ciphertext)?;
                    if secret.is_empty() || secret.len() > MAX_STORED_SECRET_BYTES {
                        secret.fill(0);
                        return Err(McpCredentialError::SetupInvalid.into());
                    }
                    let stored = self.secrets.store(credential_id, secret.clone()).await;
                    secret.fill(0);
                    stored?;
                    self.client
                        .acknowledge(state.relay_id, &cancellation)
                        .await?;
                    self.states.delete(&state_id).await?;
                    return Ok(());
                }
                CredentialRelayStatus::Acknowledged { .. } => {
                    if self.secret_exists(credential_id).await? {
                        self.states.delete(&state_id).await?;
                        return Ok(());
                    }
                    return Err(McpCredentialError::SetupInvalid.into());
                }
            }
        }
    }

    async fn finish_existing(
        &self,
        state_id: &str,
        state: &CredentialSetupState,
        cancellation: CancellationToken,
    ) -> Result<(), McpHostError> {
        match self.client.status(state.relay_id, &cancellation).await? {
            CredentialRelayStatus::Submitted { .. } => {
                self.client
                    .acknowledge(state.relay_id, &cancellation)
                    .await?;
            }
            CredentialRelayStatus::Pending { .. } | CredentialRelayStatus::Acknowledged { .. } => {}
        }
        self.states.delete(state_id).await
    }

    async fn secret_exists(&self, credential_id: &str) -> Result<bool, McpHostError> {
        let mut secret = self
            .secrets
            .lookup(credential_id, MAX_STORED_SECRET_BYTES)
            .await?;
        let exists = secret.is_some();
        if let Some(secret) = secret.as_mut() {
            secret.fill(0);
        }
        Ok(exists)
    }
}

fn setup_requirement(reference: &McpConnectionAuth) -> Option<(&str, CredentialRelayKind)> {
    match reference {
        McpConnectionAuth::SecretServiceBearer { credential_id }
        | McpConnectionAuth::SecretServiceHeader { credential_id, .. } => {
            Some((credential_id, CredentialRelayKind::ApiToken))
        }
        McpConnectionAuth::OAuth {
            registration: super::McpOAuthRegistration::PreRegistered { credential_id },
            ..
        } => Some((credential_id, CredentialRelayKind::OAuthClient)),
        McpConnectionAuth::None
        | McpConnectionAuth::GhCli { .. }
        | McpConnectionAuth::OAuth { .. } => None,
    }
}

fn kind_name(kind: CredentialRelayKind) -> &'static str {
    match kind {
        CredentialRelayKind::ApiToken => "api_token",
        CredentialRelayKind::OAuthClient => "oauth_client",
    }
}
