use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::{
    OAuthCoordinator, process,
    process::OAuthResult,
    resolution::{ensure_local_state_unchanged, unknown},
    secret::OAuthSecretBundle,
    store::{OAuthFlow, OAuthPhase, OAuthReceipt},
};
use crate::mcp::{
    McpAdapterError, McpCredentialHeader, McpHostError, McpOAuthError, McpOutcomeCertainty,
    McpRemoteFailure,
};

impl OAuthCoordinator {
    pub(super) async fn recover_exchange(
        &self,
        endpoint: &str,
        credential_id: &str,
        flow: OAuthFlow,
        bundle: OAuthSecretBundle,
        current_operation_id: &str,
        cancellation: CancellationToken,
    ) -> Result<McpCredentialHeader, McpHostError> {
        if bundle.pending_callback.is_some() {
            self.mark_unknown(&flow.connection_id, &flow.operation_id)
                .await?;
            return Err(unknown(
                &flow.connection_id,
                "authorization-code exchange has no durable terminal result",
            ));
        }
        if let Ok(OAuthResult::Authorized {
            authorization,
            state,
            ..
        }) = process::token(
            self.adapter()?,
            endpoint,
            &bundle.adapter_state,
            cancellation.clone(),
        )
        .await
        {
            ensure_local_state_unchanged(&bundle.adapter_state, &state)?;
            self.complete_authorized(
                &flow,
                credential_id,
                current_operation_id,
                authorization,
                state,
                cancellation,
            )
            .await
        } else {
            self.mark_unknown(&flow.connection_id, &flow.operation_id)
                .await?;
            Err(unknown(
                &flow.connection_id,
                "authorization-code exchange has no durable terminal result",
            ))
        }
    }

    pub(super) async fn recover_completed_begin(
        &self,
        endpoint: &str,
        credential_id: &str,
        flow: OAuthFlow,
        bundle: OAuthSecretBundle,
        current_operation_id: &str,
        cancellation: CancellationToken,
    ) -> Result<McpCredentialHeader, McpHostError> {
        if let Ok(OAuthResult::Authorized {
            authorization,
            state,
            ..
        }) = process::token(
            self.adapter()?,
            endpoint,
            &bundle.adapter_state,
            cancellation.clone(),
        )
        .await
        {
            ensure_local_state_unchanged(&bundle.adapter_state, &state)?;
            self.complete_authorized(
                &flow,
                credential_id,
                current_operation_id,
                authorization,
                state,
                cancellation,
            )
            .await
        } else {
            self.mark_unknown(&flow.connection_id, &flow.operation_id)
                .await?;
            Err(unknown(
                &flow.connection_id,
                "OAuth begin has no matching durable redirect or terminal credential",
            ))
        }
    }

    pub(super) async fn replay_receipt(
        &self,
        connection_id: &str,
        endpoint: &str,
        credential_id: &str,
        operation_id: &str,
        cancellation: CancellationToken,
    ) -> Result<Option<McpCredentialHeader>, McpHostError> {
        let Some(receipt) = self.flows.receipt(connection_id, operation_id).await? else {
            return Ok(None);
        };
        match receipt {
            OAuthReceipt::Authorized => {
                let Some(bundle) = self
                    .secrets
                    .load(credential_id, cancellation.clone())
                    .await?
                else {
                    return Err(McpOAuthError::ReceiptUnavailable(connection_id.to_owned()).into());
                };
                match process::token(
                    self.adapter()?,
                    endpoint,
                    &bundle.adapter_state,
                    cancellation,
                )
                .await?
                {
                    OAuthResult::Authorized {
                        authorization,
                        state,
                        ..
                    } => {
                        ensure_local_state_unchanged(&bundle.adapter_state, &state)?;
                        self.delete_owned_flow(connection_id, operation_id).await?;
                        Ok(Some(authorization))
                    }
                    OAuthResult::Failed { failure, state } => {
                        ensure_local_state_unchanged(&bundle.adapter_state, &state)?;
                        Err(McpAdapterError::Remote(failure).into())
                    }
                    OAuthResult::Redirect { .. } | OAuthResult::RefreshRequired { .. } => {
                        Err(McpOAuthError::ReceiptUnavailable(connection_id.to_owned()).into())
                    }
                }
            }
            OAuthReceipt::RemoteFailure => {
                Err(McpOAuthError::ReceiptFailure(connection_id.to_owned()).into())
            }
            OAuthReceipt::CallbackRejected { error } => {
                Err(McpOAuthError::CallbackRejected(error).into())
            }
            OAuthReceipt::AuthorizationRequired => {
                Err(McpOAuthError::AuthorizationRequired(connection_id.to_owned()).into())
            }
        }
    }

    pub(super) async fn complete_authorized(
        &self,
        flow: &OAuthFlow,
        credential_id: &str,
        current_operation_id: &str,
        authorization: McpCredentialHeader,
        state: Value,
        cancellation: CancellationToken,
    ) -> Result<McpCredentialHeader, McpHostError> {
        if let Err(error) = self
            .secrets
            .store(credential_id, &OAuthSecretBundle::new(state), cancellation)
            .await
        {
            self.mark_unknown(&flow.connection_id, &flow.operation_id)
                .await?;
            return Err(unknown(&flow.connection_id, &error.to_string()));
        }
        if let Err(error) = self
            .record_receipt_pair(
                &flow.connection_id,
                &flow.operation_id,
                current_operation_id,
                &OAuthReceipt::Authorized,
            )
            .await
        {
            self.mark_unknown(&flow.connection_id, &flow.operation_id)
                .await?;
            return Err(unknown(&flow.connection_id, &error.to_string()));
        }
        self.flows.delete(&flow.connection_id).await?;
        Ok(authorization)
    }

    pub(super) async fn complete_failure(
        &self,
        flow: &OAuthFlow,
        credential_id: &str,
        current_operation_id: &str,
        failure: McpRemoteFailure,
        state: Value,
        cancellation: CancellationToken,
    ) -> Result<McpCredentialHeader, McpHostError> {
        if let Err(error) = self
            .secrets
            .store(credential_id, &OAuthSecretBundle::new(state), cancellation)
            .await
        {
            self.mark_unknown(&flow.connection_id, &flow.operation_id)
                .await?;
            return Err(unknown(&flow.connection_id, &error.to_string()));
        }
        if failure.certainty() == McpOutcomeCertainty::Unknown {
            self.mark_unknown(&flow.connection_id, &flow.operation_id)
                .await?;
        } else {
            let receipt = OAuthReceipt::RemoteFailure;
            if let Err(error) = self
                .record_receipt_pair(
                    &flow.connection_id,
                    &flow.operation_id,
                    current_operation_id,
                    &receipt,
                )
                .await
            {
                self.mark_unknown(&flow.connection_id, &flow.operation_id)
                    .await?;
                return Err(unknown(&flow.connection_id, &error.to_string()));
            }
            self.flows.delete(&flow.connection_id).await?;
        }
        Err(McpAdapterError::Remote(failure).into())
    }

    pub(super) async fn complete_authorization_required(
        &self,
        flow: &OAuthFlow,
        credential_id: &str,
        current_operation_id: &str,
        state: Value,
        cancellation: CancellationToken,
    ) -> Result<McpCredentialHeader, McpHostError> {
        if let Err(error) = self
            .secrets
            .store(credential_id, &OAuthSecretBundle::new(state), cancellation)
            .await
        {
            self.mark_unknown(&flow.connection_id, &flow.operation_id)
                .await?;
            return Err(unknown(&flow.connection_id, &error.to_string()));
        }
        if let Err(error) = self
            .record_receipt_pair(
                &flow.connection_id,
                &flow.operation_id,
                current_operation_id,
                &OAuthReceipt::AuthorizationRequired,
            )
            .await
        {
            self.mark_unknown(&flow.connection_id, &flow.operation_id)
                .await?;
            return Err(unknown(&flow.connection_id, &error.to_string()));
        }
        self.flows.delete(&flow.connection_id).await?;
        Err(McpOAuthError::AuthorizationRequired(flow.connection_id.clone()).into())
    }

    pub(super) async fn complete_callback_rejection(
        &self,
        flow: &OAuthFlow,
        current_operation_id: &str,
        error: String,
    ) -> Result<McpCredentialHeader, McpHostError> {
        if let Err(receipt_error) = self
            .record_receipt_pair(
                &flow.connection_id,
                &flow.operation_id,
                current_operation_id,
                &OAuthReceipt::CallbackRejected {
                    error: error.clone(),
                },
            )
            .await
        {
            self.mark_unknown(&flow.connection_id, &flow.operation_id)
                .await?;
            return Err(unknown(&flow.connection_id, &receipt_error.to_string()));
        }
        self.flows.delete(&flow.connection_id).await?;
        Err(McpOAuthError::CallbackRejected(error).into())
    }

    pub(super) async fn record_refresh_failure(
        &self,
        connection_id: &str,
        operation_id: &str,
    ) -> Result<(), McpHostError> {
        self.flows
            .put_receipt(connection_id, operation_id, &OAuthReceipt::RemoteFailure)
            .await
    }

    async fn record_receipt_pair(
        &self,
        connection_id: &str,
        original_operation_id: &str,
        current_operation_id: &str,
        receipt: &OAuthReceipt,
    ) -> Result<(), McpHostError> {
        self.flows
            .put_receipt(connection_id, original_operation_id, receipt)
            .await?;
        if current_operation_id != original_operation_id {
            self.flows
                .put_receipt(connection_id, current_operation_id, receipt)
                .await?;
        }
        Ok(())
    }

    async fn delete_owned_flow(
        &self,
        connection_id: &str,
        operation_id: &str,
    ) -> Result<(), McpHostError> {
        if self.flows.load(connection_id).await?.is_some_and(|flow| {
            flow.operation_id == operation_id && flow.phase != OAuthPhase::Unknown
        }) {
            self.flows.delete(connection_id).await?;
        }
        Ok(())
    }
}
