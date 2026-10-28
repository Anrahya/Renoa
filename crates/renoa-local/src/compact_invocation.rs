//! Whether a message's text invokes the `/compact` control.
//!
//! The Host owns this rule so a surface that carries plain text (ACP, and the
//! node behind RCP) cannot disagree about which text is a control. Slack's
//! `!compact` and Telegram's `/compact@bot` are those platforms' own spelling
//! and stay in their crates.

/// How a message's text addresses `/compact`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompactInvocation {
    /// Not an invocation: ordinary text, `/compactly` included.
    Absent,
    /// The text is exactly `/compact`, ignoring surrounding whitespace.
    Bare,
    /// `/compact` followed by more text. The control takes no arguments.
    WithArguments,
}

/// Classifies one message's text.
#[must_use]
pub fn compact_invocation(text: &str) -> CompactInvocation {
    let text = text.trim();
    match text.strip_prefix("/compact") {
        Some("") => CompactInvocation::Bare,
        Some(tail) if tail.starts_with(char::is_whitespace) => CompactInvocation::WithArguments,
        _ => CompactInvocation::Absent,
    }
}

#[cfg(test)]
mod tests {
    use super::{CompactInvocation, compact_invocation};

    #[test]
    fn bare_invocation_ignores_surrounding_whitespace() {
        for text in ["/compact", "  /compact\n", "\t/compact "] {
            assert_eq!(
                compact_invocation(text),
                CompactInvocation::Bare,
                "{text:?}"
            );
        }
    }

    #[test]
    fn any_text_after_the_command_is_an_argument() {
        for text in ["/compact now", "/compact\nkeep the plan", "/compact   x"] {
            assert_eq!(
                compact_invocation(text),
                CompactInvocation::WithArguments,
                "{text:?}"
            );
        }
    }

    #[test]
    fn similar_text_is_not_an_invocation() {
        for text in [
            "",
            "compact",
            "/compactly",
            "/compact-now",
            "/Compact",
            "please /compact",
            "//compact",
        ] {
            assert_eq!(
                compact_invocation(text),
                CompactInvocation::Absent,
                "{text:?}"
            );
        }
    }
}
