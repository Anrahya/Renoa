use super::Store;
use crate::{SlackError, ingress::Topic};
use renoa_local::AutomationRun;
use rusqlite::{OptionalExtension as _, params};

pub(crate) struct AutomationDelivery {
    pub(crate) run_id: String,
    pub(crate) chunk: i64,
    pub(crate) topic: Topic,
    pub(crate) text: String,
}
impl Store {
    pub(crate) async fn automation_cursor(&self) -> Result<i64, SlackError> {
        self.run(|db| {
            Ok(db.query_row(
                "SELECT sequence FROM automation_delivery_cursor WHERE singleton=1",
                [],
                |r| r.get(0),
            )?)
        })
        .await
    }
    pub(crate) async fn admit_automation_result(
        &self,
        run: AutomationRun,
    ) -> Result<(), SlackError> {
        self.run(move|db|{
            let tx=db.transaction()?;
            let cursor:i64=tx.query_row("SELECT sequence FROM automation_delivery_cursor WHERE singleton=1",[],|r|r.get(0))?;
            if run.sequence<=cursor{return Ok(())}
            let channel:Option<String>=tx.query_row("SELECT channel_id FROM bot_channels WHERE agent_id=?1 AND state='ready'",[run.agent_id.to_string()],|r|r.get(0)).optional()?;
            let output=run.output.ok_or_else(||SlackError::Invalid("automation result is unfinished".to_owned()))?;
            for (index,text) in crate::formatting::chunks(&format!("Automation result\n\n{output}")).into_iter().enumerate(){
                let index=i64::try_from(index).map_err(|_|SlackError::Invalid("too many automation output chunks".to_owned()))?;
                tx.execute("INSERT INTO automation_deliveries(run_id,chunk,agent_id,channel,text,state) VALUES(?1,?2,?3,?4,?5,?6)",params![run.id.to_string(),index,run.agent_id.to_string(),channel,text,if channel.is_some(){"pending"}else{"waiting"}])?;
            }
            tx.execute("UPDATE automation_delivery_cursor SET sequence=?1 WHERE singleton=1",[run.sequence])?;
            tx.commit()?;Ok(())
        }).await
    }
    pub(crate) async fn next_automation_delivery(
        &self,
    ) -> Result<Option<AutomationDelivery>, SlackError> {
        self.run(|db|{
            db.execute("UPDATE automation_deliveries SET channel=(SELECT channel_id FROM bot_channels WHERE agent_id=automation_deliveries.agent_id AND state='ready'),state='pending' WHERE state='waiting' AND EXISTS(SELECT 1 FROM bot_channels WHERE agent_id=automation_deliveries.agent_id AND state='ready')",[])?;
            Ok(db.query_row("SELECT run_id,chunk,channel,text FROM automation_deliveries d WHERE state='pending' AND NOT EXISTS(SELECT 1 FROM automation_deliveries prior WHERE prior.run_id=d.run_id AND prior.chunk<d.chunk AND prior.state!='sent') ORDER BY rowid LIMIT 1",[],|r|Ok(AutomationDelivery{run_id:r.get(0)?,chunk:r.get(1)?,topic:Topic{channel:r.get(2)?,thread:String::new()},text:r.get(3)?})).optional()?)
        }).await
    }
    pub(crate) async fn claim_automation_delivery(
        &self,
        id: String,
        chunk: i64,
    ) -> Result<(), SlackError> {
        self.run(move|db|{
            if db.execute("UPDATE automation_deliveries SET state='sending' WHERE run_id=?1 AND chunk=?2 AND state='pending'",params![id,chunk])?!=1{return Err(SlackError::Invalid("automation delivery is no longer pending".to_owned()))}Ok(())
        }).await
    }
    pub(crate) async fn finish_automation_delivery(
        &self,
        id: String,
        chunk: i64,
        state: super::DeliveryState,
        ts: Option<String>,
        error: Option<String>,
    ) -> Result<(), SlackError> {
        self.run(move|db|{
            if db.execute("UPDATE automation_deliveries SET state=?3,slack_ts=?4,error=?5 WHERE run_id=?1 AND chunk=?2 AND state='sending'",params![id,chunk,state.as_str(),ts,error])?!=1{return Err(SlackError::Invalid("automation delivery is no longer sending".to_owned()))}Ok(())
        }).await
    }
}
