use tokio_util::sync::CancellationToken;

use super::{
    OAuthCoordinator, REFRESH_LOCK_WAIT, adapter_may_have_dispatched, lock, process,
    process::OAuthResult,
    secret_store::OAuthSecretBundle,
    store::{OAuthFlow, OAuthPhase},
};
use crate::mcp::{
    McpAdapterError, McpConnectionAuth, McpCredentialHeader, McpHostError, McpOAuthError,
    McpOutcomeCertainty,
};

struct RefreshAttempt<'a> {
    connection_id: &'a str,
    endpoint: &'a str,
    credential_id: &'a str,
    registration: &'a process::OAuthRegistration,
    operation_id: &'a str,
    state: serde_json::Value,
    cancellation: CancellationToken,
}

impl OAuthCoordinator {
    pub(super) async fn resolve(
        &self,
        connection_id: &str,
        endpoint: &str,
        reference: &McpConnectionAuth,
        operation_id: &str,
        cancellation: CancellationToken,
    ) -> Result<McpCredentialHeader, McpHostError> {
        let credential_id = Self::credential_id(reference)?;
        let _lock = lock::acquire(
            self.locks.clone(),
            connection_id,
            REFRESH_LOCK_WAIT,
            &cancellation,
        )
        .await?;
        if let Some(authorization) = self
            .replay_receipt(
                connection_id,
                endpoint,
                credential_id,
                operation_id,
                cancellation.clone(),
            )
            .await?
        {
            return Ok(authorization);
        }
        if let Some(flow) = self.flows.load(connection_id).await? {
            match flow.phase {
                OAuthPhase::RefreshInFlight => {
                    return self
                        .recover_refresh(
                            connection_id,
                            endpoint,
                            credential_id,
                            &flow,
                            cancellation,
                        )
                        .await;
                }
                OAuthPhase::Unknown => return Err(unknown(connection_id, "previous exchange")),
                OAuthPhase::BeginInFlight
                | OAuthPhase::AwaitingCallback
                | OAuthPhase::CallbackReady
                | OAuthPhase::ExchangeInFlight => {
                    return Err(
                        McpOAuthError::AuthorizationRequired(connection_id.to_owned()).into(),
                    );
                }
            }
        }
        let Some(bundle) = self
            .secrets
            .load(credential_id, cancellation.clone())
            .await?
        else {
            return Err(McpOAuthError::AuthorizationRequired(connection_id.to_owned()).into());
        };
        match process::token(
            self.adapter()?,
            endpoint,
            &bundle.adapter_state,
            cancellation.clone(),
        )
        .await?
        {
            OAuthResult::Authorized {
                authorization,
                state,
                ..
            } => {
                ensure_local_state_unchanged(&bundle.adapter_state, &state)?;
                Ok(authorization)
            }
            OAuthResult::RefreshRequired { state } => {
                ensure_local_state_unchanged(&bundle.adapter_state, &state)?;
                let registration = self
                    .adapter_registration(reference, cancellation.clone())
                    .await?;
                self.refresh(RefreshAttempt {
                    connection_id,
                    endpoint,
                    credential_id,
                    registration: &registration,
                    operation_id,
                    state: bundle.adapter_state,
                    cancellation,
                })
                .await
            }
            OAuthResult::Failed { failure, state } => {
                ensure_local_state_unchanged(&bundle.adapter_state, &state)?;
                Err(McpAdapterError::Remote(failure).into())
            }
            OAuthResult::Redirect { .. } => Err(McpAdapterError::Protocol(
                "local OAuth token inspection unexpectedly requested a redirect".to_owned(),
            )
            .into()),
        }
    }

    pub(super) async fn recover_refresh(
        &self,
        connection_id: &str,
        endpoint: &str,
        credential_id: &str,
        flow: &OAuthFlow,
        cancellation: CancellationToken,
    ) -> Result<McpCredentialHeader, McpHostError> {
        let Some(bundle) = self
            .secrets
            .load(credential_id, cancellation.clone())
            .await?
        else {
            self.mark_unknown(connection_id, &flow.operation_id).await?;
            return Err(unknown(
                connection_id,
                "refresh state was not durably stored",
            ));
        };
        match process::token(
            self.adapter()?,
            endpoint,
            &bundle.adapter_state,
            cancellation.clone(),
        )
        .await
        {
            Ok(OAuthResult::Authorized {
                authorization,
                state,
                ..
            }) => {
                ensure_local_state_unchanged(&bundle.adapter_state, &state)?;
                self.flows.delete(connection_id).await?;
                Ok(authorization)
            }
            Ok(
                OAuthResult::RefreshRequired { .. }
                | OAuthResult::Redirect { .. }
                | OAuthResult::Failed { .. },
            )
            | Err(_) => {
                self.mark_unknown(connection_id, &flow.operation_id).await?;
                Err(unknown(
                    connection_id,
                    "previous refresh has no durable terminal result",
                ))
            }
        }
    }

    async fn refresh(
        &self,
        attempt: RefreshAttempt<'_>,
    ) -> Result<McpCredentialHeader, McpHostError> {
        let flow = OAuthFlow::non_interactive(
            attempt.connection_id,
            attempt.operation_id,
            OAuthPhase::RefreshInFlight,
        )?;
        self.flows.put(&flow).await?;
        let result = process::refresh(
            self.adapter()?,
            attempt.endpoint,
            attempt.registration,
            &attempt.state,
            attempt.cancellation.clone(),
        )
        .await;
        match result {
            Ok(OAuthResult::Authorized {
                authorization,
                state,
                ..
            }) => {
                if let Err(error) = self
                    .secrets
                    .store(
                        attempt.credential_id,
                        &OAuthSecretBundle::new(state),
                        attempt.cancellation,
                    )
                    .await
                {
                    self.mark_unknown(attempt.connection_id, attempt.operation_id)
                        .await?;
                    return Err(unknown(attempt.connection_id, &error.to_string()));
                }
                self.flows.delete(attempt.connection_id).await?;
                Ok(authorization)
            }
            Ok(OAuthResult::Failed { failure, state }) => {
                if let Err(error) = self
                    .secrets
                    .store(
                        attempt.credential_id,
                        &OAuthSecretBundle::new(state),
                        attempt.cancellation,
                    )
                    .await
                {
                    self.mark_unknown(attempt.connection_id, attempt.operation_id)
                        .await?;
                    return Err(unknown(attempt.connection_id, &error.to_string()));
                }
                if failure.certainty() == McpOutcomeCertainty::Unknown {
                    self.mark_unknown(attempt.connection_id, attempt.operation_id)
                        .await?;
                } else {
                    if let Err(error) = self
                        .record_refresh_failure(attempt.connection_id, attempt.operation_id)
                        .await
                    {
                        self.mark_unknown(attempt.connection_id, attempt.operation_id)
                            .await?;
                        return Err(unknown(attempt.connection_id, &error.to_string()));
                    }
                    self.flows.delete(attempt.connection_id).await?;
                }
                Err(McpAdapterError::Remote(failure).into())
            }
            Ok(OAuthResult::Redirect { state, .. } | OAuthResult::RefreshRequired { state }) => {
                self.complete_authorization_required(
                    &flow,
                    attempt.credential_id,
                    attempt.operation_id,
                    state,
                    attempt.cancellation,
                )
                .await
            }
            Err(error) if adapter_may_have_dispatched(&error) => {
                self.mark_unknown(attempt.connection_id, attempt.operation_id)
                    .await?;
                Err(unknown(attempt.connection_id, &error.to_string()))
            }
            Err(error) => {
                self.flows.delete(attempt.connection_id).await?;
                Err(error.into())
            }
        }
    }

    pub(super) async fn mark_unknown(
        &self,
        connection_id: &str,
        operation_id: &str,
    ) -> Result<(), McpHostError> {
        self.flows
            .put(&OAuthFlow::non_interactive(
                connection_id,
                operation_id,
                OAuthPhase::Unknown,
            )?)
            .await
    }
}

pub(super) fn unknown(connection_id: &str, detail: &str) -> McpHostError {
    McpOAuthError::OutcomeUnknown {
        connection: connection_id.to_owned(),
        detail: detail.to_owned(),
    }
    .into()
}

pub(super) fn ensure_local_state_unchanged(
    expected: &serde_json::Value,
    returned: &serde_json::Value,
) -> Result<(), McpHostError> {
    if expected == returned {
        Ok(())
    } else {
        Err(McpAdapterError::Protocol(
            "local OAuth token inspection changed durable credential state".to_owned(),
        )
        .into())
    }
}
