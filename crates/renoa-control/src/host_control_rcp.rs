//! `call_host`: a signed-in app calls its owner's Host over RCP, as a browser
//! does under `/v1/host` (`host_control_http`). The relay is the same
//! (`host_calls`); here the app proved its key on the connection, and the
//! answer travels back as a `host_answer` frame.

use std::sync::Arc;

use renoa_protocol::PrincipalId;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

use crate::{
    ErrorCode, HostMethod, ServerMessage, control_log,
    coordinator::CoordinatorState,
    device_admin::SurfaceSession,
    host_calls::{
        HOST_UNREACHABLE, HostCallError, HostReply, HostRequest, INVALID_REPLY, MAX_BODY_BYTES,
        call, relayable,
    },
    wire::send_error,
};

/// How many calls one connection may have waiting on its Host at once.
pub(crate) const MAX_CALLS_PER_CONNECTION: usize = 8;

/// The longest path a call may name. The Host's longest is far shorter.
const MAX_PATH_BYTES: usize = 1024;

/// The calls a connection has waiting; one per connection.
pub(crate) fn call_slots() -> Arc<Semaphore> {
    Arc::new(Semaphore::new(MAX_CALLS_PER_CONNECTION))
}

/// Checks the call and sends it on. A refusal is answered here, before the
/// connection reads its next message, so a peer that does not read its answers
/// is slowed like any other. An admitted call waits on the Host beside the
/// connection's other requests and ends with the connection.
pub(crate) async fn call_host(
    state: Arc<CoordinatorState>,
    outgoing: mpsc::Sender<ServerMessage>,
    cancelled: CancellationToken,
    session: &SurfaceSession,
    request_id: u64,
    request: HostRequest,
) {
    let admitted = admit(session, &request);
    let permit = match admitted {
        Ok(permit) => permit,
        Err((code, message)) => {
            send_error(&outgoing, Some(request_id), code, message).await;
            return;
        }
    };
    let principal_id = session.principal_id;
    tokio::spawn(async move {
        let _permit = permit;
        let outcome = tokio::select! {
            biased;
            () = cancelled.cancelled() => return,
            outcome = call(&state, principal_id, request) => outcome,
        };
        // A connection ended meanwhile, as by its device's revocation, hears nothing more.
        if !cancelled.is_cancelled() {
            answer(&outgoing, request_id, principal_id, outcome).await;
        }
    });
}

fn admit(
    session: &SurfaceSession,
    request: &HostRequest,
) -> Result<OwnedSemaphorePermit, (ErrorCode, &'static str)> {
    if !session.authority.may_call_host() {
        return Err((
            ErrorCode::InvalidRole,
            "The Host is controlled from a signed-in app here, or from a browser at its Host address.",
        ));
    }
    check(request).map_err(|message| (ErrorCode::InvalidMessage, message))?;
    Arc::clone(&session.host_calls)
        .try_acquire_owned()
        .map_err(|_| {
            (
                ErrorCode::Conflict,
                "Too many Host calls from this connection are waiting. Wait for one to answer.",
            )
        })
}

/// What the relay forwards: a path under `/v1/host` of visible ASCII, with no
/// query and no empty or dot segment, and a body only with a change, no larger
/// than the Host accepts.
fn check(request: &HostRequest) -> Result<(), &'static str> {
    let path = &request.path;
    let segments_fit = path
        .strip_prefix("/v1/host")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        && path
            .split('/')
            .skip(1)
            .all(|segment| !matches!(segment, "" | "." | ".."));
    let characters_fit = path
        .bytes()
        .all(|byte| byte.is_ascii_graphic() && byte != b'?' && byte != b'#');
    if !segments_fit || !characters_fit || path.len() > MAX_PATH_BYTES {
        return Err(
            "A Host path is /v1/host or a path under it, such as /v1/host/agents, in visible ASCII with no query.",
        );
    }
    match (&request.body, request.method) {
        (Some(_), HostMethod::Get) => Err("A GET call has no body; send body null."),
        (Some(body), _) if body.len() > MAX_BODY_BYTES => {
            Err("The call's body is larger than the Host accepts (64 KiB).")
        }
        _ => Ok(()),
    }
}

async fn answer(
    outgoing: &mpsc::Sender<ServerMessage>,
    request_id: u64,
    principal_id: PrincipalId,
    outcome: Result<HostReply, HostCallError>,
) {
    let (code, message) = match outcome {
        Ok(reply) if relayable(reply.status) => {
            let _ = outgoing
                .send(ServerMessage::HostAnswer {
                    request_id,
                    status: reply.status,
                    body: reply.body,
                })
                .await;
            return;
        }
        // The Host may have acted before its answer went wrong, so the outcome is
        // unknown, as for a dropped call; `internal` is never a refusal.
        Ok(reply) => {
            control_log::event(
                "warn",
                "host_call_invalid_status",
                &serde_json::json!({ "status": reply.status }),
            );
            (ErrorCode::Internal, INVALID_REPLY)
        }
        Err(HostCallError::Store(error)) => {
            control_log::event(
                "warn",
                "host_call_store_failed",
                &serde_json::json!({ "principal_id": principal_id, "error": error.to_string() }),
            );
            (ErrorCode::Internal, HOST_UNREACHABLE)
        }
        Err(error @ HostCallError::NotAnOwner) => (ErrorCode::InvalidRole, error.message()),
        // Whether a change took effect is unknown, so these are never refusals:
        // the caller retries under the operation identity the Host records.
        Err(
            error @ (HostCallError::Offline
            | HostCallError::Ambiguous
            | HostCallError::Disconnected
            | HostCallError::TimedOut),
        ) => (ErrorCode::NodeOffline, error.message()),
    };
    send_error(outgoing, Some(request_id), code, message).await;
}
