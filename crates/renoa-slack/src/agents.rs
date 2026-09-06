use renoa_kernel::AgentId;
use uuid::Uuid;

use crate::{
    SlackError,
    commands::Command,
    socket::Receiver,
    store::{AgentSelection, Store},
};

impl Receiver {
    pub(crate) async fn select_agent(&self, text: &str) -> Result<AgentSelection, SlackError> {
        let Command::Agent(Some(value)) = Command::parse(text) else {
            return Ok(AgentSelection::Unchanged);
        };
        let id = if value == "arcee" {
            self.store.operator_agent().await?
        } else if let Ok(id) = Uuid::parse_str(&value) {
            id
        } else {
            return Ok(AgentSelection::Rejected("Use !agent to list bots, !agent <id> to start a bot conversation, or !agent arcee.".to_owned()));
        };
        if id != self.store.operator_agent().await?
            && self.host.bot(AgentId::from_uuid(id)).await?.is_none()
        {
            return Ok(AgentSelection::Rejected(
                "That agent is not in this Host. Use !agent to list available bots.".to_owned(),
            ));
        }
        Ok(AgentSelection::Selected(id))
    }
}

impl Store {
    pub(crate) async fn operator_agent(&self) -> Result<Uuid, SlackError> {
        self.run(|connection| {
            let id: String = connection.query_row(
                "SELECT agent_id FROM identity WHERE singleton=1",
                [],
                |row| row.get(0),
            )?;
            super::store::uuid(&id)
        })
        .await
    }

    pub(crate) async fn session_agent(&self, session: Uuid) -> Result<Uuid, SlackError> {
        self.run(move |connection| {
            let id:String=connection.query_row("SELECT COALESCE(s.agent_id,i.agent_id) FROM sessions s CROSS JOIN identity i WHERE s.session_id=?1 AND i.singleton=1",[session.to_string()],|row|row.get(0))?;
            super::store::uuid(&id)
        }).await
    }
}
