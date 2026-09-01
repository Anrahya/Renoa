use std::path::Path;

use futures_util::{SinkExt as _, StreamExt as _};
use renoa_control::{ClientMessage, EnrollmentToken, JSON_WS_VERSION, ServerMessage};
use serde::Deserialize;
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};

use crate::{
    error::ServiceError,
    private_file::{read_secret, require_new_secret_path, write_new_secret},
};

const MAX_ENROLLMENT_MESSAGE_BYTES: usize = 64 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EnrollmentDocument {
    token: EnrollmentToken,
}

pub(crate) async fn enroll(
    endpoint: &str,
    enrollment_path: &Path,
    output_path: &Path,
) -> Result<(), ServiceError> {
    require_new_secret_path(output_path)?;
    let enrollment = read_enrollment(enrollment_path)?;
    let websocket = WebSocketConfig::default()
        .max_message_size(Some(MAX_ENROLLMENT_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_ENROLLMENT_MESSAGE_BYTES));
    let (mut socket, _) = connect_async_with_config(endpoint, Some(websocket), false).await?;
    let request = serde_json::to_string(&ClientMessage::Enroll {
        version: JSON_WS_VERSION,
        token: enrollment.token,
    })
    .map_err(ServiceError::Serialization)?;
    socket.send(Message::Text(request.into())).await?;
    let response = receive_response(&mut socket).await?;
    let credentials = match response {
        ServerMessage::Enrolled {
            version,
            credentials,
        } if version == JSON_WS_VERSION => credentials,
        ServerMessage::Enrolled { version, .. } => {
            return Err(ServiceError::EnrollmentProtocol(format!(
                "coordinator enrolled protocol version {version}, expected {JSON_WS_VERSION}"
            )));
        }
        ServerMessage::Error { code, message, .. } => {
            return Err(ServiceError::EnrollmentProtocol(format!(
                "coordinator rejected enrollment ({code:?}): {message}"
            )));
        }
        _ => {
            return Err(ServiceError::EnrollmentProtocol(
                "coordinator did not return a device credential".to_owned(),
            ));
        }
    };
    let mut encoded = serde_json::to_vec(&credentials).map_err(|source| {
        ServiceError::IssuedCredentialNotEncoded {
            path: output_path.to_path_buf(),
            source,
        }
    })?;
    encoded.push(b'\n');
    write_new_secret(output_path, &encoded).map_err(|source| {
        ServiceError::IssuedCredentialNotSaved {
            path: output_path.to_path_buf(),
            source,
        }
    })?;
    let _ = socket.close(None).await;
    Ok(())
}

fn read_enrollment(path: &Path) -> Result<EnrollmentDocument, ServiceError> {
    let bytes = read_secret(path)?;
    serde_json::from_slice(&bytes).map_err(|source| ServiceError::Json {
        path: path.to_path_buf(),
        source,
    })
}

async fn receive_response(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Result<ServerMessage, ServiceError> {
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(json))) => {
                return serde_json::from_str(&json).map_err(|source| ServiceError::Json {
                    path: Path::new("coordinator response").to_path_buf(),
                    source,
                });
            }
            Some(Ok(Message::Ping(payload))) => socket.send(Message::Pong(payload)).await?,
            Some(Ok(Message::Pong(_))) => {}
            Some(Ok(Message::Close(_))) | None => {
                return Err(ServiceError::EnrollmentProtocol(
                    "coordinator closed before returning a device credential".to_owned(),
                ));
            }
            Some(Ok(Message::Binary(_) | Message::Frame(_))) => {
                return Err(ServiceError::EnrollmentProtocol(
                    "coordinator returned a non-JSON enrollment message".to_owned(),
                ));
            }
            Some(Err(error)) => return Err(error.into()),
        }
    }
}
