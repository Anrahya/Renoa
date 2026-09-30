//! The command a Host admits for one operation, and its stored wire shape.

use renoa_agent::ContentBlock;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{
    turn_context::{TurnContext, TurnContextError},
    turn_timing::TurnTiming,
};

/// Command content consumed by the model/tool loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCommand {
    kind: AgentCommandKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AgentCommandKind {
    Prompt {
        content: Vec<ContentBlock>,
        observation: Observation,
    },
    Compact,
}

/// When the Host admitted a prompt, and what it admitted with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Observation {
    Unobserved,
    /// Admitted before per-message context existed. Kept so a stored command
    /// decodes and re-encodes byte-identically.
    Timed(TurnTiming),
    Observed {
        observed_at_unix_ms: i64,
        context: TurnContext,
    },
}

#[derive(Serialize)]
#[serde(untagged)]
enum AgentCommandRef<'a> {
    Prompt(PromptCommandRef<'a>),
    Control(ControlCommand),
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct PromptCommandRef<'a> {
    content: &'a [ContentBlock],
    #[serde(skip_serializing_if = "Option::is_none")]
    turn_timing: Option<&'a TurnTiming>,
    #[serde(skip_serializing_if = "Option::is_none")]
    observed_at_unix_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<&'a TurnContext>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum AgentCommandWire {
    Prompt(PromptCommand),
    Control(ControlCommand),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PromptCommand {
    content: Vec<ContentBlock>,
    turn_timing: Option<TurnTiming>,
    observed_at_unix_ms: Option<i64>,
    context: Option<TurnContext>,
}

impl PromptCommand {
    fn observation(&self) -> Result<Observation, &'static str> {
        match (&self.turn_timing, self.observed_at_unix_ms, &self.context) {
            (None, None, None) => Ok(Observation::Unobserved),
            (Some(timing), None, None) => Ok(Observation::Timed(timing.clone())),
            (None, Some(at), context) if at >= 0 => Ok(Observation::Observed {
                observed_at_unix_ms: at,
                context: context.clone().unwrap_or_default(),
            }),
            (None, Some(_), _) => Err("a prompt cannot be observed before the Unix epoch"),
            (None, None, Some(_)) => Err("turn context requires observed_at_unix_ms"),
            (Some(_), ..) => Err("turn_timing cannot be combined with turn context"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlCommand {
    control: ControlKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ControlKind {
    Compact,
}

impl AgentCommand {
    #[must_use]
    pub fn new(content: Vec<ContentBlock>) -> Self {
        Self {
            kind: AgentCommandKind::Prompt {
                content,
                observation: Observation::Unobserved,
            },
        }
    }

    /// Creates a prompt the Host admitted at `observed_at_unix_ms` with the
    /// context it computed then. Retries reuse this command unchanged.
    ///
    /// # Errors
    ///
    /// Rejects a time before the Unix epoch.
    pub fn observed(
        content: Vec<ContentBlock>,
        observed_at_unix_ms: i64,
        turn_context: TurnContext,
    ) -> Result<Self, TurnContextError> {
        if observed_at_unix_ms < 0 {
            return Err(TurnContextError::BeforeUnixEpoch);
        }
        Ok(Self {
            kind: AgentCommandKind::Prompt {
                content,
                observation: Observation::Observed {
                    observed_at_unix_ms,
                    context: turn_context,
                },
            },
        })
    }

    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::new(vec![ContentBlock::text(text)])
    }

    #[must_use]
    pub const fn compact() -> Self {
        Self {
            kind: AgentCommandKind::Compact,
        }
    }

    /// Returns the model-visible prompt content, or an empty slice for a
    /// control command that deliberately contributes no conversation message.
    #[must_use]
    pub fn content(&self) -> &[ContentBlock] {
        match &self.kind {
            AgentCommandKind::Prompt { content, .. } => content,
            AgentCommandKind::Compact => &[],
        }
    }

    /// Returns prompt content while preserving the distinction from a control command.
    #[must_use]
    pub fn prompt_content(&self) -> Option<&[ContentBlock]> {
        match &self.kind {
            AgentCommandKind::Prompt { content, .. } => Some(content),
            AgentCommandKind::Compact => None,
        }
    }

    /// When the Host admitted this prompt, if it recorded one.
    #[must_use]
    pub const fn observed_at_unix_ms(&self) -> Option<i64> {
        match &self.kind {
            AgentCommandKind::Prompt {
                observation: Observation::Timed(timing),
                ..
            } => Some(timing.observed_at_unix_ms()),
            AgentCommandKind::Prompt {
                observation:
                    Observation::Observed {
                        observed_at_unix_ms,
                        ..
                    },
                ..
            } => Some(*observed_at_unix_ms),
            AgentCommandKind::Prompt {
                observation: Observation::Unobserved,
                ..
            }
            | AgentCommandKind::Compact => None,
        }
    }

    /// The context admitted with this prompt; empty when none was.
    #[must_use]
    pub fn context(&self) -> &[crate::ContextContribution] {
        match &self.kind {
            AgentCommandKind::Prompt {
                observation: Observation::Observed { context, .. },
                ..
            } => context.entries(),
            AgentCommandKind::Prompt { .. } | AgentCommandKind::Compact => &[],
        }
    }

    pub(crate) fn into_kind(self) -> AgentCommandKind {
        self.kind
    }
}

impl Serialize for AgentCommand {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match &self.kind {
            AgentCommandKind::Prompt {
                content,
                observation,
            } => {
                let (turn_timing, observed_at_unix_ms, context) = match observation {
                    Observation::Unobserved => (None, None, None),
                    Observation::Timed(timing) => (Some(timing), None, None),
                    Observation::Observed {
                        observed_at_unix_ms,
                        context,
                    } => (
                        None,
                        Some(*observed_at_unix_ms),
                        (!context.is_empty()).then_some(context),
                    ),
                };
                AgentCommandRef::Prompt(PromptCommandRef {
                    content,
                    turn_timing,
                    observed_at_unix_ms,
                    context,
                })
                .serialize(serializer)
            }
            AgentCommandKind::Compact => AgentCommandRef::Control(ControlCommand {
                control: ControlKind::Compact,
            })
            .serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for AgentCommand {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match AgentCommandWire::deserialize(deserializer)? {
            AgentCommandWire::Prompt(command) => {
                let observation = command.observation().map_err(serde::de::Error::custom)?;
                Ok(Self {
                    kind: AgentCommandKind::Prompt {
                        content: command.content,
                        observation,
                    },
                })
            }
            AgentCommandWire::Control(ControlCommand {
                control: ControlKind::Compact,
            }) => Ok(Self::compact()),
        }
    }
}
