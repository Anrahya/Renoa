//! One-time links to a file an agent shared. A signed-in app asks for one
//! with `share_link` and opens it like any web page; `/s/{share}` serves the
//! file once for the ticket in the link (see `share_page_http`). The
//! coordinator keeps only the ticket's digest, until it is used or, once
//! expired, until the next link is issued.

use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use renoa_protocol::PrincipalId;
use rusqlite::{Connection, OptionalExtension as _, params};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    ControlError, ErrorCode, ServerMessage, ShareLink,
    control_schema::open_connection,
    coordinator::CoordinatorState,
    device_admin::SurfaceSession,
    identity::{Secret as _, ShareTicket},
    identity_store::timestamp_millis,
    store::{ControlStore, blocking, id_error, sqlite_error},
    wire::{send_control_error, send_error},
};

/// How long a link works once issued: the app opens it as soon as it has it,
/// and a slow phone connection still reaches it in time.
pub(crate) const SHARE_TICKET_LIFETIME: Duration = Duration::from_secs(120);

/// Unused tickets, by digest, each for one share and the principal it reads as.
pub(crate) fn create_schema(connection: &Connection) -> Result<(), ControlError> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS share_tickets (
                ticket_digest BLOB PRIMARY KEY CHECK (length(ticket_digest) = 32),
                principal_id TEXT NOT NULL,
                share_id TEXT NOT NULL,
                expires_at_ms INTEGER NOT NULL
            );",
        )
        .map_err(sqlite_error)
}

/// Answers `share_link` with a one-time link to `share_id`. A link reads the
/// Host, so only a connection that may call the Host may ask for one: an app
/// that proved its key.
pub(crate) async fn issue(
    state: &CoordinatorState,
    outgoing: &mpsc::Sender<ServerMessage>,
    request_id: u64,
    session: &SurfaceSession,
    share_id: Uuid,
) {
    if !session.authority.may_call_host() {
        send_error(
            outgoing,
            Some(request_id),
            ErrorCode::InvalidRole,
            "Shared files open from a signed-in app here, or from a browser at its Host address.",
        )
        .await;
        return;
    }
    let issued = state
        .store
        .issue_share_ticket(session.principal_id, share_id, SystemTime::now())
        .await;
    match issued {
        Ok(ticket) => {
            let path = ShareLink::new(format!("/s/{share_id}?ticket={}", ticket.expose()));
            let _ = outgoing
                .send(ServerMessage::ShareLinkIssued { request_id, path })
                .await;
        }
        Err(error) => send_control_error(outgoing, Some(request_id), &error).await,
    }
}

impl ControlStore {
    /// Stores a new ticket for `share_id`, and drops every expired one.
    pub(crate) async fn issue_share_ticket(
        &self,
        principal_id: PrincipalId,
        share_id: Uuid,
        now: SystemTime,
    ) -> Result<ShareTicket, ControlError> {
        let ticket = ShareTicket::generate()?;
        let digest = ticket
            .digest()
            .ok_or_else(|| ControlError::store("generated an invalid share ticket"))?;
        let now_ms = timestamp_millis(now)?;
        let expires_at_ms = timestamp_millis(now + SHARE_TICKET_LIFETIME)?;
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = open_connection(&path)?;
            connection
                .execute(
                    "DELETE FROM share_tickets WHERE expires_at_ms <= ?1",
                    [now_ms],
                )
                .map_err(sqlite_error)?;
            connection
                .execute(
                    "INSERT INTO share_tickets(ticket_digest, principal_id, share_id, expires_at_ms)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        digest.as_slice(),
                        principal_id.to_string(),
                        share_id.to_string(),
                        expires_at_ms
                    ],
                )
                .map_err(sqlite_error)?;
            Ok(())
        })
        .await?;
        Ok(ticket)
    }

    /// The principal `ticket` reads `share_id` as, while it is unused and has
    /// not expired. Looking it up does not use it.
    pub(crate) async fn share_ticket_reader(
        &self,
        share_id: Uuid,
        ticket: &ShareTicket,
        now: SystemTime,
    ) -> Result<Option<PrincipalId>, ControlError> {
        self.ticket_statement(
            "SELECT principal_id FROM share_tickets
             WHERE ticket_digest = ?1 AND share_id = ?2 AND expires_at_ms > ?3",
            share_id,
            ticket,
            now,
        )
        .await
    }

    /// Uses `ticket` for `share_id`: the principal it reads as, once, while it
    /// has not expired. A ticket for another share is not used up.
    pub(crate) async fn redeem_share_ticket(
        &self,
        share_id: Uuid,
        ticket: &ShareTicket,
        now: SystemTime,
    ) -> Result<Option<PrincipalId>, ControlError> {
        self.ticket_statement(
            "DELETE FROM share_tickets
             WHERE ticket_digest = ?1 AND share_id = ?2 AND expires_at_ms > ?3
             RETURNING principal_id",
            share_id,
            ticket,
            now,
        )
        .await
    }

    /// Runs `statement`, which names a live ticket for one share as ?1 to ?3,
    /// and returns the principal of the row it finds.
    async fn ticket_statement(
        &self,
        statement: &'static str,
        share_id: Uuid,
        ticket: &ShareTicket,
        now: SystemTime,
    ) -> Result<Option<PrincipalId>, ControlError> {
        let Some(digest) = ticket.digest() else {
            return Ok(None);
        };
        let now_ms = timestamp_millis(now)?;
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = open_connection(&path)?;
            let principal: Option<String> = connection
                .query_row(
                    statement,
                    params![digest.as_slice(), share_id.to_string(), now_ms],
                    |row| row.get(0),
                )
                .optional()
                .map_err(sqlite_error)?;
            principal
                .map(|principal| {
                    principal
                        .parse()
                        .map(PrincipalId::from_uuid)
                        .map_err(id_error)
                })
                .transpose()
        })
        .await
    }
}

#[cfg(test)]
mod tests;
