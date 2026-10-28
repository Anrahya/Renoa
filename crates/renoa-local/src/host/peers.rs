//! Messaging between agents: the tools an agent calls and the port a node
//! implements.
//!
//! The Host knows nothing of how a message travels. A node attaches an
//! [`AgentPeers`] to its Host when it opens; until then the Host offers
//! neither tool. Once attached, the tools stay offered and fail with an error
//! that says to try again while the node's coordinator link is down. Both
//! tools belong to `renoa.agents`, so the owner's switch for that plugin
//! governs them like any other ability.

use std::sync::Arc;

use renoa_agent::{
    BoxFuture, ContentBlock, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates,
};
use renoa_agent_loop::AgentToolBinding;
use renoa_kernel::{AgentId, CommandId, EffectRecovery};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{LocalHost, capabilities, stable_id::stable_id};

const BINDING_REVISION: &str = "renoa-agent-messages-v1";
const MESSAGE_DOMAIN: &str = "renoa.agent.message.v1";
/// The longest a name is echoed back in an error.
const MAX_ECHOED_BYTES: usize = 80;
const CORRECT_CALL: &str = r#"{"to": "Research", "text": "Please summarise today's notes and reply with the three most important points."}"#;

/// Another agent of the same owner that a message can reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerAgent {
    /// What the owner and other agents call it.
    pub name: String,
    /// Where the port delivers to it. Opaque to the Host and never shown to
    /// the agent.
    pub route: String,
}

/// One message from the agent executing `from_command`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerMessage {
    /// The command the sending agent is executing, which is how the receiver
    /// of the message learns who sent it.
    pub from_command: Uuid,
    /// Stable for one tool call, so delivering it again changes nothing.
    pub message_id: Uuid,
    pub to: PeerAgent,
    pub text: String,
}

/// Why a roster or a send failed. Each text is written for the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerError {
    /// The message was not delivered, and sending it again as it is would be
    /// refused again.
    Refused(String),
    /// The message was not delivered; trying again later may work.
    Unavailable(String),
    /// Whether the message was delivered is not known. The text says so and
    /// what sending it again would do.
    Unconfirmed(String),
}

/// How a node reaches the owner's other agents.
pub trait AgentPeers: Send + Sync {
    /// The agents `agent` can message right now, without itself. A lookup
    /// sends nothing, so it never fails as [`PeerError::Unconfirmed`].
    fn roster(&self, agent: AgentId) -> BoxFuture<'_, Result<Vec<PeerAgent>, PeerError>>;

    /// Delivers `message`. `Ok` means the receiving side holds it durably.
    fn send(&self, message: PeerMessage) -> BoxFuture<'_, Result<(), PeerError>>;
}

impl LocalHost {
    /// Lets this Host's agents message the owner's other agents through
    /// `peers`, replacing any earlier attachment. A node attaches itself when
    /// it opens on the Host; until then no agent is offered a messaging tool.
    pub fn attach_agent_peers(&self, peers: Arc<dyn AgentPeers>) {
        *self
            .config
            .peers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(peers);
    }
}

pub(crate) fn bindings(
    peers: Arc<dyn AgentPeers>,
    actor: AgentId,
    command: Option<CommandId>,
) -> [AgentToolBinding; 2] {
    [
        AgentToolBinding::new(
            BINDING_REVISION,
            Arc::new(Roster {
                peers: Arc::clone(&peers),
                actor,
                spec: ToolSpec {
                    name: capabilities::AGENT_ROSTER.to_owned(),
                    description: "List the other agents you can message with message_agent, by name. They may run on this machine or on another of your owner's machines. Only agents whose machine is on right now are listed.".to_owned(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {},
                        "additionalProperties": false
                    }),
                },
            }),
            EffectRecovery::SafeToReplay,
        ),
        AgentToolBinding::new(
            BINDING_REVISION,
            Arc::new(Message {
                peers,
                actor,
                command,
                spec: ToolSpec {
                    name: capabilities::MESSAGE_AGENT.to_owned(),
                    description: "Send a message to another agent and carry on; do not wait. Its answer arrives later as a new message in this conversation, so say what you want back. The other agent sees only the text you send and who it is from, not this conversation. Use agent_roster to see who can be messaged.".to_owned(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {
                            "to": {
                                "type": "string",
                                "description": "The name of the agent, as agent_roster lists it."
                            },
                            "text": {
                                "type": "string",
                                "description": "What to tell or ask it, complete in itself."
                            }
                        },
                        "required": ["to", "text"],
                        "additionalProperties": false
                    }),
                },
            }),
            EffectRecovery::SafeToReplay,
        ),
    ]
}

struct Roster {
    peers: Arc<dyn AgentPeers>,
    actor: AgentId,
    spec: ToolSpec,
}

impl Tool for Roster {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        call: ToolCall,
        cancellation: CancellationToken,
        _: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(self.run(call, cancellation))
    }
}

impl Roster {
    async fn run(
        &self,
        call: ToolCall,
        cancellation: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        let object = arguments(&call, capabilities::AGENT_ROSTER)?;
        if let Some(field) = object.keys().next() {
            return Err(ToolError::invalid_input(format!(
                "`{}` is not a field of agent_roster, which takes none. A correct call is {{}}.",
                echoed(field)
            )));
        }
        let roster = tokio::select! {
            () = cancellation.cancelled() => return Err(cancelled("listing agents")),
            roster = self.peers.roster(self.actor) => roster,
        }
        .map_err(|error| tool_error("Could not list the agents", error, false))?;
        let agents: Vec<Value> = roster
            .iter()
            .map(|agent| json!({ "name": agent.name }))
            .collect();
        Ok(text(json!({ "agents": agents }).to_string()))
    }
}

struct Message {
    peers: Arc<dyn AgentPeers>,
    actor: AgentId,
    command: Option<CommandId>,
    spec: ToolSpec,
}

impl Tool for Message {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        call: ToolCall,
        cancellation: CancellationToken,
        _: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(self.run(call, cancellation))
    }
}

impl Message {
    async fn run(
        &self,
        call: ToolCall,
        cancellation: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        let object = arguments(&call, capabilities::MESSAGE_AGENT)?;
        if let Some(field) = object
            .keys()
            .find(|key| !matches!(key.as_str(), "to" | "text"))
        {
            return Err(ToolError::invalid_input(format!(
                "`{}` is not a field of message_agent, which takes `to` and `text`. A correct call is {CORRECT_CALL}.",
                echoed(field)
            )));
        }
        let to = string_field(object, "to")?;
        let body = string_field(object, "text")?;
        let command = self.command.ok_or_else(|| {
            ToolError::unavailable(
                "message_agent works only while you are answering a message. Wait for the next message and call it then.",
            )
        })?;
        let from_command = Uuid::parse_str(&command.to_string())
            .map_err(|_| ToolError::internal("the executing command has no usable identity"))?;
        let roster = tokio::select! {
            () = cancellation.cancelled() => return Err(cancelled("messaging")),
            roster = self.peers.roster(self.actor) => roster,
        }
        .map_err(|error| {
            tool_error(
                "Could not look up the agents, so nothing was sent",
                error,
                false,
            )
        })?;
        let target = resolve(&roster, to)?;
        let message = PeerMessage {
            from_command,
            message_id: stable_id(&format!("{MESSAGE_DOMAIN}:{command}:{}", call.id)),
            to: target.clone(),
            text: body.to_owned(),
        };
        tokio::select! {
            () = cancellation.cancelled() => return Err(ToolError::cancelled(
                "Sending was cancelled. The message may or may not have been delivered.",
                true,
            )),
            sent = self.peers.send(message) => sent,
        }
        .map_err(|error| tool_error(&format!("Could not message {}", target.name), error, true))?;
        Ok(text(format!(
            "Sent to {}. Its answer will arrive later as a new message in this conversation; you do not need to wait or look for it.",
            target.name
        )))
    }
}

fn resolve<'a>(roster: &'a [PeerAgent], to: &str) -> Result<&'a PeerAgent, ToolError> {
    let wanted = to.trim().to_lowercase();
    let mut matches = roster
        .iter()
        .filter(|agent| agent.name.trim().to_lowercase() == wanted);
    let found = matches.next();
    if let (Some(first), Some(_)) = (found, matches.next()) {
        return Err(ToolError::invalid_input(format!(
            "More than one agent is named \"{}\", so the name does not say which one you mean. Ask the owner to give them different names.",
            first.name
        )));
    }
    found.ok_or_else(|| {
        let known = roster
            .iter()
            .map(|agent| format!("\"{}\"", agent.name))
            .collect::<Vec<_>>();
        match known.first() {
            None => ToolError::unavailable(
                "There is no other agent to message: none of your owner's other agents is on right now. Try again later.",
            ),
            Some(first) => ToolError::invalid_input(format!(
                "No agent named \"{}\" can be messaged. The agents you can message are {}. A correct call is {{\"to\": {first}, \"text\": \"...\"}}.",
                echoed(to),
                known.join(", ")
            )),
        }
    })
}

fn arguments<'a>(
    call: &'a ToolCall,
    tool: &str,
) -> Result<&'a serde_json::Map<String, Value>, ToolError> {
    call.arguments.as_object().ok_or_else(|| {
        ToolError::invalid_input(format!(
            "{tool} takes an object but received {}. A correct call is {}.",
            json_type(&call.arguments),
            if tool == capabilities::MESSAGE_AGENT {
                CORRECT_CALL
            } else {
                "{}"
            }
        ))
    })
}

fn string_field<'a>(
    object: &'a serde_json::Map<String, Value>,
    field: &str,
) -> Result<&'a str, ToolError> {
    let value = object.get(field).ok_or_else(|| {
        ToolError::invalid_input(format!(
            "`{field}` is missing. message_agent needs `to` and `text`. A correct call is {CORRECT_CALL}."
        ))
    })?;
    let text = value.as_str().ok_or_else(|| {
        ToolError::invalid_input(format!(
            "`{field}` must be a string but received {}. A correct call is {CORRECT_CALL}.",
            json_type(value)
        ))
    })?;
    if text.trim().is_empty() {
        return Err(ToolError::invalid_input(format!(
            "`{field}` is empty. A correct call is {CORRECT_CALL}."
        )));
    }
    Ok(text)
}

fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

fn echoed(value: &str) -> String {
    if value.len() <= MAX_ECHOED_BYTES {
        return value.to_owned();
    }
    let mut end = MAX_ECHOED_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &value[..end])
}

/// The error for a failed port call, which says whether repeating the call
/// could help. `sending` is true for a message, whose refusal or failure also
/// says that nothing was sent; an unconfirmed message says its own outcome.
fn tool_error(what: &str, error: PeerError, sending: bool) -> ToolError {
    let not_sent = if sending { " Nothing was sent." } else { "" };
    match error {
        PeerError::Refused(reason) => ToolError::invalid_input(format!(
            "{what}: {}{not_sent} Repeating the same call straight away would be refused again.",
            sentence(&reason)
        )),
        PeerError::Unavailable(reason) => ToolError::unavailable(format!(
            "{what}: {}{not_sent} Trying again in a little while may work.",
            sentence(&reason)
        )),
        PeerError::Unconfirmed(reason) => {
            ToolError::unavailable(format!("{what}: {}", sentence(&reason)))
        }
    }
}

/// `reason` as a whole sentence, so the advice after it reads on.
fn sentence(reason: &str) -> String {
    let reason = reason.trim();
    if reason.ends_with(['.', '!', '?']) {
        reason.to_owned()
    } else {
        format!("{reason}.")
    }
}

fn cancelled(what: &str) -> ToolError {
    ToolError::cancelled(format!("{what} was cancelled."), false)
}

fn text(text: String) -> ToolOutput {
    ToolOutput {
        content: vec![ContentBlock::text(text)],
        details: None,
        is_error: false,
    }
}

#[cfg(test)]
mod tests;
