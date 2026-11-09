//! Signing this Mac in: a code to type on the owner's phone, then the
//! credential the phone approved.

use std::sync::Arc;

use renoa_rcp_client::{ClientError, Collected, ask_to_sign_in, collect_sign_in};
use renoa_rcp_wire::{DeviceCode, DeviceCredentials, ErrorCode};
use tokio::{sync::mpsc, time::sleep};
use uuid::Uuid;

use super::{Endpoints, Pace, within};
use crate::key::MacKey;

/// What signing in shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignInStep {
    /// The code to type on the phone.
    Showing(DeviceCode),
    /// Whether Renoa answered the last call.
    Reachable(bool),
    /// The credential is kept; the Mac is signed in.
    SignedIn(DeviceCredentials),
    /// The approved credential could not be kept.
    NotKept(String),
}

/// Asks for a sign-in and waits for the owner's phone to approve it. Only an
/// expired request is replaced by a new one with a new code; any other
/// failure asks again under the same id after a pause, which converges on
/// one request. The credential is kept with `keep` before the views hear of
/// it, so an approval is kept even when no view is left to show it. Stops
/// once no view listens.
pub async fn sign_in(
    endpoints: Endpoints,
    key: Arc<MacKey>,
    label: String,
    pace: Pace,
    keep: impl Fn(&DeviceCredentials) -> Result<(), String>,
    steps: mpsc::UnboundedSender<SignInStep>,
) {
    let mut reachable = true;
    let mut heard = |error: Option<&ClientError>| {
        let answered = !matches!(error, Some(ClientError::Transport(_)));
        if answered != reachable {
            reachable = answered;
            let _ = steps.send(SignInStep::Reachable(answered));
        }
    };
    loop {
        let request_id = Uuid::new_v4();
        let asked = loop {
            let asked = ask_to_sign_in(&endpoints.origin, request_id, "mac", &label, key.as_ref());
            match within(asked).await {
                Ok(asked) => break asked,
                Err(error) => heard(Some(&error)),
            }
            if steps.is_closed() {
                return;
            }
            sleep(pace.retry_after).await;
        };
        heard(None);
        if steps.send(SignInStep::Showing(asked.code)).is_err() {
            return;
        }
        loop {
            sleep(pace.collect_every).await;
            match within(collect_sign_in(&endpoints.origin, request_id, key.as_ref())).await {
                Ok(Collected::Waiting { .. }) => heard(None),
                Ok(Collected::SignedIn(credentials)) => {
                    let step = match keep(&credentials) {
                        Ok(()) => SignInStep::SignedIn(credentials),
                        Err(problem) => SignInStep::NotKept(problem),
                    };
                    let _ = steps.send(step);
                    return;
                }
                // The request expired: show a new code.
                Err(ClientError::Rejected {
                    code: ErrorCode::NotFound,
                    ..
                }) => break,
                Err(error) => {
                    heard(Some(&error));
                    sleep(pace.retry_after).await;
                }
            }
            if steps.is_closed() {
                return;
            }
        }
    }
}
