//! Who one prompt comes from, and what that lets its turn do.

use std::sync::Arc;

use renoa_agent::{BoxFuture, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates};
use renoa_agent_loop::AgentToolBinding;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const GUEST_REFUSAL: &str =
    "This message is from a guest, not the owner, so no tool can run for it. Answer in text.";

/// Who one prompt comes from. It decides the `USER.md` the turn reads and
/// edits, the plugin context it is admitted with, and whether its tool calls run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speaker {
    /// A person the Host knows: the turn reads and may edit their `USER.md`.
    Principal(Uuid),
    /// Someone on a surface whose chat identities are not Host principals: the
    /// turn has no `USER.md`, but keeps plugin context and every tool.
    Unidentified,
    /// Someone other than the owner in a conversation the owner shares: no
    /// `USER.md`, no plugin context, and every tool call is refused.
    Guest,
}

impl Speaker {
    /// The person whose `USER.md` the turn reads and may edit.
    #[must_use]
    pub(crate) const fn principal(self) -> Option<Uuid> {
        match self {
            Self::Principal(principal) => Some(principal),
            Self::Unidentified | Self::Guest => None,
        }
    }

    /// Whether plugins contribute context to this prompt. No plugin serves
    /// guests yet; one would have to opt in.
    #[must_use]
    pub(crate) const fn admits_plugin_context(self) -> bool {
        match self {
            Self::Principal(_) | Self::Unidentified => true,
            Self::Guest => false,
        }
    }

    #[must_use]
    pub(crate) const fn tool_access(self) -> ToolAccess {
        match self {
            Self::Principal(_) | Self::Unidentified => ToolAccess::Granted,
            Self::Guest => ToolAccess::Refused,
        }
    }
}

/// Whether a turn's tool calls run. A refused turn still offers every tool
/// definition, so a history holding earlier tool calls stays valid for the
/// provider; each call returns a refusal instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolAccess {
    Granted,
    Refused,
}

impl ToolAccess {
    pub(crate) fn apply(self, bindings: Vec<AgentToolBinding>) -> Vec<AgentToolBinding> {
        match self {
            Self::Granted => bindings,
            Self::Refused => bindings.iter().map(refused).collect(),
        }
    }
}

/// The same tool definition under its own revision, refusing every call. It
/// keeps the original recovery: a refusal is safe under either, and Code Mode
/// requires its nested executor to stay never-replayed.
fn refused(binding: &AgentToolBinding) -> AgentToolBinding {
    AgentToolBinding::new(
        format!("guest-refused/{}", binding.revision()),
        Arc::new(Refused {
            tool: binding.tool(),
        }),
        binding.recovery(),
    )
}

struct Refused {
    tool: Arc<dyn Tool>,
}

impl Tool for Refused {
    fn spec(&self) -> &ToolSpec {
        self.tool.spec()
    }

    fn execute(
        &self,
        _call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async { Err(ToolError::permission_denied(GUEST_REFUSAL)) })
    }
}

#[cfg(test)]
#[path = "speaker_tests.rs"]
mod tests;
