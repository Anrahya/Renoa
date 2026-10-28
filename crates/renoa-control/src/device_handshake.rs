//! The second step of authenticating a key-bound device: the coordinator
//! challenges the connection, and only a signature by the registered key lets
//! it continue. A bearer credential alone never reaches an operation.

use axum::extract::ws::WebSocket;
use futures_util::stream::SplitStream;
use tokio::sync::mpsc;

use crate::{
    ClientMessage, DeviceId, ErrorCode, ServerMessage, control_log,
    device_key::{DevicePublicKey, DeviceSignature, proof_message},
    identity::random_nonce,
    wire::{next_application_message, send_control_error, send_error},
};

/// Challenges the connection of `device_id` and waits for its proof. Returns
/// whether the connection proved it holds `key`; on `false` it has already
/// been answered and must be closed.
pub(crate) async fn prove_possession(
    receiver: &mut SplitStream<WebSocket>,
    outgoing: &mpsc::Sender<ServerMessage>,
    device_id: DeviceId,
    key: &DevicePublicKey,
) -> bool {
    // The nonce lives only in this connection: a proof made for another
    // connection, or recorded earlier, signs a different message.
    let nonce = match random_nonce() {
        Ok(nonce) => nonce,
        Err(error) => {
            send_control_error(outgoing, None, &error).await;
            return false;
        }
    };
    let challenge = ServerMessage::DeviceChallenge {
        nonce: nonce.clone(),
    };
    if outgoing.send(challenge).await.is_err() {
        return false;
    }
    let Some(message) = next_application_message(receiver, outgoing).await else {
        return false;
    };
    let verified = match message {
        ClientMessage::DeviceProof { signature } => DeviceSignature::parse(&signature)
            .is_some_and(|signature| key.verify(&proof_message(device_id, &nonce), &signature)),
        _ => false,
    };
    if !verified {
        control_log::event(
            "warn",
            "device_proof_rejected",
            &serde_json::json!({ "device_id": device_id }),
        );
        send_error(
            outgoing,
            None,
            ErrorCode::AuthenticationFailed,
            "authentication failed",
        )
        .await;
    }
    verified
}
