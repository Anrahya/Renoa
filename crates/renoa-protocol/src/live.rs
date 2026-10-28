use serde::{Deserialize, Serialize};

/// Ephemeral progress of a running command.
///
/// Live activity is never stored, replayed or acknowledged. A surface that
/// misses a frame loses nothing durable: the journal's `assistant_message`
/// record supersedes whatever live text it showed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LiveActivity {
    /// The next piece of the assistant message being written. `sequence`
    /// counts the message's deltas from zero, so a surface can tell that it
    /// missed one.
    TextDelta { sequence: u64, text: String },
    /// The message being written was abandoned, for example because the model
    /// call was retried. Discard the live text.
    TextReset,
}
