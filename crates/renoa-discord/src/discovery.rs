//! Discord reads that let the owner connect a bot without editing files.

use std::collections::HashMap;

use serde::{Deserialize, Deserializer, Serialize};

use crate::{DiscordError, api::DiscordApi, snowflake::Snowflake};

const ADMINISTRATOR: u64 = 1 << 3;
/// `GATEWAY_MESSAGE_CONTENT` or `GATEWAY_MESSAGE_CONTENT_LIMITED`.
const MESSAGE_CONTENT: u64 = (1 << 18) | (1 << 19);

#[derive(Deserialize)]
pub(crate) struct User {
    id: Snowflake,
    username: String,
}

#[derive(Deserialize)]
pub(crate) struct Application {
    id: Snowflake,
    owner: Option<User>,
    team: Option<Team>,
    flags: Option<u64>,
    /// Discord's string form of `flags`, present once flags exceed 31 bits.
    flags_new: Option<String>,
}

#[derive(Deserialize)]
struct Team {
    owner_user_id: Snowflake,
}

/// A server the bot belongs to, and whether its roles grant Administrator.
#[derive(Deserialize, Serialize)]
pub struct DiscordGuild {
    pub(crate) id: Snowflake,
    pub(crate) name: String,
    #[serde(
        rename(deserialize = "permissions"),
        deserialize_with = "administrator"
    )]
    pub(crate) administrator: bool,
}

/// What a pasted bot token can reach, before anything is saved.
#[derive(Serialize)]
pub struct DiscordInspection {
    bot_name: String,
    invite_url: String,
    guilds: Vec<DiscordGuild>,
}

/// A text or announcement channel, in Discord's sidebar order.
#[derive(Serialize)]
pub struct DiscordChannel {
    id: Snowflake,
    name: String,
}

/// The bot account and who administers its application.
pub(crate) struct Identity {
    pub(crate) bot_name: String,
    pub(crate) operator_user_id: Snowflake,
    application_id: Snowflake,
}

/// Confirms the token is a bot whose application may read message content.
pub(crate) async fn identify(api: &DiscordApi) -> Result<Identity, DiscordError> {
    let application = api.application().await?;
    let bot = api.user().await?;
    let flags = match application.flags_new {
        Some(flags) => flags.parse::<u64>().map_err(|_| {
            DiscordError::Api("Discord returned unreadable application flags".into())
        })?,
        None => application.flags.unwrap_or_default(),
    };
    if flags & MESSAGE_CONTENT == 0 {
        return Err(DiscordError::Invalid(
            "Turn on Message Content Intent on the application's Bot page, then retry".into(),
        ));
    }
    let operator_user_id = match (application.team, application.owner) {
        (Some(team), _) => team.owner_user_id,
        (None, Some(owner)) => owner.id,
        (None, None) => {
            return Err(DiscordError::Api(
                "Discord did not identify the application owner".into(),
            ));
        }
    };
    Ok(Identity {
        bot_name: bot.username,
        operator_user_id,
        application_id: application.id,
    })
}

pub(crate) async fn inspect(api: &DiscordApi) -> Result<DiscordInspection, DiscordError> {
    let identity = identify(api).await?;
    Ok(DiscordInspection {
        invite_url: format!(
            "https://discord.com/oauth2/authorize?client_id={}&scope=bot&permissions={ADMINISTRATOR}",
            identity.application_id.as_str()
        ),
        bot_name: identity.bot_name,
        guilds: api.guilds().await?,
    })
}

pub(crate) async fn channels(
    api: &DiscordApi,
    guild: &Snowflake,
) -> Result<Vec<DiscordChannel>, DiscordError> {
    let channels = api.channels(guild.as_str()).await?;
    let categories: HashMap<&Snowflake, i64> = channels
        .iter()
        .filter(|channel| channel.is_category())
        .map(|channel| (&channel.id, channel.position))
        .collect();
    let mut listed: Vec<_> = channels
        .iter()
        .filter(|channel| channel.is_text())
        .map(|channel| {
            let category = channel
                .parent_id
                .as_ref()
                .and_then(|parent| Some((*categories.get(parent)?, parent)));
            ((category, channel.position, &channel.id), channel)
        })
        .collect();
    // Uncategorized channels lead, then each category in its own order.
    listed.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    Ok(listed
        .into_iter()
        .map(|(_, channel)| DiscordChannel {
            id: channel.id.clone(),
            name: channel.display_name().to_owned(),
        })
        .collect())
}

fn administrator<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    let bits = String::deserialize(deserializer)?
        .parse::<u64>()
        .map_err(serde::de::Error::custom)?;
    Ok(bits & ADMINISTRATOR != 0)
}
