use crate::{SlackError, store::Store};
use renoa_local::BotSummary;
use rusqlite::{OptionalExtension as _, params};
use uuid::Uuid;

pub(super) struct Provision {
    pub(super) agent: String,
    pub(super) name: String,
    pub(super) state: State,
}
pub(super) enum State {
    Pending,
    Creating,
    Inviting(String),
    Ready,
}

impl Store {
    pub(super) async fn channel_provision(
        &self,
        bot: &BotSummary,
    ) -> Result<Provision, SlackError> {
        let agent = bot.id.to_string();
        let slug: String = bot
            .name
            .chars()
            .filter(char::is_ascii)
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .take(36)
            .collect();
        let name = format!(
            "renoa-{}-{}",
            if slug.trim_matches('-').is_empty() {
                "bot"
            } else {
                slug.trim_matches('-')
            },
            agent.replace('-', "")
        );
        self.run(move |db| {
            db.execute(
                "INSERT OR IGNORE INTO bot_channels(agent_id,name,state) VALUES (?1,?2,'pending')",
                params![agent, name],
            )?;
            let (name, state, channel): (String, String, Option<String>) = db.query_row(
                "SELECT name,state,channel_id FROM bot_channels WHERE agent_id=?1",
                [&agent],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            let state = match (state.as_str(), channel) {
                ("pending", None) => State::Pending,
                ("creating", None) => State::Creating,
                ("inviting", Some(id)) => State::Inviting(id),
                ("ready", Some(_)) => State::Ready,
                _ => {
                    return Err(SlackError::Invalid(
                        "invalid channel provisioning state".to_owned(),
                    ));
                }
            };
            Ok(Provision { agent, name, state })
        })
        .await
    }

    pub(super) async fn channel_state(
        &self,
        agent: &str,
        state: &str,
        channel: Option<&str>,
        error: Option<&str>,
    ) -> Result<(), SlackError> {
        let (agent, state, channel, error) = (
            agent.to_owned(),
            state.to_owned(),
            channel.map(str::to_owned),
            error.map(str::to_owned),
        );
        self.run(move |db| {
            db.execute(
                "UPDATE bot_channels SET state=?2,channel_id=?3,error=?4 WHERE agent_id=?1",
                params![agent, state, channel, error],
            )?;
            Ok(())
        })
        .await
    }

    pub(super) async fn bind_channel(&self, agent: &str, channel: &str) -> Result<(), SlackError> {
        let (agent, channel) = (agent.to_owned(), channel.to_owned());
        self.run(move |db| {
            let tx = db.transaction()?;
            let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM conversations WHERE channel=?1)",[&channel],|r|r.get(0))?;
            if exists { return Err(SlackError::Invalid("specialist channel already has a conversation binding".to_owned())); }
            let session = Uuid::new_v4().to_string();
            tx.execute("INSERT INTO sessions(session_id,channel,thread,agent_id) VALUES (?1,?2,'',?3)",params![session,channel,agent])?;
            tx.execute("INSERT INTO conversations VALUES (?1,'',?2)",params![channel,session])?;
            tx.execute("UPDATE bot_channels SET state='inviting',channel_id=?2,error=NULL WHERE agent_id=?1",params![agent,channel])?;
            tx.commit()?;
            Ok(())
        }).await
    }

    #[cfg(test)]
    pub(crate) async fn dedicated_channel(&self, channel: &str) -> Result<bool, SlackError> {
        let channel = channel.to_owned();
        self.run(move |db| {
            Ok(db.query_row(
                "SELECT EXISTS(SELECT 1 FROM bot_channels WHERE channel_id=?1)",
                [channel],
                |r| r.get(0),
            )?)
        })
        .await
    }

    pub(crate) async fn channel_description(&self, agent: String) -> Result<String, SlackError> {
        self.run(move |db| {
            let row: Option<(String, Option<String>)> = db
                .query_row(
                    "SELECT state,error FROM bot_channels WHERE agent_id=?1",
                    [&agent],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let result = match row {
                Some((state, error)) => {
                    let name: String = db.query_row(
                        "SELECT COALESCE((SELECT desired FROM bot_channel_labels WHERE agent_id=?1 AND applied=1),name) FROM bot_channels WHERE agent_id=?1",
                        [agent],
                        |r| r.get(0),
                    )?;
                    format!(
                        "#{name}: {state}{}",
                        error.map_or_else(String::new, |e| format!(" ({e})"))
                    )
                }
                None => "channel setup pending".to_owned(),
            };
            Ok(result)
        })
        .await
    }
}
