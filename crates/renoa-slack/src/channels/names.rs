use super::Channels;
use crate::{
    SlackError,
    api::{ApiError, SlackApi},
    store::Store,
};
use renoa_local::BotSummary;
use rusqlite::{OptionalExtension as _, params};

pub(super) struct Label {
    pub(super) channel: String,
    pub(super) name: String,
}

fn slug(name: &str) -> String {
    let words = name
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    let joined = words.join("-");
    let bounded = joined.chars().take(60).collect::<String>();
    if bounded.is_empty() {
        "assistant".to_owned()
    } else {
        bounded.trim_end_matches('-').to_owned()
    }
}
impl Store {
    pub(super) async fn label_plan(
        &self,
        bot: &BotSummary,
        advance: bool,
    ) -> Result<Option<Label>, SlackError> {
        let agent = bot.id.to_string();
        let base = slug(&bot.name);
        self.run(move|db|{
            let tx=db.transaction()?;
            let channel:Option<String>=tx.query_row("SELECT channel_id FROM bot_channels WHERE agent_id=?1 AND state='ready'",[&agent],|r|r.get(0)).optional()?;
            let Some(channel)=channel else{return Ok(None)};
            let old:Option<(String,String,i64,bool)>=tx.query_row("SELECT base,desired,suffix,applied FROM bot_channel_labels WHERE agent_id=?1",[&agent],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
            let mut suffix=0;
            if let Some((old_base,name,old_suffix,applied))=old && old_base==base {
                    if applied && !advance{return Ok(None)}
                    if !advance{return Ok(Some(Label{channel,name}))}
                    suffix=old_suffix.checked_add(1).ok_or_else(||SlackError::Invalid("channel suffix overflow".to_owned()))?;
            }
            let mut chosen=None;
            for _ in 0..1000 {
                let next=suffix.checked_add(1).ok_or_else(||SlackError::Invalid("channel suffix overflow".to_owned()))?;
                let name=if suffix==0{base.clone()}else{format!("{base}-{next}")};
                let taken:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM bot_channel_labels WHERE desired=?1 AND agent_id!=?2)",params![name,agent],|r|r.get(0))?;
                if !taken{chosen=Some(name);break;}
                suffix=next;
            }
            let name=chosen.ok_or_else(||SlackError::Invalid("too many conflicting channel names".to_owned()))?;
            tx.execute("INSERT INTO bot_channel_labels(agent_id,base,desired,suffix,applied,error) VALUES(?1,?2,?3,?4,0,NULL) ON CONFLICT(agent_id) DO UPDATE SET base=?2,desired=?3,suffix=?4,applied=0,error=NULL",params![agent,base,name,suffix])?;
            tx.commit()?;Ok(Some(Label{channel,name}))
        }).await
    }
    pub(super) async fn label_result(
        &self,
        agent: String,
        name: String,
        error: Option<String>,
    ) -> Result<(), SlackError> {
        self.run(move|db|{
            db.execute("UPDATE bot_channel_labels SET applied=?3,error=?4 WHERE agent_id=?1 AND desired=?2",params![agent,name,error.is_none(),error])?;Ok(())
        }).await
    }
}
impl SlackApi {
    async fn rename_channel(&self, id: &str, name: &str) -> Result<(), ApiError> {
        #[derive(serde::Deserialize)]
        struct Reply {
            channel: Named,
        }
        #[derive(serde::Deserialize)]
        struct Named {
            id: String,
            name: String,
        }
        let current: Reply = self
            .bot_get("conversations.info", &[("channel", id)])
            .await?;
        if current.channel.id != id {
            return Err(ApiError::Unknown(
                "channel lookup returned another identity".to_owned(),
            ));
        }
        if current.channel.name == name {
            return Ok(());
        }
        let response: Reply = self
            .bot_call(
                "conversations.rename",
                serde_json::json!({"channel":id,"name":name}),
            )
            .await?;
        if response.channel.id != id || response.channel.name != name {
            return Err(ApiError::Unknown(
                "renamed channel did not match its retained identity/name".to_owned(),
            ));
        }
        Ok(())
    }
}
impl Channels {
    pub(super) async fn label(&self, bot: &BotSummary) -> Result<(), SlackError> {
        let Some(label) = self.store.label_plan(bot, false).await? else {
            return Ok(());
        };
        if self.shutdown.is_cancelled() {
            return Ok(());
        }
        match self.api.rename_channel(&label.channel, &label.name).await {
            Ok(()) => {
                self.store
                    .label_result(bot.id.to_string(), label.name, None)
                    .await
            }
            Err(ApiError::Rejected(code)) if code == "name_taken" => {
                // The rename failed definitively. Reserve another short name;
                // a future pass retries it on the same bound channel ID.
                self.store.label_plan(bot, true).await?;
                Ok(())
            }
            Err(error) => {
                self.store
                    .label_result(bot.id.to_string(), label.name, Some(error.to_string()))
                    .await?;
                if let ApiError::RateLimited(delay) = error {
                    crate::service::pause(&self.shutdown, delay).await;
                }
                Ok(())
            }
        }
    }
}
