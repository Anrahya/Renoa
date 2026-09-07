use rusqlite::{OptionalExtension as _, params};
use uuid::Uuid;

use super::{Admission, Store, uuid};
use crate::{SlackError, commands::Command, ingress::Incoming};

impl Store {
    #[cfg(test)]
    pub(crate) async fn admit(
        &self,
        input: Incoming,
        observed_at_ms: i64,
    ) -> Result<Admission, SlackError> {
        self.admit_with_agent(input, observed_at_ms, super::AgentSelection::Unchanged)
            .await
    }

    pub(crate) async fn admit_with_agent(
        &self,
        mut input: Incoming,
        observed_at_ms: i64,
        selection: super::AgentSelection,
    ) -> Result<Admission, SlackError> {
        self.run(move |connection| {
            let transaction = connection.transaction()?;
            let receipt: Option<(String,String)> = transaction.query_row(
                "SELECT channel,message_ts FROM receipts WHERE event_id=?1", [&input.event_id], |row| Ok((row.get(0)?,row.get(1)?))
            ).optional()?;
            if receipt.is_some_and(|(channel,ts)|channel!=input.topic.channel || ts!=input.message_ts) {
                return Err(SlackError::Invalid("Slack reused an event identity for another message".to_owned()));
            }
            let existing: Option<(String,String)> = transaction.query_row(
                "SELECT thread,input FROM messages WHERE channel=?1 AND message_ts=?2",
                params![input.topic.channel,input.message_ts], |row|Ok((row.get(0)?,row.get(1)?))
            ).optional()?;
            if let Some((thread,text)) = existing {
                if thread!=input.topic.thread || text!=input.text {
                    return Err(SlackError::Invalid("Slack reused a message identity with different content".to_owned()));
                }
                transaction.execute("INSERT OR IGNORE INTO receipts VALUES (?1,?2,?3)",params![input.event_id,input.topic.channel,input.message_ts])?;
                transaction.commit()?;
                return Ok(Admission { queued:false,cancel_target:None });
            }
            // Deduplication compares the original Slack topic even when a channel
            // becomes dedicated later. Routing is normalized only after that check.
            let received_thread = input.topic.thread.clone();
            let dedicated: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM bot_channels WHERE channel_id=?1)",[&input.topic.channel],|r|r.get(0))?;
            if dedicated { input.topic.thread.clear(); input.starts_conversation = true; }
            let current: Option<String> = transaction.query_row(
                "SELECT session_id FROM conversations WHERE channel=?1 AND thread=?2",
                params![input.topic.channel, input.topic.thread], |row| row.get(0),
            ).optional()?;
            if current.is_none() && !input.starts_conversation {
                transaction.execute("INSERT INTO messages VALUES (?1,?2,?3,?4,NULL)", params![input.topic.channel,input.message_ts,received_thread,input.text])?;
                transaction.execute("INSERT INTO receipts VALUES (?1,?2,?3)", params![input.event_id,input.topic.channel,input.message_ts])?;
                transaction.commit()?;
                return Ok(Admission { queued: false, cancel_target: None });
            }
            let selection = if dedicated && matches!(Command::parse(&input.text), Command::Agent(Some(_))) {
                super::AgentSelection::Rejected("This channel belongs to its specialist. Use Arcee's DM to switch agents; !new starts a fresh conversation here.".to_owned())
            } else { selection };
            let (command, selected) = match selection {
                super::AgentSelection::Unchanged => (Command::parse(&input.text), None),
                super::AgentSelection::Selected(id) => (Command::Agent(Some(id.to_string())), Some(id.to_string())),
                super::AgentSelection::Rejected(reason) => (Command::Notice(reason), None),
            };
            let inherited: Option<String> = if let Some(current) = &current {
                transaction.query_row("SELECT agent_id FROM sessions WHERE session_id=?1", [current], |row| row.get(0))?
            } else { None };
            let selected_agent = selected.clone().or(inherited);
            let session_id = match current {
                Some(id) if !matches!(command, Command::New) && selected.is_none() => id,
                _ => {
                    let id = Uuid::new_v4().to_string();
                    transaction.execute("INSERT INTO sessions(session_id,channel,thread,agent_id) VALUES (?1, ?2, ?3, ?4)", params![id, input.topic.channel, input.topic.thread, selected_agent])?;
                    transaction.execute("INSERT INTO conversations VALUES (?1, ?2, ?3) ON CONFLICT(channel,thread) DO UPDATE SET session_id=excluded.session_id", params![input.topic.channel, input.topic.thread, id])?;
                    id
                }
            };
            let target: Option<String> = if matches!(command, Command::Cancel) {
                transaction.query_row("SELECT request_id FROM requests WHERE channel=?1 AND thread=?2 AND executes_model=1 AND state IN('queued','running') AND cancel_requested=0 ORDER BY seq LIMIT 1", params![input.topic.channel, input.topic.thread], |row| row.get(0)).optional()?
            } else { None };
            if let Some(target) = &target {
                transaction.execute("UPDATE requests SET cancel_requested=1 WHERE request_id=?1", [target])?;
            }
            let request_id = Uuid::new_v4().to_string();
            transaction.execute(
                "INSERT INTO requests(request_id, channel, thread, message_ts, command_json, executes_model, session_id, observed_at_ms, state, cancel_target, surface_context)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'queued',?9,?10)",
                params![request_id, input.topic.channel, input.topic.thread, input.message_ts, serde_json::to_string(&command)?, command.executes_model(), session_id, observed_at_ms, target,
                    matches!(command, Command::Prompt(_)).then_some(crate::surface_context::CONTEXT)],
            )?;
            transaction.execute("INSERT INTO messages VALUES (?1,?2,?3,?4,?5)",params![input.topic.channel,input.message_ts,received_thread,input.text,request_id])?;
            transaction.execute("INSERT INTO receipts VALUES (?1,?2,?3)", params![input.event_id,input.topic.channel,input.message_ts])?;
            let cancel_target = target.as_deref().map(uuid).transpose()?;
            transaction.commit()?;
            Ok(Admission { queued: true, cancel_target })
        }).await
    }
}
