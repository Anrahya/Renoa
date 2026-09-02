use crate::api::{Message, ReceivedUpdate, Update};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Topic {
    pub(crate) chat_id: i64,
    pub(crate) thread_id: Option<i64>,
}

impl Topic {
    pub(crate) fn stored_thread_id(self) -> i64 {
        self.thread_id.unwrap_or(0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum InboundKind {
    Prompt(String),
    Compact,
    New,
    Status,
    Model(Option<String>),
    Reasoning(Option<String>),
    Cancel,
    Notice(&'static str),
    Stopped { draft_id: i64 },
    Ignored(&'static str),
}

impl InboundKind {
    pub(crate) const fn storage_name(&self) -> &'static str {
        match self {
            Self::Prompt(_) => "prompt",
            Self::Compact => "compact",
            Self::New => "new",
            Self::Status => "status",
            Self::Model(_) => "model",
            Self::Reasoning(_) => "reasoning",
            Self::Cancel => "cancel",
            Self::Notice(_) => "notice",
            Self::Stopped { .. } => "stopped",
            Self::Ignored(_) => "ignored",
        }
    }

    pub(crate) const fn is_queued(&self) -> bool {
        !matches!(self, Self::Stopped { .. } | Self::Ignored(_))
    }
}

pub(crate) struct ParsedUpdate {
    pub(crate) update_id: i64,
    pub(crate) canonical: Vec<u8>,
    pub(crate) topic: Option<Topic>,
    pub(crate) message_id: Option<i64>,
    pub(crate) kind: InboundKind,
}

pub(crate) fn parse(
    received: ReceivedUpdate,
    allowed_user_id: i64,
    bot_username: Option<&str>,
) -> ParsedUpdate {
    let ReceivedUpdate { canonical, update } = received;
    let update_id = update.id;
    let (topic, message_id, kind) = parse_update(update, allowed_user_id, bot_username);
    ParsedUpdate {
        update_id,
        canonical,
        topic,
        message_id,
        kind,
    }
}

fn parse_update(
    update: Update,
    allowed_user_id: i64,
    bot_username: Option<&str>,
) -> (Option<Topic>, Option<i64>, InboundKind) {
    if let Some(message) = update.message {
        return parse_message(message, allowed_user_id, bot_username);
    }
    if let Some(stopped) = update.stopped_message_generation {
        let topic = valid_topic(
            stopped.chat.id,
            &stopped.chat.kind,
            stopped.message_thread_id,
            allowed_user_id,
        );
        return match topic {
            Some(topic) if stopped.draft_id != 0 => (
                Some(topic),
                None,
                InboundKind::Stopped {
                    draft_id: stopped.draft_id,
                },
            ),
            _ => (None, None, InboundKind::Ignored("unauthorized stop")),
        };
    }
    (None, None, InboundKind::Ignored("unsupported update"))
}

fn parse_message(
    message: Message,
    allowed_user_id: i64,
    bot_username: Option<&str>,
) -> (Option<Topic>, Option<i64>, InboundKind) {
    let topic = valid_topic(
        message.chat.id,
        &message.chat.kind,
        message.thread_id,
        allowed_user_id,
    );
    let authorized = topic.is_some()
        && message
            .sender
            .as_ref()
            .is_some_and(|sender| sender.id == allowed_user_id && !sender.is_bot);
    if !authorized {
        return (
            None,
            Some(message.id),
            InboundKind::Ignored("unauthorized message"),
        );
    }
    let kind = message.text.map_or(
        InboundKind::Notice("Arcee's first Telegram slice accepts text messages only."),
        |text| command_or_prompt(text, bot_username),
    );
    (topic, Some(message.id), kind)
}

fn valid_topic(
    chat_id: i64,
    chat_kind: &str,
    thread_id: Option<i64>,
    allowed_user_id: i64,
) -> Option<Topic> {
    (chat_kind == "private"
        && chat_id == allowed_user_id
        && thread_id.is_none_or(|value| value > 0))
    .then_some(Topic { chat_id, thread_id })
}

fn command_or_prompt(text: String, bot_username: Option<&str>) -> InboundKind {
    let Some(first) = text.split_whitespace().next() else {
        return InboundKind::Notice("Send Arcee a task in text.");
    };
    let Some(command) = recognized_command(first, bot_username) else {
        return InboundKind::Prompt(text);
    };
    let mut arguments = text.split_whitespace().skip(1);
    let argument = arguments.next().map(str::to_owned);
    if arguments.next().is_some() {
        return InboundKind::Notice("Arcee commands accept at most one argument.");
    }
    match command {
        "model" => InboundKind::Model(argument),
        "reasoning" => InboundKind::Reasoning(argument),
        "new" if argument.is_none() => InboundKind::New,
        "compact" if argument.is_none() => InboundKind::Compact,
        "status" if argument.is_none() => InboundKind::Status,
        "cancel" if argument.is_none() => InboundKind::Cancel,
        "start" | "help" if argument.is_none() => InboundKind::Notice(
            "Arcee is ready. Send a task, or use /new, /compact, /status, /model, /reasoning, or /cancel.",
        ),
        _ => InboundKind::Notice("This command does not accept an argument."),
    }
}

fn recognized_command<'a>(first: &'a str, bot_username: Option<&str>) -> Option<&'a str> {
    let command = first.strip_prefix('/')?;
    let (name, addressed) = command.split_once('@').unwrap_or((command, ""));
    if !addressed.is_empty()
        && !bot_username.is_some_and(|username| addressed.eq_ignore_ascii_case(username))
    {
        return None;
    }
    matches!(
        name,
        "new" | "compact" | "status" | "model" | "reasoning" | "cancel" | "start" | "help"
    )
    .then_some(name)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{InboundKind, parse};
    use crate::api::{ReceivedUpdate, Update};

    fn received(value: serde_json::Value) -> ReceivedUpdate {
        ReceivedUpdate {
            canonical: serde_json::to_vec(&value).expect("encode fixture"),
            update: serde_json::from_value::<Update>(value).expect("parse fixture"),
        }
    }

    #[test]
    fn only_the_allowlisted_private_user_can_submit_work() {
        let authorized = parse(
            received(json!({
                "update_id": 1,
                "message": {
                    "message_id": 2,
                    "from": {"id": 42, "is_bot": false},
                    "chat": {"id": 42, "type": "private"},
                    "text": "deploy the site"
                }
            })),
            42,
            Some("rc_bot"),
        );
        assert_eq!(
            authorized.kind,
            InboundKind::Prompt("deploy the site".to_owned())
        );
        assert_eq!(authorized.topic.expect("authorized topic").chat_id, 42);

        for value in [
            json!({"update_id": 2, "message": {"message_id": 3, "from": {"id": 7, "is_bot": false}, "chat": {"id": 7, "type": "private"}, "text": "hi"}}),
            json!({"update_id": 3, "message": {"message_id": 4, "from": {"id": 42, "is_bot": false}, "chat": {"id": -9, "type": "group"}, "text": "hi"}}),
        ] {
            assert!(matches!(
                parse(received(value), 42, Some("rc_bot")).kind,
                InboundKind::Ignored(_)
            ));
        }
    }

    #[test]
    fn commands_are_exact_and_bot_addressing_is_respected() {
        let fixture = |text: &str| {
            received(json!({
                "update_id": 10,
                "message": {
                    "message_id": 11,
                    "from": {"id": 42, "is_bot": false},
                    "chat": {"id": 42, "type": "private"},
                    "text": text
                }
            }))
        };
        assert_eq!(
            parse(fixture("/compact@RC_BOT"), 42, Some("rc_bot")).kind,
            InboundKind::Compact
        );
        assert!(matches!(
            parse(fixture("/compact now"), 42, Some("rc_bot")).kind,
            InboundKind::Notice(_)
        ));
        assert_eq!(
            parse(fixture("/model@RC_BOT glm-5.3-flash"), 42, Some("rc_bot")).kind,
            InboundKind::Model(Some("glm-5.3-flash".to_owned()))
        );
        assert_eq!(
            parse(fixture("/model"), 42, Some("rc_bot")).kind,
            InboundKind::Model(None)
        );
        assert_eq!(
            parse(fixture("/reasoning high"), 42, Some("rc_bot")).kind,
            InboundKind::Reasoning(Some("high".to_owned()))
        );
        assert!(matches!(
            parse(fixture("/model one two"), 42, Some("rc_bot")).kind,
            InboundKind::Notice(_)
        ));
        assert_eq!(
            parse(fixture("/compact@other"), 42, Some("rc_bot")).kind,
            InboundKind::Prompt("/compact@other".to_owned())
        );
    }

    #[test]
    fn native_stop_keeps_the_exact_draft_identity() {
        let parsed = parse(
            received(json!({
                "update_id": 20,
                "stopped_message_generation": {
                    "chat": {"id": 42, "type": "private"},
                    "message_thread_id": 8,
                    "draft_id": 99
                }
            })),
            42,
            None,
        );
        assert_eq!(parsed.kind, InboundKind::Stopped { draft_id: 99 });
        assert_eq!(parsed.topic.expect("stop topic").thread_id, Some(8));
    }
}
