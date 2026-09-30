//! Where a Discord message was written.
//!
//! The surface keeps a directory of the connected server's channels and
//! threads from gateway events. A thread records the channel it belongs to, so
//! it answers as that channel's agent. The directory also names the place for
//! the agent: each message carries a short description as its surface context.

use serde::Deserialize;
use serde_json::Value;

use crate::{DiscordError, api::Channel, snowflake::Snowflake};

/// Longest channel or thread name, in bytes, quoted in a description. Two
/// names, three ids and the labels stay within the 256-byte context entry.
const MAX_NAME_BYTES: usize = 60;

/// One channel or thread as the directory keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Place {
    pub(crate) channel_id: String,
    pub(crate) name: Option<String>,
    /// The channel a thread belongs to.
    pub(crate) thread_parent_id: Option<String>,
}

impl Place {
    pub(crate) fn of(channel: &Channel) -> Self {
        Self {
            channel_id: channel.id.as_str().to_owned(),
            name: channel.name().map(str::to_owned),
            thread_parent_id: channel
                .is_thread()
                .then(|| channel.parent_id.as_ref().map(|id| id.as_str().to_owned()))
                .flatten(),
        }
    }
}

/// What one gateway dispatch changes in the directory.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Changes {
    pub(crate) known: Vec<Place>,
    pub(crate) gone: Vec<String>,
}

/// The directory changes in a gateway dispatch about the connected server.
/// Returns `None` for any other dispatch, including another server's.
///
/// # Errors
///
/// Rejects a channel event whose payload is not a Discord channel.
pub(crate) fn changes(
    kind: &str,
    data: &Value,
    guild_id: &str,
) -> Result<Option<Changes>, DiscordError> {
    #[derive(Deserialize)]
    struct Guild {
        id: String,
        #[serde(default)]
        channels: Vec<Channel>,
        #[serde(default)]
        threads: Vec<Channel>,
    }
    #[derive(Deserialize)]
    struct ThreadList {
        guild_id: String,
        #[serde(default)]
        threads: Vec<Channel>,
    }
    let known = |channels: Vec<Channel>| Changes {
        known: channels.iter().map(Place::of).collect(),
        gone: Vec::new(),
    };
    let ours =
        |channel: &Channel| channel.guild_id.as_ref().map(Snowflake::as_str) == Some(guild_id);
    Ok(match kind {
        "GUILD_CREATE" => {
            let guild = Guild::deserialize(data)?;
            (guild.id == guild_id)
                .then(|| known(guild.channels.into_iter().chain(guild.threads).collect()))
        }
        "THREAD_LIST_SYNC" => {
            let list = ThreadList::deserialize(data)?;
            (list.guild_id == guild_id).then(|| known(list.threads))
        }
        "CHANNEL_CREATE" | "CHANNEL_UPDATE" | "THREAD_CREATE" | "THREAD_UPDATE" => {
            let channel = Channel::deserialize(data)?;
            ours(&channel).then(|| known(vec![channel]))
        }
        "CHANNEL_DELETE" | "THREAD_DELETE" => {
            let channel = Channel::deserialize(data)?;
            ours(&channel).then(|| Changes {
                known: Vec::new(),
                gone: vec![channel.id.as_str().to_owned()],
            })
        }
        _ => None,
    })
}

/// The surface context for a message: the server, the channel, and the thread
/// when there is one. A direct message has no server. Names the directory
/// does not know are left out; ids are always present.
pub(crate) fn describe(guild_id: Option<&str>, place: &Place, parent: Option<&Place>) -> String {
    let Some(guild_id) = guild_id else {
        return format!("Discord direct message (channel {})", place.channel_id);
    };
    let channel = |place: &Place| match place.name.as_deref() {
        Some(name) => format!("channel #{} ({})", label(name), place.channel_id),
        None => format!("channel {}", place.channel_id),
    };
    match &place.thread_parent_id {
        None => format!("Discord server {guild_id}\n{}", channel(place)),
        Some(parent_id) => {
            let parent = parent.cloned().unwrap_or_else(|| Place {
                channel_id: parent_id.clone(),
                name: None,
                thread_parent_id: None,
            });
            let thread = match place.name.as_deref() {
                Some(name) => format!("thread \"{}\" ({})", label(name), place.channel_id),
                None => format!("thread {}", place.channel_id),
            };
            format!("Discord server {guild_id}\n{}\n{thread}", channel(&parent))
        }
    }
}

/// A name as a description quotes it: one line, at most [`MAX_NAME_BYTES`].
fn label(name: &str) -> String {
    let line: String = name
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if line.len() <= MAX_NAME_BYTES {
        return line;
    }
    let mut end = MAX_NAME_BYTES;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &line[..end])
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Changes, Place, changes, describe};

    fn place(id: &str, name: Option<&str>, parent: Option<&str>) -> Place {
        Place {
            channel_id: id.to_owned(),
            name: name.map(str::to_owned),
            thread_parent_id: parent.map(str::to_owned),
        }
    }

    #[test]
    fn only_threads_record_a_parent_and_only_this_server_counts() {
        let guild = json!({
            "id": "10",
            "channels": [
                {"id": "200", "type": 4, "name": "Work"},
                {"id": "202", "type": 0, "name": "desk", "parent_id": "200"}
            ],
            "threads": [{"id": "303", "type": 11, "name": "plan", "parent_id": "202"}]
        });
        assert_eq!(
            changes("GUILD_CREATE", &guild, "10").expect("guild"),
            Some(Changes {
                known: vec![
                    place("200", Some("Work"), None),
                    place("202", Some("desk"), None),
                    place("303", Some("plan"), Some("202")),
                ],
                gone: Vec::new(),
            })
        );
        assert_eq!(changes("GUILD_CREATE", &guild, "11").expect("other"), None);
        let thread = json!({"id": "304", "guild_id": "10", "type": 12, "name": "private", "parent_id": "202"});
        assert_eq!(
            changes("THREAD_CREATE", &thread, "10")
                .expect("thread")
                .expect("ours")
                .known,
            vec![place("304", Some("private"), Some("202"))]
        );
        assert_eq!(
            changes("THREAD_DELETE", &thread, "10")
                .expect("deleted")
                .expect("ours")
                .gone,
            vec!["304".to_owned()]
        );
        assert_eq!(
            changes("THREAD_UPDATE", &thread, "11").expect("other"),
            None
        );
        assert_eq!(
            changes("MESSAGE_CREATE", &thread, "10").expect("not ours"),
            None
        );
        assert!(changes("CHANNEL_UPDATE", &json!({"id": "x", "type": 0}), "10").is_err());
    }

    #[test]
    fn a_description_names_the_server_channel_and_thread() {
        let desk = place("202", Some("desk"), None);
        assert_eq!(
            describe(Some("10"), &desk, None),
            "Discord server 10\nchannel #desk (202)"
        );
        assert_eq!(
            describe(
                Some("10"),
                &place("303", Some("plan"), Some("202")),
                Some(&desk)
            ),
            "Discord server 10\nchannel #desk (202)\nthread \"plan\" (303)"
        );
        assert_eq!(
            describe(Some("10"), &place("303", None, Some("202")), None),
            "Discord server 10\nchannel 202\nthread 303"
        );
        assert_eq!(
            describe(None, &place("404", None, None), None),
            "Discord direct message (channel 404)"
        );
    }

    #[test]
    fn the_longest_description_fits_one_context_entry() {
        let id = "18446744073709551615";
        let long = "é\u{7}".repeat(80);
        let parent = place(id, Some(&long), None);
        let text = describe(Some(id), &place(id, Some(&long), Some(id)), Some(&parent));
        assert!(text.len() <= 256, "{} bytes", text.len());
        assert!(!text.chars().any(|c| c.is_control() && c != '\n'));
        renoa_agent_loop::ContextContribution::surface(text).expect("a valid surface entry");
    }
}
