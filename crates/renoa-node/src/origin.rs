//! How a command another agent caused is shown to the agent that runs it.
//!
//! The sender's name goes in the turn's `surface` context entry, the one place
//! the turn context admits a description of where a message came from. The
//! depth of the chain is not shown: it belongs to the coordinator.

use renoa_local::{Speaker, TurnObservation};
use renoa_protocol::{AgentMessageKind, AgentOrigin, Author, CommandEnvelope};

/// The longest sender name shown; the context entry holds 256 bytes in all.
const MAX_NAME_BYTES: usize = 100;

/// Who the Host treats as speaking `command`. Another agent's message talks to
/// no person, so it runs with every tool and no `USER.md`, never with the
/// owner's.
pub(crate) fn speaker(command: &CommandEnvelope) -> Speaker {
    match (&command.origin, command.input.author()) {
        (Some(_), _) => Speaker::Unidentified,
        (None, Author::Principal) => Speaker::Principal(command.principal_id.as_uuid()),
        (None, Author::Guest) => Speaker::Guest,
    }
}

/// `observation` with the `surface` context entry of `command`: the surface's
/// own, or, for another agent's message, who sent it.
pub(crate) fn observed(command: &CommandEnvelope, observation: TurnObservation) -> TurnObservation {
    match (&command.origin, command.input.context()) {
        (Some(origin), _) => observation.with_surface_context(surface_context(origin)),
        (None, Some(context)) => observation.with_surface_context(context),
        (None, None) => observation,
    }
}

/// The `surface` context entry for a command with `origin`.
fn surface_context(origin: &AgentOrigin) -> String {
    let name = shown(&origin.name);
    match origin.kind {
        AgentMessageKind::Request => format!(
            "Message from the agent {name}. Your answer is sent back to them automatically."
        ),
        AgentMessageKind::Reply => format!("Reply from the agent {name} to a message you sent."),
        AgentMessageKind::Failure => {
            format!("The agent {name} could not answer a message you sent.")
        }
    }
}

/// The text the agent reads: the command's own, or a line saying that it was
/// empty when the sender had nothing to add.
pub(crate) fn prompt_text(command: &CommandEnvelope) -> String {
    let text = command.input.text();
    if !text.trim().is_empty() {
        return text.to_owned();
    }
    match &command.origin {
        Some(origin) if origin.kind == AgentMessageKind::Failure => {
            format!("{} gave no reason.", shown(&origin.name))
        }
        Some(origin) => format!("{} finished without a written answer.", shown(&origin.name)),
        None => text.to_owned(),
    }
}

fn shown(name: &str) -> &str {
    let mut end = name.len().min(MAX_NAME_BYTES);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    &name[..end]
}

#[cfg(test)]
mod tests {
    use renoa_protocol::{
        AgentMessageKind, AgentOrigin, CommandEnvelope, CommandId, CommandInput, PrincipalId,
        SurfaceRef, TargetRef,
    };

    use renoa_local::Speaker;
    use renoa_protocol::Author;

    use super::{prompt_text, speaker, surface_context};

    fn origin(kind: AgentMessageKind, name: &str) -> AgentOrigin {
        AgentOrigin {
            from_command: CommandId::new(),
            from: TargetRef::new("agent:sender"),
            name: name.to_owned(),
            depth: 3,
            kind,
        }
    }

    fn command(text: &str, origin: Option<AgentOrigin>) -> CommandEnvelope {
        CommandEnvelope {
            command_id: CommandId::new(),
            principal_id: PrincipalId::new(),
            surface: SurfaceRef::new("agent"),
            target: TargetRef::new("agent:receiver"),
            input: CommandInput::from_text(text),
            origin,
        }
    }

    #[test]
    fn each_kind_names_the_sender_and_none_shows_the_depth() {
        for (kind, expected) in [
            (
                AgentMessageKind::Request,
                "Message from the agent Research.",
            ),
            (AgentMessageKind::Reply, "Reply from the agent Research"),
            (
                AgentMessageKind::Failure,
                "The agent Research could not answer",
            ),
        ] {
            let shown = surface_context(&origin(kind, "Research"));
            assert!(shown.starts_with(expected), "{shown}");
            assert!(!shown.contains('3'), "the depth is not shown: {shown}");
        }
    }

    #[test]
    fn a_long_name_still_fits_the_context_entry() {
        let shown = surface_context(&origin(AgentMessageKind::Request, &"é".repeat(300)));
        assert!(shown.len() <= 256, "{} bytes", shown.len());
    }

    #[test]
    fn an_agents_message_never_speaks_as_the_owner_or_a_guest() {
        let principal = PrincipalId::new();
        let mut person = command("Hello", None);
        person.principal_id = principal;
        assert_eq!(speaker(&person), Speaker::Principal(principal.as_uuid()));
        let mut guest = person.clone();
        guest.input = CommandInput::Text {
            text: "Hello".to_owned(),
            context: None,
            author: Author::Guest,
        };
        assert_eq!(speaker(&guest), Speaker::Guest);

        for kind in [
            AgentMessageKind::Request,
            AgentMessageKind::Reply,
            AgentMessageKind::Failure,
        ] {
            let mut message = person.clone();
            message.origin = Some(origin(kind, "Research"));
            assert_eq!(speaker(&message), Speaker::Unidentified, "{kind:?}");
        }
    }

    #[test]
    fn an_empty_answer_reads_as_one() {
        let reply = command("", Some(origin(AgentMessageKind::Reply, "Research")));
        assert_eq!(
            prompt_text(&reply),
            "Research finished without a written answer."
        );
        let failure = command(" ", Some(origin(AgentMessageKind::Failure, "Research")));
        assert_eq!(prompt_text(&failure), "Research gave no reason.");
        assert_eq!(prompt_text(&command("Hello", None)), "Hello");
    }
}
