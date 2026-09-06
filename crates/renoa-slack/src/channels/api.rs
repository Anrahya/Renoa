use serde::Deserialize;
use serde_json::json;

use crate::{
    api::{ApiError, SlackApi},
    ingress::valid_id,
};

#[derive(Deserialize)]
pub(super) struct Channel {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) creator: String,
    pub(super) is_private: bool,
    pub(super) is_archived: bool,
}

#[derive(Deserialize)]
struct Created {
    channel: Channel,
}
#[derive(Deserialize)]
struct Page {
    channels: Vec<Channel>,
    response_metadata: Metadata,
}
#[derive(Deserialize)]
struct Metadata {
    next_cursor: String,
}

impl SlackApi {
    pub(super) async fn create_private_channel(&self, name: &str) -> Result<Channel, ApiError> {
        let created: Created = self
            .bot_call(
                "conversations.create",
                json!({"name":name,"is_private":true}),
            )
            .await?;
        Ok(created.channel)
    }

    pub(super) async fn invite_operator(&self, channel: &str, user: &str) -> Result<(), ApiError> {
        match self
            .bot_call::<serde_json::Value>(
                "conversations.invite",
                json!({"channel":channel,"users":user}),
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(ApiError::Rejected(code)) if code == "already_in_channel" => Ok(()),
            Err(error) => Err(error),
        }
    }

    pub(super) async fn find_created_channel(
        &self,
        name: &str,
        bot: &str,
    ) -> Result<Option<Channel>, ApiError> {
        let mut cursor = String::new();
        // A bounded lookup may fail unresolved; it must never trigger another create.
        for _ in 0..100 {
            let page: Page = self
                .bot_get(
                    "conversations.list",
                    &[
                        ("types", "private_channel"),
                        ("limit", "200"),
                        ("cursor", &cursor),
                    ],
                )
                .await?;
            for channel in page.channels {
                if channel.name == name && channel.creator == bot {
                    channel.validate(name, bot)?;
                    return Ok(Some(channel));
                }
            }
            let next = page.response_metadata.next_cursor;
            if next.is_empty() {
                return Ok(None);
            }
            if next == cursor {
                return Err(ApiError::Unknown(
                    "Slack repeated its channel cursor".to_owned(),
                ));
            }
            cursor = next;
        }
        Err(ApiError::Unknown(
            "channel recovery exceeds lookup page limit".to_owned(),
        ))
    }
}

impl Channel {
    pub(super) fn validate(&self, name: &str, bot: &str) -> Result<(), ApiError> {
        if !valid_id(&self.id, b"CG")
            || self.name != name
            || self.creator != bot
            || !self.is_private
            || self.is_archived
        {
            return Err(ApiError::Unknown("created channel identity or privacy does not match the requested specialist channel".to_owned()));
        }
        Ok(())
    }
}
