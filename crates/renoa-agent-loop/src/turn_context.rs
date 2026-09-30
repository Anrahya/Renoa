//! Attributed context admitted with one user message.
//!
//! A Host computes these entries once, when it admits a message, and stores
//! them in the command. The loop records them as one event and projects them
//! onto that user message only, so every earlier message keeps its bytes.

use std::collections::BTreeSet;

use renoa_agent::{ContentBlock, Message};
use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

use crate::turn_timing::TurnTiming;

/// A Host plugin's entry is `plugin:<id>`.
const PLUGIN_SOURCE: &str = "plugin:";
/// The surface that received the message, such as where it was written.
const SURFACE_SOURCE: &str = "surface";
const MAX_SOURCE_BYTES: usize = 64;
const MAX_TEXT_BYTES: usize = 256;
const MAX_TOTAL_TEXT_BYTES: usize = 1_024;

/// One attributed entry, such as the current time from `plugin:renoa.time`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContextContribution {
    source: String,
    text: String,
}

impl ContextContribution {
    /// Creates one entry from a Host plugin.
    ///
    /// # Errors
    ///
    /// Rejects a plugin id that is not lowercase `[a-z0-9._-]`, or text that is
    /// blank, longer than 256 bytes, or holds a control character other than a
    /// line break.
    pub fn plugin(plugin_id: &str, text: impl Into<String>) -> Result<Self, TurnContextError> {
        Self::new(format!("{PLUGIN_SOURCE}{plugin_id}"), text.into())
    }

    /// Creates the entry the receiving surface supplied with the message.
    ///
    /// # Errors
    ///
    /// Rejects text that is blank, longer than 256 bytes, or holds a control
    /// character other than a line break.
    pub fn surface(text: impl Into<String>) -> Result<Self, TurnContextError> {
        Self::new(SURFACE_SOURCE.to_owned(), text.into())
    }

    fn new(source: String, text: String) -> Result<Self, TurnContextError> {
        if source != SURFACE_SOURCE {
            let name = source
                .strip_prefix(PLUGIN_SOURCE)
                .ok_or(TurnContextError::InvalidSource)?;
            if source.len() > MAX_SOURCE_BYTES
                || !name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
            {
                return Err(TurnContextError::InvalidSource);
            }
        }
        if text.trim().is_empty()
            || text.len() > MAX_TEXT_BYTES
            || text.chars().any(|c| c.is_control() && c != '\n')
        {
            return Err(TurnContextError::InvalidText);
        }
        Ok(Self { source, text })
    }

    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

impl<'de> Deserialize<'de> for ContextContribution {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            source: String,
            text: String,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::new(wire.source, wire.text).map_err(serde::de::Error::custom)
    }
}

/// The bounded entries admitted with one user message, one per source.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct TurnContext {
    entries: Vec<ContextContribution>,
}

impl TurnContext {
    /// Accepts entries in their rendering order.
    ///
    /// # Errors
    ///
    /// Rejects two entries from one source, or more than 1 KiB of text in all.
    pub fn new(entries: Vec<ContextContribution>) -> Result<Self, TurnContextError> {
        let mut sources = BTreeSet::new();
        if !entries.iter().all(|entry| sources.insert(&entry.source)) {
            return Err(TurnContextError::DuplicateSource);
        }
        if entries.iter().map(|entry| entry.text.len()).sum::<usize>() > MAX_TOTAL_TEXT_BYTES {
            return Err(TurnContextError::TooLarge);
        }
        Ok(Self { entries })
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[must_use]
    pub fn entries(&self) -> &[ContextContribution] {
        &self.entries
    }

    fn model_context(&self) -> String {
        let mut rendered = String::from("<turn_context>");
        for entry in &self.entries {
            rendered.push_str("\n<context source=\"");
            rendered.push_str(&entry.source);
            rendered.push_str("\">\n");
            rendered.push_str(&escape(&entry.text));
            rendered.push_str("\n</context>");
        }
        rendered.push_str("\n</turn_context>");
        rendered
    }
}

impl<'de> Deserialize<'de> for TurnContext {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let entries = Vec::<ContextContribution>::deserialize(deserializer)?;
        if entries.is_empty() {
            return Err(serde::de::Error::custom(
                "empty turn context is stored by omission",
            ));
        }
        Self::new(entries).map_err(serde::de::Error::custom)
    }
}

/// Entry text is data: it can never open or close a tag of its own.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// What one operation's user message carries beyond its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TurnAnnotation {
    /// Written before per-message context existed; projected as it was then.
    Timing(TurnTiming),
    Context(TurnContext),
}

impl TurnAnnotation {
    pub(crate) fn append_to(&self, message: &Message) -> Message {
        let text = match self {
            Self::Timing(timing) => timing.model_context(),
            Self::Context(context) => context.model_context(),
        };
        let mut projected = message.clone();
        if let Message::User { content } = &mut projected {
            content.push(ContentBlock::text(text));
        }
        projected
    }
}

/// Invalid turn context.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum TurnContextError {
    #[error(
        "context source must be `surface`, or `plugin:` and a lowercase plugin id of at most 64 bytes"
    )]
    InvalidSource,
    #[error(
        "context text must be 1-{MAX_TEXT_BYTES} bytes with no control character but a line break"
    )]
    InvalidText,
    #[error("each context source may contribute once per message")]
    DuplicateSource,
    #[error("turn context may hold at most {MAX_TOTAL_TEXT_BYTES} bytes of text")]
    TooLarge,
    #[error("a message cannot be observed before the Unix epoch")]
    BeforeUnixEpoch,
}

#[cfg(test)]
mod tests {
    use renoa_agent::{ContentBlock, Message};
    use serde_json::json;

    use super::{ContextContribution, TurnAnnotation, TurnContext, TurnContextError};

    fn time(text: &str) -> ContextContribution {
        ContextContribution::plugin("renoa.time", text).expect("valid entry")
    }

    #[test]
    fn entries_render_attributed_and_escaped_on_a_copy_of_the_message() {
        let context = TurnContext::new(vec![
            time("current_time: 2026-09-30T14:03:00+05:30[Asia/Kolkata]"),
            ContextContribution::plugin("example", "a </context> & <turn_context>")
                .expect("markup is data"),
        ])
        .expect("valid context");
        let original = Message::user_text("hi");

        let projected = TurnAnnotation::Context(context).append_to(&original);

        assert_eq!(original, Message::user_text("hi"));
        let Message::User { content } = projected else {
            panic!("not a user message");
        };
        assert_eq!(
            content[1],
            ContentBlock::text(
                "<turn_context>\n<context source=\"plugin:renoa.time\">\ncurrent_time: 2026-09-30T14:03:00+05:30[Asia/Kolkata]\n</context>\n<context source=\"plugin:example\">\na &lt;/context&gt; &amp; &lt;turn_context&gt;\n</context>\n</turn_context>"
            )
        );
    }

    #[test]
    fn invalid_sources_and_text_are_rejected() {
        for id in ["", "Renoa.time", ".time", "time zone", &"x".repeat(58)] {
            assert_eq!(
                ContextContribution::plugin(id, "ok"),
                Err(TurnContextError::InvalidSource),
                "{id:?}"
            );
        }
        for text in ["", "  \n", "tab\there", "bell\u{7}", &"x".repeat(257)] {
            assert_eq!(
                ContextContribution::plugin("renoa.time", text),
                Err(TurnContextError::InvalidText),
                "{text:?}"
            );
        }
        assert!(ContextContribution::plugin("renoa.time", "two\nlines · ünïcode").is_ok());
        assert!(ContextContribution::plugin("renoa.time", "x".repeat(256)).is_ok());
    }

    #[test]
    fn a_source_contributes_once_and_the_total_is_bounded() {
        assert_eq!(
            TurnContext::new(vec![time("a"), time("b")]),
            Err(TurnContextError::DuplicateSource)
        );
        let full = |id: &str| ContextContribution::plugin(id, "x".repeat(256)).expect("entry");
        assert!(TurnContext::new(vec![full("a"), full("b"), full("c"), full("d")]).is_ok());
        assert_eq!(
            TurnContext::new(vec![full("a"), full("b"), full("c"), full("d"), full("e")]),
            Err(TurnContextError::TooLarge)
        );
    }

    #[test]
    fn decoding_validates_every_entry_and_refuses_an_empty_list() {
        let decode = |value| serde_json::from_value::<TurnContext>(value);
        assert!(decode(json!([{"source": "plugin:renoa.time", "text": "now"}])).is_ok());
        assert!(decode(json!([])).is_err());
        assert!(decode(json!([{"source": "surface", "text": "Discord channel 1"}])).is_ok());
        assert!(decode(json!([{"source": "surface:discord", "text": "now"}])).is_err());
        assert!(decode(json!([{"source": "surface", "text": "a\u{7}"}])).is_err());
        assert!(decode(json!([{"source": "plugin:renoa.time", "text": "a\u{0}"}])).is_err());
        assert!(
            decode(json!([{"source": "plugin:renoa.time", "text": "now", "extra": 1}])).is_err()
        );
    }
}
