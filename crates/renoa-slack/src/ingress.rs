use serde::{Deserialize, Serialize};

use crate::SlackError;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct Topic {
    pub(crate) channel: String,
    pub(crate) thread: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Envelope {
    #[serde(rename = "type")]
    pub(crate) kind: String,
    #[serde(rename = "envelope_id")]
    pub(crate) id: Option<String>,
    pub(crate) payload: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct Callback {
    #[serde(rename = "type")]
    kind: String,
    team_id: String,
    event_id: String,
    api_app_id: String,
    authorizations: Vec<Authorization>,
    event: Event,
}

#[derive(Deserialize)]
struct Authorization {
    team_id: Option<String>,
    user_id: Option<String>,
    is_bot: bool,
}

#[derive(Deserialize)]
struct Event {
    #[serde(rename = "type")]
    kind: String,
    user: Option<String>,
    bot_id: Option<String>,
    subtype: Option<String>,
    channel: Option<String>,
    channel_type: Option<String>,
    ts: Option<String>,
    thread_ts: Option<String>,
    text: Option<String>,
}

#[derive(Debug)]
pub(crate) struct Incoming {
    pub(crate) event_id: String,
    pub(crate) topic: Topic,
    pub(crate) message_ts: String,
    pub(crate) text: String,
    pub(crate) starts_conversation: bool,
}

pub(crate) fn parse(
    payload: serde_json::Value,
    team: &str,
    user: &str,
    bot: &str,
) -> Result<Option<Incoming>, SlackError> {
    if payload.get("type").and_then(serde_json::Value::as_str) != Some("event_callback") {
        return Ok(None);
    }
    let callback: Callback = serde_json::from_value(payload)?;
    if callback.kind != "event_callback" || callback.team_id != team {
        return Ok(None);
    }
    if !valid_id(&callback.api_app_id, b"A")
        || !callback.authorizations.iter().any(|authorization| {
            authorization.is_bot
                && authorization.team_id.as_deref() == Some(team)
                && authorization.user_id.as_deref() == Some(bot)
        })
    {
        return Err(SlackError::Invalid(
            "Slack event installation does not match the configured bot token".to_owned(),
        ));
    }
    let event = callback.event;
    if event.user.as_deref() != Some(user)
        || event.bot_id.is_some()
        || event.subtype.is_some()
        || !matches!(event.kind.as_str(), "message" | "app_mention")
    {
        return Ok(None);
    }
    let (Some(channel), Some(ts), Some(text)) = (event.channel, event.ts, event.text) else {
        return Ok(None);
    };
    if !valid_id(&channel, b"CDG")
        || !valid_ts(&ts)
        || callback.event_id.is_empty()
        || callback.event_id.len() > 256
    {
        return Err(SlackError::Invalid(
            "invalid Slack message identity".to_owned(),
        ));
    }
    let mention = format!("<@{bot}>");
    let direct = event.channel_type.as_deref() == Some("im");
    let addressed = event.kind == "app_mention" || text.contains(&mention);
    let thread = if direct {
        String::new()
    } else {
        event.thread_ts.unwrap_or_else(|| ts.clone())
    };
    if !thread.is_empty() && !valid_ts(&thread) {
        return Err(SlackError::Invalid(
            "invalid Slack thread identity".to_owned(),
        ));
    }
    Ok(Some(Incoming {
        event_id: callback.event_id,
        topic: Topic { channel, thread },
        message_ts: ts,
        text: text
            .replace(&mention, "")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&")
            .trim()
            .to_owned(),
        starts_conversation: direct || addressed,
    }))
}

pub(crate) fn valid_id(id: &str, prefixes: &[u8]) -> bool {
    (2..=128).contains(&id.len())
        && id.as_bytes().first().is_some_and(|p| prefixes.contains(p))
        && id.bytes().all(|c| c.is_ascii_alphanumeric())
}

pub(crate) fn valid_ts(ts: &str) -> bool {
    ts.split_once('.').is_some_and(|(seconds, fraction)| {
        !seconds.is_empty()
            && seconds.len() <= 16
            && fraction.len() == 6
            && seconds
                .bytes()
                .chain(fraction.bytes())
                .all(|b| b.is_ascii_digit())
    })
}
