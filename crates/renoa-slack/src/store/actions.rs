use rusqlite::params;

use super::{DeliveryState, Store};
use crate::SlackError;

impl Store {
    pub(crate) async fn claim_action(
        &self,
        seq: i64,
        call: String,
        stage: String,
        digest: Vec<u8>,
    ) -> Result<DeliveryState, SlackError> {
        self.run(move |db| {
            let tx = db.transaction()?;
            let running: bool = tx.query_row("SELECT state='running' FROM requests WHERE seq=?1", [seq], |row| row.get(0))?;
            if !running { return Err(SlackError::Invalid("setup action is not attached to an active request".to_owned())); }
            // Only the fingerprint is retained. Setup URL fragments contain
            // secrets; the Host owns their durable recovery and re-emits them.
            tx.execute("INSERT OR IGNORE INTO setup_actions(request_seq,call_id,stage,digest,state) VALUES (?1,?2,?3,?4,'pending')", params![seq,call,stage,digest])?;
            let (saved, status): (Vec<u8>,String) = tx.query_row("SELECT digest,state FROM setup_actions WHERE request_seq=?1 AND call_id=?2 AND stage=?3", params![seq,call,stage], |row| Ok((row.get(0)?,row.get(1)?)))?;
            if saved != digest { return Err(SlackError::Invalid("setup action identity was reused with changed content".to_owned())); }
            let result = match status.as_str() {
                "pending" => {
                    tx.execute("UPDATE setup_actions SET state='sending' WHERE request_seq=?1 AND call_id=?2 AND stage=?3", params![seq,call,stage])?;
                    DeliveryState::Sending
                }
                "sent" => DeliveryState::Sent,
                "failed" => DeliveryState::Failed,
                "sending" | "unknown" => DeliveryState::Unknown,
                _ => return Err(SlackError::Invalid("invalid setup delivery state".to_owned())),
            };
            tx.commit()?;
            Ok(result)
        }).await
    }

    pub(crate) async fn action_state(
        &self,
        seq: i64,
        call: String,
        stage: String,
        status: DeliveryState,
        ts: Option<String>,
        error: Option<String>,
    ) -> Result<(), SlackError> {
        self.run(move |db| {
            if db.execute("UPDATE setup_actions SET state=?4,slack_ts=?5,error=?6 WHERE request_seq=?1 AND call_id=?2 AND stage=?3 AND state='sending'", params![seq,call,stage,status.as_str(),ts,error])? != 1 {
                return Err(SlackError::Invalid("setup action is not sending".to_owned()));
            }
            Ok(())
        }).await
    }
}
