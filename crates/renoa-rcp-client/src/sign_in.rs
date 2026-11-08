//! A computer's sign-in by a code its owner approves from a signed-in app
//! (`docs/identity-v0.md`, "Signing in a computer from a phone"), and the
//! device key that sign-in binds. Neither call bounds how long it waits for
//! the coordinator; a caller that needs a bound wraps it in its own timeout.

use std::sync::LazyLock;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use renoa_rcp_wire::{
    ApprovedCredentials, AwaitingApproval, CollectDeviceRequest, DEVICE_REQUEST_COLLECT_PATH,
    DEVICE_REQUESTS_PATH, DeviceCode, DeviceCredentials, DeviceRequest, DeviceRequestCode,
    DeviceRequestProof, ErrorCode, IdentityRefusal, MAX_APPLICATION_MESSAGE_BYTES,
    RequestingDevice, device_request_message,
};
use reqwest::{StatusCode, header::CONTENT_TYPE, redirect::Policy};
use serde::{Serialize, de::DeserializeOwned};
use uuid::Uuid;

use crate::ClientError;

/// A device's P-256 key. The client asks it for signatures and never holds
/// its private half, so the key may live in the platform's key store.
pub trait DeviceKey: Send + Sync {
    /// The uncompressed SEC1 point: `0x04`, then X and Y.
    fn public_key(&self) -> [u8; 65];

    /// An ASN.1 DER ECDSA P-256 SHA-256 signature over `message`.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError::Key`] when the key store cannot sign.
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, ClientError>;
}

/// A request that waits for its owner's approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInRequest {
    /// The code the computer shows its owner.
    pub code: DeviceCode,
    /// When the request ends, in milliseconds since the Unix epoch.
    pub expires_at_ms: i64,
}

/// What collecting a request found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Collected {
    /// No app has approved the request yet.
    Waiting { expires_at_ms: i64 },
    /// An app approved it: the device's credential, bound to its key.
    SignedIn(DeviceCredentials),
}

/// Asks `origin`'s coordinator to sign in `key` as a `surface` device named
/// `label`, under `request_id`, which the caller chose. Asking again with the
/// same id, key, surface and label replaces the code and keeps the expiry, so
/// a caller that lost the answer asks again with the same id.
///
/// # Errors
///
/// Returns a transport failure, [`ClientError::Busy`] while too many requests
/// wait, the key's failure, or the coordinator's refusal.
pub async fn ask_to_sign_in(
    origin: &str,
    request_id: Uuid,
    surface: &str,
    label: &str,
    key: &dyn DeviceKey,
) -> Result<SignInRequest, ClientError> {
    let request = DeviceRequest {
        request_id,
        surface: surface.to_owned(),
        device: RequestingDevice {
            public_key: URL_SAFE_NO_PAD.encode(key.public_key()),
            label: label.to_owned(),
        },
        device_proof: proof(request_id, key)?,
    };
    let (status, body) = post(origin, DEVICE_REQUESTS_PATH, &request).await?;
    if status != StatusCode::OK {
        return Err(refusal(status, &body));
    }
    let DeviceRequestCode {
        code,
        expires_at_ms,
    } = parse(&body)?;
    Ok(SignInRequest {
        code,
        expires_at_ms,
    })
}

/// Collects request `request_id` with `key`. Each collection after an
/// approval, until the request expires, issues a new credential and ends the
/// connections that used the one before, so a caller that lost the answer
/// collects again.
///
/// # Errors
///
/// Returns a transport failure, the key's failure, or the coordinator's
/// refusal: [`ErrorCode::NotFound`] for a request that expired or that it
/// does not hold.
pub async fn collect_sign_in(
    origin: &str,
    request_id: Uuid,
    key: &dyn DeviceKey,
) -> Result<Collected, ClientError> {
    let collection = CollectDeviceRequest {
        request_id,
        device_proof: proof(request_id, key)?,
    };
    let (status, body) = post(origin, DEVICE_REQUEST_COLLECT_PATH, &collection).await?;
    match status {
        StatusCode::ACCEPTED => {
            let AwaitingApproval { expires_at_ms } = parse(&body)?;
            Ok(Collected::Waiting { expires_at_ms })
        }
        StatusCode::OK => {
            let ApprovedCredentials { credentials } = parse(&body)?;
            Ok(Collected::SignedIn(credentials))
        }
        _ => Err(refusal(status, &body)),
    }
}

/// `key`'s signature, encoded as the coordinator reads one.
pub(crate) fn signed(key: &dyn DeviceKey, message: &[u8]) -> Result<String, ClientError> {
    key.sign(message)
        .map(|signature| URL_SAFE_NO_PAD.encode(signature))
}

fn proof(request_id: Uuid, key: &dyn DeviceKey) -> Result<DeviceRequestProof, ClientError> {
    signed(key, &device_request_message(request_id)).map(DeviceRequestProof::new)
}

/// One client for every sign-in call. It follows no redirect: a redirect would
/// carry the request's proof, which collects its credential, somewhere else.
fn http() -> Result<&'static reqwest::Client, ClientError> {
    static CLIENT: LazyLock<Result<reqwest::Client, String>> = LazyLock::new(|| {
        reqwest::Client::builder()
            .redirect(Policy::none())
            .build()
            .map_err(|error| error.to_string())
    });
    CLIENT
        .as_ref()
        .map_err(|error| ClientError::Transport(error.clone()))
}

async fn post(
    origin: &str,
    path: &str,
    body: &impl Serialize,
) -> Result<(StatusCode, Vec<u8>), ClientError> {
    let body =
        serde_json::to_vec(body).map_err(|error| ClientError::Protocol(error.to_string()))?;
    let mut response = http()?
        .post(format!("{}{path}", origin.trim_end_matches('/')))
        .header(CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
        .map_err(|error| ClientError::Transport(error.to_string()))?;
    let status = response.status();
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| ClientError::Transport(error.to_string()))?
    {
        // No answer of the identity routes comes near a frame's size.
        if body.len() + chunk.len() > MAX_APPLICATION_MESSAGE_BYTES {
            return Err(ClientError::Protocol(format!(
                "coordinator answered {status} with a body over {MAX_APPLICATION_MESSAGE_BYTES} bytes"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok((status, body))
}

/// Reads an answer. Its error names no value from the body, which may hold a
/// credential.
fn parse<T: DeserializeOwned>(body: &[u8]) -> Result<T, ClientError> {
    serde_json::from_slice(body).map_err(|_error| {
        ClientError::Protocol("coordinator sent an answer this client cannot read".to_owned())
    })
}

/// The identity routes' refusal, in the terms the RCP binding refuses with.
/// An answer with no refusal body came from something in front of the
/// coordinator, such as a proxy that could not reach it.
fn refusal(status: StatusCode, body: &[u8]) -> ClientError {
    let Ok(IdentityRefusal { code, message }) = parse(body) else {
        return if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
            ClientError::Transport(format!("Renoa could not be reached: {status}"))
        } else {
            ClientError::Protocol(format!("coordinator answered {status} without a refusal"))
        };
    };
    let code = match code.as_str() {
        "capacity_exceeded" => return ClientError::Busy(message),
        "authentication_failed" => ErrorCode::AuthenticationFailed,
        "invalid_request" => ErrorCode::InvalidMessage,
        "conflict" => ErrorCode::Conflict,
        "not_found" => ErrorCode::NotFound,
        "internal" => ErrorCode::Internal,
        other => {
            return ClientError::Protocol(format!("coordinator refused with unknown code {other}"));
        }
    };
    ClientError::Rejected { code, message }
}
