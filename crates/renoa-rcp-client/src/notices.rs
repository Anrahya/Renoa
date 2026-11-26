//! Surface operations on setup notices: registering this device's notice
//! key, listing notices held for it, and acknowledging notices the owner acted
//! on or could not open.

use renoa_rcp_wire::{ClientMessage, HeldNotice, NoticeKey, ServerMessage};
use uuid::Uuid;

use crate::{ClientError, Connection, unexpected};

impl Connection {
    /// Registers or replaces this device's P-256 notice public key so nodes
    /// can seal setup links to it. Only a connection that proved its device
    /// key (via [`crate::connect_with_key`]) may call this.
    ///
    /// # Errors
    ///
    /// Returns a transport failure or the coordinator's refusal (such as
    /// `InvalidRole` when the connection did not prove a device key).
    pub async fn set_notice_key(&self, public_key: NoticeKey) -> Result<(), ClientError> {
        match self
            .request(|request_id| ClientMessage::SetNoticeKey {
                request_id,
                public_key,
            })
            .await?
        {
            ServerMessage::NoticeKeySet { .. } => Ok(()),
            other => Err(unexpected("notice_key_set", &other)),
        }
    }

    /// Lists this device's unexpired held setup notices, soonest to expire
    /// first. Only a connection that proved its device key (via
    /// [`crate::connect_with_key`]) may call this.
    ///
    /// # Errors
    ///
    /// Returns a transport failure or the coordinator's refusal (such as
    /// `InvalidRole` when the connection did not prove a device key).
    pub async fn list_notices(&self) -> Result<Vec<HeldNotice>, ClientError> {
        match self
            .request(|request_id| ClientMessage::ListNotices { request_id })
            .await?
        {
            ServerMessage::NoticeList { notices, .. } => Ok(notices),
            other => Err(unexpected("notice_list", &other)),
        }
    }

    /// Acknowledges `notice_id`, removing it for this device. Repeating the
    /// call for a notice that is already gone succeeds. Only a connection that
    /// proved its device key (via [`crate::connect_with_key`]) may call this.
    ///
    /// # Errors
    ///
    /// Returns a transport failure or the coordinator's refusal (such as
    /// `InvalidRole` when the connection did not prove a device key).
    pub async fn acknowledge_notice(&self, notice_id: Uuid) -> Result<(), ClientError> {
        match self
            .request(|request_id| ClientMessage::AcknowledgeNotice {
                request_id,
                notice_id,
            })
            .await?
        {
            ServerMessage::NoticeAcknowledged { .. } => Ok(()),
            other => Err(unexpected("notice_acknowledged", &other)),
        }
    }
}
