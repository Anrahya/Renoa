use rusqlite::{OptionalExtension as _, params};
use uuid::Uuid;

use super::{Delivery, DeliveryState, ReplyState, Store, Work, uuid};
use crate::{SlackError, ingress::Topic};

impl Store {
    pub(crate) async fn next_work(&self) -> Result<Option<Work>, SlackError> {
        self.run(|connection| {
            let raw = connection.query_row(
                "SELECT seq,channel,thread,session_id,request_id,command_json,observed_at_ms,reply_ts,reply_state,cancel_target,surface_context
                 FROM requests WHERE state='queued' ORDER BY seq LIMIT 1",
                [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get::<_, String>(3)?,row.get::<_, String>(4)?,row.get::<_, String>(5)?,row.get(6)?,row.get(7)?,row.get::<_, String>(8)?,row.get::<_, Option<String>>(9)?,row.get::<_,Option<String>>(10)?)),
            ).optional()?;
            raw.map(|(seq, channel, thread, session, request, command, observed_at_ms, reply_ts, reply_state, cancel_target, surface_context)| Ok(Work {
                seq, surface_context, topic: Topic {channel, thread}, session_id: uuid(&session)?, request_id: uuid(&request)?,
                command: serde_json::from_str(&command)?, observed_at_ms, reply_ts, reply_pending: reply_state == "pending",
                cancel_target: cancel_target.as_deref().map(uuid).transpose()?,
            })).transpose()
        }).await
    }

    pub(crate) async fn mark_running(&self, seq: i64) -> Result<(), SlackError> {
        self.run(move |connection| {
            if connection.execute(
                "UPDATE requests SET state='running' WHERE seq=?1 AND state='queued'",
                [seq],
            )? != 1
            {
                return Err(SlackError::Invalid("work is no longer queued".to_owned()));
            }
            Ok(())
        })
        .await
    }

    pub(crate) async fn cancelled(&self, request: Uuid) -> Result<bool, SlackError> {
        self.run(move |connection| {
            Ok(connection.query_row(
                "SELECT cancel_requested FROM requests WHERE request_id=?1",
                [request.to_string()],
                |row| row.get(0),
            )?)
        })
        .await
    }

    pub(crate) async fn reply_state(
        &self,
        seq: i64,
        state: ReplyState,
        ts: Option<String>,
    ) -> Result<(), SlackError> {
        self.run(move |connection| {
            connection.execute(
                "UPDATE requests SET reply_state=?2, reply_ts=?3 WHERE seq=?1",
                params![seq, state.as_str(), ts],
            )?;
            Ok(())
        })
        .await
    }

    pub(crate) async fn finish(&self, seq: i64, result: String) -> Result<(), SlackError> {
        self.run(move |connection| {
            let transaction = connection.transaction()?;
            let reply: Option<String> = transaction.query_row(
                "SELECT reply_ts FROM requests WHERE seq=?1 AND state='running'",
                [seq],
                |row| row.get(0),
            )?;
            for (index, text) in crate::formatting::chunks(&result).iter().enumerate() {
                let chunk = i64::try_from(index)
                    .map_err(|_| SlackError::Invalid("too many output chunks".to_owned()))?;
                let ts = if chunk == 0 { reply.as_deref() } else { None };
                transaction.execute(
                    "INSERT INTO deliveries VALUES (?1,?2,?3,?4,'pending',NULL)",
                    params![seq, chunk, text, ts],
                )?;
            }
            transaction.execute("UPDATE setup_actions SET state='failed',error='Request ended before delivery' WHERE request_seq=?1 AND state='pending'", [seq])?;
            transaction.execute(
                "UPDATE requests SET state='ready', result=?2 WHERE seq=?1",
                params![seq, result],
            )?;
            transaction.commit()?;
            Ok(())
        })
        .await
    }

    pub(crate) async fn next_delivery(&self) -> Result<Option<Delivery>, SlackError> {
        self.run(|connection| {
            connection.execute("UPDATE requests SET state='done' WHERE state='ready' AND NOT EXISTS(SELECT 1 FROM deliveries WHERE request_seq=requests.seq AND state IN('pending','sending'))", [])?;
            Ok(connection.query_row(
                "SELECT d.request_seq,d.chunk,r.channel,r.thread,d.text,d.slack_ts FROM deliveries d JOIN requests r ON r.seq=d.request_seq WHERE d.state='pending' AND NOT EXISTS(SELECT 1 FROM deliveries prior WHERE prior.request_seq=d.request_seq AND prior.chunk<d.chunk AND prior.state!='sent') ORDER BY d.request_seq,d.chunk LIMIT 1",
                [], |row| Ok(Delivery {seq:row.get(0)?,chunk:row.get(1)?,topic:Topic {channel:row.get(2)?,thread:row.get(3)?},text:row.get(4)?,ts:row.get(5)?}),
            ).optional()?)
        }).await
    }

    pub(crate) async fn delivery_state(
        &self,
        seq: i64,
        chunk: i64,
        state: DeliveryState,
        ts: Option<String>,
        error: Option<String>,
    ) -> Result<(), SlackError> {
        self.run(move |connection| {
            connection.execute("UPDATE deliveries SET state=?3, slack_ts=COALESCE(?4,slack_ts), error=?5 WHERE request_seq=?1 AND chunk=?2", params![seq,chunk,state.as_str(),ts,error])?;
            Ok(())
        }).await
    }
}
