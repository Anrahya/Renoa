use serde::Deserialize;

use crate::{DiscordError, snowflake::Snowflake};

/// A Discord message the surface will turn into one agent request.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Addressed {
    pub(crate) message_id: Snowflake,
    pub(crate) channel_id: Snowflake,
    pub(crate) author_id: Snowflake,
    pub(crate) canonical: Vec<u8>,
    pub(crate) prompt: String,
}

/// Decides whether one `MESSAGE_CREATE` payload is a turn.
///
/// Guild members address the bot by mentioning it. A direct message is a turn
/// only for the bound operator. Discord may add fields; those are ignored
/// because the payload is an external event stream.
///
/// # Errors
///
/// Returns an error when an id that this surface would store is not a snowflake.
pub(crate) fn addressed(
    payload: &[u8],
    bot_user_id: &Snowflake,
    guild_id: &Snowflake,
    operator_user_id: &Snowflake,
) -> Result<Option<Addressed>, DiscordError> {
    let message: MessageCreate = serde_json::from_slice(payload)?;
    if message.author.bot == Some(true) || message.author.id == bot_user_id.as_str() {
        return Ok(None);
    }
    let mentioned = message
        .mentions
        .iter()
        .any(|mention| mention.id == bot_user_id.as_str());
    let direct = message.guild_id.is_none();
    let in_guild = message.guild_id.as_deref() == Some(guild_id.as_str());
    let speak =
        (direct && message.author.id == operator_user_id.as_str()) || (in_guild && mentioned);
    if !speak {
        return Ok(None);
    }
    let prompt = if direct {
        message.content.trim().to_owned()
    } else {
        strip_mention(&message.content, bot_user_id.as_str())
    };
    Ok(Some(Addressed {
        message_id: Snowflake::parse(&message.id)?,
        channel_id: Snowflake::parse(&message.channel_id)?,
        author_id: Snowflake::parse(&message.author.id)?,
        canonical: payload.to_vec(),
        prompt,
    }))
}

fn strip_mention(content: &str, bot_user_id: &str) -> String {
    let without = content
        .replace(&format!("<@{bot_user_id}>"), " ")
        .replace(&format!("<@!{bot_user_id}>"), " ");
    without.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Deserialize)]
struct MessageCreate {
    id: String,
    channel_id: String,
    guild_id: Option<String>,
    #[serde(default)]
    content: String,
    author: Author,
    #[serde(default)]
    mentions: Vec<Mention>,
}

#[derive(Deserialize)]
struct Author {
    id: String,
    bot: Option<bool>,
}

#[derive(Deserialize)]
struct Mention {
    id: String,
}

/// Splits a Discord reply into 2000-character pages without cutting a scalar.
#[must_use]
pub(crate) fn pages(text: &str) -> Vec<String> {
    const LIMIT: usize = 2000;
    let text = if text.is_empty() {
        "(The agent returned an empty reply.)"
    } else {
        text
    };
    let mut pages = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let count = rest.chars().count();
        if count <= LIMIT {
            pages.push(rest.to_owned());
            break;
        }
        let mut end = rest.chars().take(LIMIT).map(char::len_utf8).sum::<usize>();
        if let Some(split) = rest[..end].rfind('\n')
            && split > 0
        {
            end = split + 1;
        }
        let (page, next) = rest.split_at(end);
        pages.push(page.to_owned());
        rest = next;
    }
    pages
}

#[cfg(test)]
mod tests {
    use super::{addressed, pages};
    use crate::snowflake::Snowflake;

    fn snowflake(value: &str) -> Snowflake {
        Snowflake::parse(value).expect("snowflake")
    }

    #[test]
    fn a_guild_mention_is_a_turn_and_other_chatter_is_not() {
        let bot = snowflake("50");
        let guild = snowflake("10");
        let operator = snowflake("20");
        let mentioned = addressed(
            br#"{"id":"101","channel_id":"202","guild_id":"10","content":"<@50> hello","author":{"id":"99"},"mentions":[{"id":"50"}]}"#,
            &bot,
            &guild,
            &operator,
        )
        .expect("mention");
        let mentioned = mentioned.expect("addressed");
        assert_eq!(mentioned.prompt, "hello");
        assert_eq!(mentioned.author_id.as_str(), "99");

        let chatter = addressed(
            br#"{"id":"102","channel_id":"202","guild_id":"10","content":"hello","author":{"id":"99"},"mentions":[]}"#,
            &bot,
            &guild,
            &operator,
        )
        .expect("chatter");
        assert!(chatter.is_none());
    }

    #[test]
    fn only_the_operator_direct_message_is_a_turn() {
        let bot = snowflake("50");
        let guild = snowflake("10");
        let operator = snowflake("20");
        let owned = addressed(
            br#"{"id":"101","channel_id":"303","content":"hello","author":{"id":"20"}}"#,
            &bot,
            &guild,
            &operator,
        )
        .expect("dm");
        assert_eq!(owned.expect("operator dm").prompt, "hello");
        let stranger = addressed(
            br#"{"id":"102","channel_id":"303","content":"hello","author":{"id":"99"}}"#,
            &bot,
            &guild,
            &operator,
        )
        .expect("stranger");
        assert!(stranger.is_none());
    }

    #[test]
    fn pages_keep_a_multibyte_character_intact() {
        let text = format!("{}{}", "a".repeat(2000), "🙂");
        let split = pages(&text);
        assert_eq!(split.len(), 2);
        assert_eq!(split[0].chars().count(), 2000);
        assert_eq!(split[1], "🙂");
    }
}
