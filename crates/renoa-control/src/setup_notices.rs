//! Plugin setup links for the owner's signed-in apps, sealed to each device.
//!
//! A signed-in app registers a P-256 public key it can agree with
//! (`set_notice_key`). A node lists the keys of its owner's active devices
//! (`list_notice_keys`), seals one notice per device, and puts each
//! (`put_notice`). The coordinator keeps ciphertext only, outside the task
//! journal, and wakes the owner. An app lists (`list_notices`) and
//! acknowledges (`acknowledge_notice`) only its own device's notices; a node
//! withdraws the notices it put (`withdraw_notices`) when a link stops its
//! turn. A notice
//! expires with its link, and revoking a device deletes its key and notices.
//!
//! A notice id names one recipient: the first put wins, so a put retried
//! after a lost answer stores nothing new.

mod store;

use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    ControlError, ErrorCode, MAX_APPLICATION_MESSAGE_BYTES, NodeId, NoticeKey, SealedNotice,
    ServerMessage, control_log,
    coordinator::CoordinatorState,
    device_admin::{Authority, SurfaceSession},
    wire::{send_control_error, send_error},
};
pub(crate) use store::{create_schema, forget_device};
use store::{now_ms, validate};

/// The largest sealed notice the coordinator stores: the largest a node seals.
/// Its plaintext is the JSON around the longest kind and expiry (80 bytes), a
/// label of at most 128 bytes that JSON escapes to at most 256, and a link of
/// at most 16 KiB whose every byte past the shortest prefix that opens a query,
/// `https://a/?` (11 bytes), may be a backslash JSON doubles: 33,093 bytes. The
/// 94-byte envelope makes 33,187, which base64url without padding makes
/// 44,250. `renoa-node`'s tests seal that notice and find it this long.
pub const MAX_SEALED_NOTICE_BYTES: usize = 44_250;
/// The most notices one `notice_list` carries, so that the frame stays within
/// the binding's largest message: 65 bytes of frame around entries of at most
/// 98 bytes plus the sealed notice. A device sees the rest once it has
/// acknowledged the ones before them.
pub const MAX_LISTED_NOTICES: usize =
    (MAX_APPLICATION_MESSAGE_BYTES - 65) / (MAX_SEALED_NOTICE_BYTES + 98);
/// A device's notice operations, from a signed-in app.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NoticeOperation {
    SetKey(NoticeKey),
    List,
    Acknowledge(Uuid),
}

/// A node's notice operations.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NodeNoticeOperation {
    ListKeys,
    Put(SealedNotice),
    Withdraw(Vec<Uuid>),
}

/// Answers a signed-in app's notice operation. Only a device that proved its
/// key holds a key and notices; a browser ticket or a bearer credential does
/// not.
pub(crate) async fn handle(
    state: &CoordinatorState,
    outgoing: &mpsc::Sender<ServerMessage>,
    request_id: u64,
    session: &SurfaceSession,
    operation: NoticeOperation,
) {
    // A device that proved its key, never a bearer credential alone: a thin
    // chat surface's credential sits in a config file, and whoever holds a
    // notice key receives the owner's setup links.
    let Authority::KeyedDevice(device_id) = session.authority else {
        return send_error(
            outgoing,
            Some(request_id),
            ErrorCode::InvalidRole,
            "setup notices belong to an app that proved its device key; \
             this connection did not",
        )
        .await;
    };
    let reply =
        match operation {
            NoticeOperation::SetKey(key) => {
                if key.point().is_none() {
                    Err(ControlError::invalid(
                        "public_key must be an uncompressed P-256 point (65 bytes starting 0x04), \
                     base64url without padding",
                    ))
                } else {
                    state
                        .store
                        .set_notice_key(device_id, key)
                        .await
                        .map(|()| ServerMessage::NoticeKeySet { request_id })
                }
            }
            NoticeOperation::List => state.store.device_notices(device_id).await.map(|notices| {
                ServerMessage::NoticeList {
                    request_id,
                    notices,
                }
            }),
            NoticeOperation::Acknowledge(notice_id) => state
                .store
                .acknowledge_notice(device_id, notice_id)
                .await
                .map(|()| ServerMessage::NoticeAcknowledged { request_id }),
        };
    match reply {
        Ok(reply) => {
            let _ = outgoing.send(reply).await;
        }
        Err(error) => send_control_error(outgoing, Some(request_id), &error).await,
    }
}

/// Answers a node's notice operation.
pub(crate) async fn handle_node(
    state: &CoordinatorState,
    outgoing: &mpsc::Sender<ServerMessage>,
    node_id: NodeId,
    request_id: u64,
    operation: NodeNoticeOperation,
) {
    let reply = match operation {
        NodeNoticeOperation::ListKeys => state
            .store
            .owner_notice_keys(node_id)
            .await
            .map(|keys| ServerMessage::NoticeKeys { request_id, keys }),
        NodeNoticeOperation::Put(notice) => {
            let notice_id = notice.notice_id;
            let put = match now_ms().and_then(|now_ms| validate(&notice, now_ms)) {
                Ok(()) => state.store.put_notice(node_id, notice).await,
                Err(error) => Err(error),
            };
            put.map(|owner| {
                if let Some(principal_id) = owner {
                    control_log::event(
                        "info",
                        "setup_notice_put",
                        &serde_json::json!({
                            "node_id": node_id,
                            "principal_id": principal_id,
                            "notice_id": notice_id,
                        }),
                    );
                    state.push_wakes.notify_one();
                }
                ServerMessage::NoticePut { request_id }
            })
        }
        NodeNoticeOperation::Withdraw(notice_ids) => state
            .store
            .withdraw_notices(node_id, notice_ids)
            .await
            .map(|()| ServerMessage::NoticesWithdrawn { request_id }),
    };
    match reply {
        Ok(reply) => {
            let _ = outgoing.send(reply).await;
        }
        Err(error) => send_control_error(outgoing, Some(request_id), &error).await,
    }
}
