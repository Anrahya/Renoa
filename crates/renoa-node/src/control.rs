//! What a command's text asks of the node before it reaches the agent.
//!
//! RCP carries a command as opaque text and leaves its meaning to the harness
//! behind the node, so the Renoa node is the one place that knows `/compact`
//! is a control and not a message.

use renoa_local::{CompactInvocation, compact_invocation};
use renoa_protocol::{Author, CommandInput};

/// What the node runs for one command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Request {
    /// A message for the agent.
    Turn,
    /// The owner's `/compact`: summarize the conversation now, with no reply
    /// from the agent.
    Compact,
}

/// A command the node ends as failed before it opens a session.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Refusal(pub(crate) &'static str);

pub(crate) fn request(input: &CommandInput) -> Result<Request, Refusal> {
    match input.author() {
        // Compaction spends the owner's model calls, so only the owner can ask
        // for it. A guest's `/compact` is a message like any other.
        Author::Guest => Ok(Request::Turn),
        Author::Principal => match compact_invocation(input.text()) {
            CompactInvocation::Absent => Ok(Request::Turn),
            CompactInvocation::Bare => Ok(Request::Compact),
            CompactInvocation::WithArguments => Err(Refusal(
                "/compact takes no extra words. Send /compact on its own to summarize this conversation.",
            )),
        },
    }
}

#[cfg(test)]
mod tests {
    use renoa_protocol::{Author, CommandInput};

    use super::{Refusal, Request, request};

    fn text(text: &str, author: Author) -> CommandInput {
        CommandInput::Text {
            text: text.to_owned(),
            context: None,
            author,
        }
    }

    #[test]
    fn only_the_owners_bare_compact_is_a_control() {
        assert_eq!(
            request(&text(" /compact\n", Author::Principal)),
            Ok(Request::Compact)
        );
        assert_eq!(
            request(&text("/compact", Author::Guest)),
            Ok(Request::Turn),
            "a guest cannot spend the owner's model calls"
        );
        assert_eq!(
            request(&text("/compactly", Author::Principal)),
            Ok(Request::Turn)
        );
        assert_eq!(
            request(&text("summarize please", Author::Principal)),
            Ok(Request::Turn)
        );
    }

    #[test]
    fn the_owners_compact_with_words_is_refused_and_a_guests_is_not() {
        assert!(matches!(
            request(&text("/compact now", Author::Principal)),
            Err(Refusal(reason)) if reason.contains("on its own")
        ));
        assert_eq!(
            request(&text("/compact now", Author::Guest)),
            Ok(Request::Turn)
        );
    }
}
