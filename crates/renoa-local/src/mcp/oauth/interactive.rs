use renoa_agent::ToolUpdates;
use tokio_util::sync::CancellationToken;

use super::{
    INTERACTIVE_LOCK_WAIT, McpOAuthAuthorizationRequest, OAuthCoordinator,
    adapter_may_have_dispatched, browser,
    callback_receiver::OAuthCallbackReceiver,
    lock, process,
    process::OAuthResult,
    resolution::unknown,
    secret_store::{OAuthSecretBundle, PendingCallback},
    store::{OAuthCallbackIdentity, OAuthFlow, OAuthPhase},
};
use crate::mcp::{McpCredentialHeader, McpHostError, McpOAuthError};

mod support;

use support::{
    authorization_url, emit_redirect, random_state, state_string, validate_state_identity,
};

struct InteractiveAuthorization<'a> {
    connection_id: &'a str,
    display_name: Option<&'a str>,
    endpoint: &'a str,
    credential_id: &'a str,
    reference: &'a crate::mcp::McpConnectionAuth,
    operation_id: &'a str,
    restart: bool,
    updates: Option<&'a ToolUpdates>,
    cancellation: CancellationToken,
}

impl OAuthCoordinator {
    pub(super) async fn authorize(
        &self,
        request: McpOAuthAuthorizationRequest<'_>,
        cancellation: CancellationToken,
    ) -> Result<McpCredentialHeader, McpHostError> {
        let credential_id = Self::credential_id(request.reference)?;
        let _lock = lock::acquire(
            self.locks.clone(),
            request.connection_id,
            INTERACTIVE_LOCK_WAIT,
            &cancellation,
        )
        .await?;
        if let Some(authorization) = self
            .replay_receipt(
                request.connection_id,
                request.endpoint,
                credential_id,
                request.operation_id,
                cancellation.clone(),
            )
            .await?
        {
            return Ok(authorization);
        }
        let bundle = self
            .secrets
            .load(credential_id, cancellation.clone())
            .await?;
        if request.restart {
            self.flows.delete(request.connection_id).await?;
        }
        let authorization = InteractiveAuthorization {
            connection_id: request.connection_id,
            display_name: request.display_name,
            endpoint: request.endpoint,
            credential_id,
            reference: request.reference,
            operation_id: request.operation_id,
            restart: request.restart,
            updates: request.updates,
            cancellation,
        };
        let flow = self.flows.load(request.connection_id).await?;
        match flow {
            Some(flow) => self.resume(&authorization, flow, bundle).await,
            None => {
                self.begin(&authorization, authorization.restart, bundle)
                    .await
            }
        }
    }

    async fn begin(
        &self,
        context: &InteractiveAuthorization<'_>,
        force_reauthorization: bool,
        prior: Option<OAuthSecretBundle>,
    ) -> Result<McpCredentialHeader, McpHostError> {
        let csrf_state = random_state()?;
        let receiver = OAuthCallbackReceiver::new(self, &csrf_state, &context.cancellation).await?;
        let redirect_uri = receiver.redirect_uri();
        let flow = OAuthFlow::interactive(
            context.connection_id,
            context.operation_id,
            OAuthPhase::BeginInFlight,
            receiver.identity(),
            receiver.expires_at_ms(),
        )?;
        self.flows.put(&flow).await?;
        let result = {
            let registration = self
                .adapter_registration(context.reference, context.cancellation.clone())
                .await?;
            process::begin(
                self.adapter()?,
                process::OAuthBegin {
                    endpoint: context.endpoint,
                    csrf_state: &csrf_state,
                    redirect_uri: &redirect_uri,
                    force_reauthorization,
                    registration: &registration,
                    prior: prior.as_ref().map(|bundle| &bundle.adapter_state),
                },
                context.cancellation.clone(),
            )
            .await
        };
        match result {
            Ok(OAuthResult::Redirect {
                authorization_url,
                state,
            }) => {
                validate_state_identity(&state, &csrf_state, &redirect_uri)?;
                let bundle = OAuthSecretBundle::new(state);
                if let Err(error) = self
                    .secrets
                    .store(context.credential_id, &bundle, context.cancellation.clone())
                    .await
                {
                    self.mark_unknown(context.connection_id, context.operation_id)
                        .await?;
                    return Err(unknown(context.connection_id, &error.to_string()));
                }
                let waiting = flow.with_phase(OAuthPhase::AwaitingCallback)?;
                self.flows.put(&waiting).await?;
                self.wait_for_callback(context, waiting, bundle, authorization_url, receiver)
                    .await
            }
            Ok(OAuthResult::Authorized {
                authorization,
                state,
                ..
            }) => {
                self.complete_authorized(
                    &flow,
                    context.credential_id,
                    context.operation_id,
                    authorization,
                    state,
                    context.cancellation.clone(),
                )
                .await
            }
            Ok(OAuthResult::Failed { failure, state }) => {
                self.complete_failure(
                    &flow,
                    context.credential_id,
                    context.operation_id,
                    failure,
                    state,
                    context.cancellation.clone(),
                )
                .await
            }
            Ok(OAuthResult::RefreshRequired { .. }) => {
                self.mark_unknown(context.connection_id, context.operation_id)
                    .await?;
                Err(unknown(
                    context.connection_id,
                    "OAuth begin returned an impossible refresh-only result",
                ))
            }
            Err(error) if adapter_may_have_dispatched(&error) => {
                self.mark_unknown(context.connection_id, context.operation_id)
                    .await?;
                Err(unknown(context.connection_id, &error.to_string()))
            }
            Err(error) => {
                self.flows.delete(context.connection_id).await?;
                Err(error.into())
            }
        }
    }

    async fn resume(
        &self,
        context: &InteractiveAuthorization<'_>,
        mut flow: OAuthFlow,
        bundle: Option<OAuthSecretBundle>,
    ) -> Result<McpCredentialHeader, McpHostError> {
        if flow.phase == OAuthPhase::Unknown {
            return Err(unknown(context.connection_id, "previous exchange"));
        }
        if flow.phase == OAuthPhase::RefreshInFlight {
            return self
                .recover_refresh(
                    context.connection_id,
                    context.endpoint,
                    context.credential_id,
                    &flow,
                    context.cancellation.clone(),
                )
                .await;
        }
        let Some(bundle) = bundle else {
            self.mark_unknown(context.connection_id, &flow.operation_id)
                .await?;
            return Err(unknown(
                context.connection_id,
                "durable OAuth state is missing",
            ));
        };
        match flow.phase {
            OAuthPhase::BeginInFlight => {
                if bundle.adapter_state.get("authorization_url").is_none() {
                    return self
                        .recover_completed_begin(
                            context.endpoint,
                            context.credential_id,
                            flow,
                            bundle,
                            context.operation_id,
                            context.cancellation.clone(),
                        )
                        .await;
                }
                if validate_saved_callback(&flow, &bundle, self.relay.as_ref()).is_err() {
                    self.mark_unknown(context.connection_id, &flow.operation_id)
                        .await?;
                    return Err(unknown(
                        context.connection_id,
                        "client registration finished without matching durable redirect state",
                    ));
                }
                flow = flow.with_phase(OAuthPhase::AwaitingCallback)?;
                self.flows.put(&flow).await?;
                self.resume_callback(context, flow, bundle).await
            }
            OAuthPhase::AwaitingCallback if bundle.pending_callback.is_some() => {
                flow = flow.with_phase(OAuthPhase::CallbackReady)?;
                self.flows.put(&flow).await?;
                self.exchange_pending(context, flow, bundle).await
            }
            OAuthPhase::AwaitingCallback => self.resume_callback(context, flow, bundle).await,
            OAuthPhase::CallbackReady => self.exchange_pending(context, flow, bundle).await,
            OAuthPhase::ExchangeInFlight => {
                self.recover_exchange(
                    context.endpoint,
                    context.credential_id,
                    flow,
                    bundle,
                    context.operation_id,
                    context.cancellation.clone(),
                )
                .await
            }
            OAuthPhase::RefreshInFlight | OAuthPhase::Unknown => Err(McpHostError::Invalid(
                "MCP OAuth phase changed while resuming authorization".to_owned(),
            )),
        }
    }

    async fn resume_callback(
        &self,
        context: &InteractiveAuthorization<'_>,
        flow: OAuthFlow,
        bundle: OAuthSecretBundle,
    ) -> Result<McpCredentialHeader, McpHostError> {
        if validate_saved_callback(&flow, &bundle, self.relay.as_ref()).is_err() {
            self.mark_unknown(&flow.connection_id, &flow.operation_id)
                .await?;
            return Err(unknown(
                &flow.connection_id,
                "durable callback state does not match its saved listener",
            ));
        }
        let callback = flow.callback.ok_or_else(|| {
            McpOAuthError::Invalid("OAuth callback identity is missing".to_owned())
        })?;
        let expiry = flow
            .expires_at_ms
            .ok_or_else(|| McpOAuthError::Invalid("OAuth callback expiry is missing".to_owned()))?;
        let receiver = OAuthCallbackReceiver::resume(self, callback, expiry).await?;
        let url = authorization_url(&bundle.adapter_state)?.to_owned();
        self.wait_for_callback(context, flow, bundle, url, receiver)
            .await
    }

    async fn wait_for_callback(
        &self,
        context: &InteractiveAuthorization<'_>,
        flow: OAuthFlow,
        mut bundle: OAuthSecretBundle,
        authorization_url: String,
        receiver: OAuthCallbackReceiver,
    ) -> Result<McpCredentialHeader, McpHostError> {
        emit_redirect(
            context.updates,
            &flow.connection_id,
            context.display_name,
            &authorization_url,
            flow.expires_at_ms,
        )
        .await;
        if receiver.opens_local_browser() {
            browser::open(&self.browser, &authorization_url, &context.cancellation).await?;
        }
        let csrf_state = state_string(&bundle.adapter_state, "csrf_state")?;
        let mut callback_result = receiver.receive(csrf_state, &context.cancellation).await?;
        if let Some(error) = callback_result.take_rejection() {
            self.persist_callback_rejection(&flow, context.operation_id, &error)
                .await?;
            callback_result.acknowledge_browser().await;
            self.acknowledge_relay_callback(&flow, &context.cancellation)
                .await?;
            self.flows.delete(&flow.connection_id).await?;
            return Err(McpOAuthError::CallbackRejected(error).into());
        }
        let authorization_code = callback_result.take_authorization_code().ok_or_else(|| {
            McpOAuthError::Invalid("OAuth callback did not contain a result".to_owned())
        })?;
        bundle.pending_callback = Some(PendingCallback {
            authorization_code,
            issuer: callback_result.take_issuer(),
        });
        self.secrets
            .store(context.credential_id, &bundle, context.cancellation.clone())
            .await?;
        let ready = flow.with_phase(OAuthPhase::CallbackReady)?;
        self.flows.put(&ready).await?;
        callback_result.acknowledge_browser().await;
        self.exchange_pending(context, ready, bundle).await
    }

    async fn exchange_pending(
        &self,
        context: &InteractiveAuthorization<'_>,
        flow: OAuthFlow,
        bundle: OAuthSecretBundle,
    ) -> Result<McpCredentialHeader, McpHostError> {
        let pending = bundle.pending_callback.as_ref().ok_or_else(|| {
            McpOAuthError::Invalid("durable OAuth callback code is missing".to_owned())
        })?;
        self.acknowledge_relay_callback(&flow, &context.cancellation)
            .await?;
        let in_flight = flow.with_phase(OAuthPhase::ExchangeInFlight)?;
        self.flows.put(&in_flight).await?;
        let result = {
            let registration = self
                .adapter_registration(context.reference, context.cancellation.clone())
                .await?;
            process::exchange(
                self.adapter()?,
                context.endpoint,
                &pending.authorization_code,
                pending.issuer.as_deref(),
                &registration,
                &bundle.adapter_state,
                context.cancellation.clone(),
            )
            .await
        };
        match result {
            Ok(OAuthResult::Authorized {
                authorization,
                state,
                ..
            }) => {
                self.complete_authorized(
                    &flow,
                    context.credential_id,
                    context.operation_id,
                    authorization,
                    state,
                    context.cancellation.clone(),
                )
                .await
            }
            Ok(OAuthResult::Failed { failure, state }) => {
                self.complete_failure(
                    &flow,
                    context.credential_id,
                    context.operation_id,
                    failure,
                    state,
                    context.cancellation.clone(),
                )
                .await
            }
            Ok(OAuthResult::Redirect { state, .. } | OAuthResult::RefreshRequired { state }) => {
                self.complete_authorization_required(
                    &flow,
                    context.credential_id,
                    context.operation_id,
                    state,
                    context.cancellation.clone(),
                )
                .await
            }
            Err(error) if adapter_may_have_dispatched(&error) => {
                self.mark_unknown(&flow.connection_id, &flow.operation_id)
                    .await?;
                Err(unknown(&flow.connection_id, &error.to_string()))
            }
            Err(error) => {
                self.flows.delete(&flow.connection_id).await?;
                Err(error.into())
            }
        }
    }

    pub(super) async fn acknowledge_relay_callback(
        &self,
        flow: &OAuthFlow,
        cancellation: &CancellationToken,
    ) -> Result<(), McpHostError> {
        let Some(OAuthCallbackIdentity::Relay(relay_id)) = flow.callback else {
            return Ok(());
        };
        let expires_at_ms = flow
            .expires_at_ms
            .ok_or_else(|| McpOAuthError::Invalid("OAuth callback expiry is missing".to_owned()))?;
        let relay = self.relay.as_ref().ok_or_else(|| {
            McpOAuthError::CallbackUnavailable("saved callback relay is not configured".to_owned())
        })?;
        relay
            .acknowledge_saved(relay_id, expires_at_ms, cancellation)
            .await
    }
}

fn validate_saved_callback(
    flow: &OAuthFlow,
    bundle: &OAuthSecretBundle,
    relay: Option<&super::relay::OAuthRelayClient>,
) -> Result<(), McpHostError> {
    let expected_redirect = match flow
        .callback
        .ok_or_else(|| McpOAuthError::Invalid("OAuth callback identity is missing".to_owned()))?
    {
        OAuthCallbackIdentity::Loopback(port) => {
            format!("http://127.0.0.1:{port}/oauth/callback")
        }
        OAuthCallbackIdentity::Relay(_) => relay
            .ok_or_else(|| {
                McpOAuthError::CallbackUnavailable(
                    "saved callback relay is not configured".to_owned(),
                )
            })?
            .callback_uri()
            .to_owned(),
    };
    let csrf_state = state_string(&bundle.adapter_state, "csrf_state")?;
    validate_state_identity(&bundle.adapter_state, csrf_state, &expected_redirect)?;
    authorization_url(&bundle.adapter_state)?;
    Ok(())
}
