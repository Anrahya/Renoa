//! What `ask_owner` and `notify_owner` accept. Every refusal names the field,
//! what was sent and what is expected, and shows a correct call.

use renoa_agent::ToolError;
use serde_json::{Map, Value};

use super::{ASK, NOTIFY};
use crate::host::feed::{BODY_CHARS, CHOICE_CHARS, CHOICES, Urgency};

const ASK_EXAMPLE: &str =
    r#"{"question": "Book the 6pm table?", "choices": ["Yes", "No"], "urgency": "soon"}"#;
const NOTIFY_EXAMPLE: &str = r#"{"message": "The build failed twice.", "urgency": "soon"}"#;
const OWN_WORDS: &str = r#"Leave "choices" out when the owner should answer in their own words."#;

/// One checked call.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Input {
    Ask {
        question: String,
        choices: Option<Vec<String>>,
        urgency: Urgency,
    },
    Notify {
        message: String,
        urgency: Urgency,
    },
}

#[derive(Clone, Copy)]
pub(super) enum Kind {
    Ask,
    Notify,
}

impl Kind {
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::Ask => ASK,
            Self::Notify => NOTIFY,
        }
    }

    const fn fields(self) -> &'static [&'static str] {
        match self {
            Self::Ask => &["question", "choices", "urgency"],
            Self::Notify => &["message", "urgency"],
        }
    }

    const fn example(self) -> &'static str {
        match self {
            Self::Ask => ASK_EXAMPLE,
            Self::Notify => NOTIFY_EXAMPLE,
        }
    }

    /// The field a model most likely meant by `field`.
    fn meant(self, field: &str) -> Option<&'static str> {
        let text = match self {
            Self::Ask => "question",
            Self::Notify => "message",
        };
        match (self, field) {
            (Self::Ask, "options" | "answers" | "buttons" | "replies" | "choice") => {
                Some("choices")
            }
            (_, "priority" | "importance" | "level" | "urgent") => Some("urgency"),
            (_, "text" | "body" | "content" | "prompt" | "note" | "question" | "message") => {
                Some(text)
            }
            _ => None,
        }
    }
}

pub(super) fn parse(kind: Kind, arguments: &Value) -> Result<Input, ToolError> {
    let name = kind.name();
    let Some(fields) = arguments.as_object() else {
        return Err(invalid(format!(
            "{name} takes one object. Example: {}",
            kind.example()
        )));
    };
    if let Some(unknown) = fields
        .keys()
        .find(|field| !kind.fields().contains(&field.as_str()))
    {
        let suggestion = kind
            .meant(unknown)
            .map(|field| format!(" Did you mean \"{field}\"?"))
            .unwrap_or_default();
        return Err(invalid(format!(
            "{name} has no field \"{unknown}\". Its fields are {}.{suggestion}",
            kind.fields().join(", ")
        )));
    }
    match kind {
        Kind::Ask => Ok(Input::Ask {
            question: text(kind, fields, "question")?,
            choices: choices(fields)?,
            urgency: urgency(kind, fields)?,
        }),
        Kind::Notify => Ok(Input::Notify {
            message: text(kind, fields, "message")?,
            urgency: urgency(kind, fields)?,
        }),
    }
}

fn text(kind: Kind, fields: &Map<String, Value>, field: &str) -> Result<String, ToolError> {
    let example = kind.example();
    let what = match kind {
        Kind::Ask => "the one decision you need from the owner",
        Kind::Notify => "what the owner should know",
    };
    let text = match fields.get(field) {
        None | Some(Value::Null) => {
            return Err(invalid(format!(
                "{} needs \"{field}\": {what}, at most {BODY_CHARS} characters. Example: {example}",
                kind.name()
            )));
        }
        Some(Value::String(text)) => text.trim(),
        Some(other) => {
            return Err(invalid(format!(
                "\"{field}\" must be text; you sent {other}. Example: {example}"
            )));
        }
    };
    if text.is_empty() {
        return Err(invalid(format!(
            "\"{field}\" is empty. Write {what}. Example: {example}"
        )));
    }
    let length = text.chars().count();
    if length > BODY_CHARS {
        let advice = match kind {
            Kind::Ask => {
                "Shorten it to the one decision you need, and write any background in the message before this call."
            }
            Kind::Notify => "Keep it to the one thing the owner needs to know.",
        };
        return Err(invalid(format!(
            "\"{field}\" is {length} characters; the limit is {BODY_CHARS}. {advice}"
        )));
    }
    Ok(text.to_owned())
}

fn choices(fields: &Map<String, Value>) -> Result<Option<Vec<String>>, ToolError> {
    let (least, most) = (CHOICES.start(), CHOICES.end());
    let list = match fields.get("choices") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Array(list)) => list,
        Some(other) => {
            return Err(invalid(format!(
                "\"choices\" must be a list of {least} to {most} short answers, such as [\"Yes\", \"No\"]; you sent {other}. {OWN_WORDS}"
            )));
        }
    };
    if !CHOICES.contains(&list.len()) {
        return Err(invalid(format!(
            "\"choices\" needs {least} to {most} options; you sent {}. {OWN_WORDS}",
            list.len()
        )));
    }
    let mut checked: Vec<String> = Vec::with_capacity(list.len());
    for (index, choice) in list.iter().enumerate() {
        let position = index + 1;
        let choice = match choice {
            Value::String(choice) if !choice.trim().is_empty() => choice.trim(),
            other => {
                return Err(invalid(format!(
                    "choice {position} must be text of 1 to {CHOICE_CHARS} characters; you sent {other}."
                )));
            }
        };
        let length = choice.chars().count();
        if length > CHOICE_CHARS {
            return Err(invalid(format!(
                "choice {position} is {length} characters; the limit is {CHOICE_CHARS}. Shorten it to the answer itself."
            )));
        }
        if checked.iter().any(|earlier| earlier == choice) {
            return Err(invalid(format!(
                "choice {position} repeats \"{choice}\". Give each option once."
            )));
        }
        checked.push(choice.to_owned());
    }
    Ok(Some(checked))
}

fn urgency(kind: Kind, fields: &Map<String, Value>) -> Result<Urgency, ToolError> {
    let example = kind.example();
    match fields.get("urgency") {
        None | Some(Value::Null) => Err(invalid(format!(
            "{} needs \"urgency\": \"whenever\", \"soon\" or \"now\". You sent none. Use \"now\" only if waiting costs the owner something within minutes. Example: {example}",
            kind.name()
        ))),
        Some(sent) => Urgency::ALL
            .into_iter()
            .find(|urgency| sent.as_str() == Some(urgency.as_str()))
            .ok_or_else(|| {
                invalid(format!(
                    "\"urgency\" must be \"whenever\", \"soon\" or \"now\"; you sent {sent}. Example: {example}"
                ))
            }),
    }
}

fn invalid(message: String) -> ToolError {
    ToolError::invalid_input(message)
}
