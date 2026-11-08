//! Proving a device to the coordinator: enrolling it, and authenticating a
//! connection with its credential and, for a key-bound device, its key.

use renoa_rcp_wire::{
    ClientMessage, DeviceCredentials, EnrollmentToken, JSON_WS_VERSION, ServerMessage,
    device_proof_message,
};

use crate::{ClientError, DeviceKey, Socket, open, receive, send, sign_in};

/// Exchanges a single-use enrollment token for this surface's device credential.
///
/// # Errors
///
/// Returns a transport failure or the coordinator's refusal.
pub async fn enroll(
    endpoint: &str,
    token: EnrollmentToken,
) -> Result<DeviceCredentials, ClientError> {
    let mut socket = open(endpoint).await?;
    send(
        &mut socket,
        &ClientMessage::Enroll {
            version: JSON_WS_VERSION,
            token,
        },
    )
    .await?;
    match receive(&mut socket).await? {
        ServerMessage::Enrolled {
            version,
            credentials,
        } if version == JSON_WS_VERSION => Ok(credentials),
        ServerMessage::Error { code, message, .. } => Err(ClientError::Rejected { code, message }),
        _ => Err(ClientError::Protocol(
            "coordinator did not return a device credential".to_owned(),
        )),
    }
}

/// Authenticates `socket` as the device `credentials` names, answering a
/// key-bound device's challenge with `key`.
pub(crate) async fn authenticate(
    socket: &mut Socket,
    credentials: DeviceCredentials,
    key: Option<&dyn DeviceKey>,
) -> Result<(), ClientError> {
    let device_id = credentials.device_id;
    send(
        socket,
        &ClientMessage::Authenticate {
            version: JSON_WS_VERSION,
            credentials,
        },
    )
    .await?;
    // The coordinator challenges a connection once; a key is never asked to
    // sign twice for one.
    let mut answered = false;
    loop {
        match receive(socket).await? {
            ServerMessage::Authenticated { version } if version == JSON_WS_VERSION => return Ok(()),
            ServerMessage::DeviceChallenge { nonce } if !answered => {
                let Some(key) = key else {
                    return Err(ClientError::Protocol(
                        "this device registered a key; connect with it".to_owned(),
                    ));
                };
                let signature = sign_in::signed(key, &device_proof_message(device_id, &nonce))?;
                send(socket, &ClientMessage::DeviceProof { signature }).await?;
                answered = true;
            }
            ServerMessage::Error { code, message, .. } => {
                return Err(ClientError::Rejected { code, message });
            }
            _ => {
                return Err(ClientError::Protocol(
                    "coordinator did not authenticate the surface".to_owned(),
                ));
            }
        }
    }
}
