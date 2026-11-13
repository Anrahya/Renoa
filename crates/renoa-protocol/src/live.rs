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
    /// A short normalized plain-text line describing what the agent is doing.
    Activity { text: String },
}

/// Maximum UTF-8 byte length of an agent `activity` status string on the wire.
pub const MAX_ACTIVITY_BYTES: usize = 120;

/// Strips Unicode format (`Cf`) characters from `raw`.
#[must_use]
pub fn strip_format_chars(raw: &str) -> String {
    raw.chars().filter(|&ch| !is_unicode_format(ch)).collect()
}

/// Validates and normalizes an agent `activity` string across all protocol
/// boundaries.
///
/// Strips Unicode format (`Cf`) characters, trims leading and trailing
/// whitespace, rejects empty strings or control characters, and drops strings
/// exceeding [`MAX_ACTIVITY_BYTES`] UTF-8 bytes rather than truncating them.
#[must_use]
pub fn normalize_activity(raw: &str) -> Option<String> {
    let stripped = strip_format_chars(raw);
    let trimmed = stripped.trim();
    if trimmed.is_empty()
        || trimmed.chars().any(char::is_control)
        || trimmed.len() > MAX_ACTIVITY_BYTES
    {
        return None;
    }
    Some(trimmed.to_owned())
}

const fn is_unicode_format(ch: char) -> bool {
    matches!(
        ch as u32,
        0x00AD
            | 0x0600..=0x0605
            | 0x061C
            | 0x06DD
            | 0x070F
            | 0x0890..=0x0891
            | 0x08E2
            | 0x180E
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x2064
            | 0x2066..=0x206F
            | 0xFEFF
            | 0xFFF9..=0xFFFB
            | 0x110BD
            | 0x110CD
            | 0x13430..=0x1343F
            | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A
            | 0xE0001
            | 0xE0020..=0xE007F
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_activity_strips_cf_rejects_controls_and_drops_over_limit() {
        assert_eq!(normalize_activity(""), None);
        assert_eq!(normalize_activity("   \u{200B}\t "), None);
        assert_eq!(normalize_activity("first\nsecond"), None);
        assert_eq!(normalize_activity("bad\x1b[31mcolor"), None);
        assert_eq!(normalize_activity("zero\0byte"), None);
        assert_eq!(
            normalize_activity("  \u{200B}Reading \u{202E}main.rs\u{E0020}  ").as_deref(),
            Some("Reading main.rs")
        );
        let exact = "a".repeat(MAX_ACTIVITY_BYTES);
        assert_eq!(normalize_activity(&exact).as_deref(), Some(exact.as_str()));
        let too_long_ascii = "a".repeat(MAX_ACTIVITY_BYTES + 1);
        assert_eq!(normalize_activity(&too_long_ascii), None);
        let too_long_utf8 = "é".repeat(MAX_ACTIVITY_BYTES / 2 + 1);
        assert_eq!(normalize_activity(&too_long_utf8), None);
    }
}
