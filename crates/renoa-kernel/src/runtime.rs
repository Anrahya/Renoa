use std::{collections::BTreeMap, future::Future, pin::Pin, sync::Arc};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::{AgentId, Command, EffectId, KernelError, OperationId, SemanticEvent, SessionId};

/// Opaque loop-owned durable state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    schema_version: u32,
    state: Value,
}

impl Checkpoint {
    #[must_use]
    pub const fn new(schema_version: u32, state: Value) -> Self {
        Self {
            schema_version,
            state,
        }
    }

    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    #[must_use]
    pub const fn state(&self) -> &Value {
        &self.state
    }
}

/// One semantic fact requested by a loop decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEvent {
    pub(crate) kind: String,
    pub(crate) payload: Value,
}

impl NewEvent {
    #[must_use]
    pub fn new(kind: impl Into<String>, payload: Value) -> Self {
        Self {
            kind: kind.into(),
            payload,
        }
    }
}

/// Whether a possibly dispatched effect may run again after process loss.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum EffectRecovery {
    SafeToReplay,
    NeverReplay,
}

impl EffectRecovery {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::SafeToReplay => "safe_to_replay",
            Self::NeverReplay => "never_replay",
        }
    }
}

/// A definite adapter outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
#[non_exhaustive]
pub enum EffectOutcome {
    Success(Value),
    Failure { message: String },
}

/// What an adapter can prove after one invocation attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EffectCompletion {
    /// The adapter knows the exact final outcome.
    Settled(EffectOutcome),
    /// The adapter cannot prove whether the external effect completed.
    OutcomeUnknown,
}

impl From<EffectOutcome> for EffectCompletion {
    fn from(outcome: EffectOutcome) -> Self {
        Self::Settled(outcome)
    }
}

/// Exact persisted effect data plus the lifecycle signal for this attempt.
#[derive(Debug, Clone)]
pub struct EffectInvocation {
    pub effect_id: EffectId,
    pub request: Value,
    /// Cancelled when the caller drops the drive that owns this attempt.
    pub cancellation: CancellationToken,
}

/// Future returned by an effect adapter.
pub type EffectFuture<'a> = Pin<Box<dyn Future<Output = EffectCompletion> + Send + 'a>>;

/// A named external capability. The kernel invokes it only after durable dispatch.
pub trait EffectAdapter: Send + Sync {
    /// Runs one exact persisted invocation.
    ///
    /// The future must observe `invocation.cancellation` and resolve only after
    /// work it started has stopped. The kernel retains the session and database
    /// writer leases until that cleanup finishes, even when the driving caller
    /// goes away.
    fn invoke(&self, invocation: EffectInvocation) -> EffectFuture<'_>;
}

/// One loop-visible settled effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettledEffect {
    pub effect_id: EffectId,
    pub binding: String,
    pub binding_revision: String,
    pub request: Value,
    pub outcome: EffectOutcome,
}

/// One loop-visible effect whose external outcome cannot be proven.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownEffect {
    pub effect_id: EffectId,
    pub binding: String,
    pub binding_revision: String,
    pub request: Value,
}

/// Owned durable input for explicitly abandoning one unknown effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownEffectInput {
    pub agent_id: AgentId,
    pub session_id: SessionId,
    pub operation_id: OperationId,
    pub command: Command,
    pub events: Vec<SemanticEvent>,
    pub checkpoint: Checkpoint,
    pub effect: UnknownEffect,
}

/// The loop-owned state and semantic events that close an unknown effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownEffectAbandonment {
    pub checkpoint: Checkpoint,
    pub events: Vec<NewEvent>,
}

/// Owned durable input to a decision-only loop plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopInput {
    pub agent_id: AgentId,
    pub session_id: SessionId,
    pub operation_id: OperationId,
    pub command: Command,
    pub events: Vec<SemanticEvent>,
    pub checkpoint: Option<Checkpoint>,
    pub effect: Option<SettledEffect>,
}

/// A plugin failure that left the durable decision state unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct LoopError {
    message: String,
}

impl LoopError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// A pure decision producer. External capabilities are unavailable here.
pub trait LoopPlugin: Send + Sync {
    /// Produces the next state transition from an owned durable snapshot.
    ///
    /// # Errors
    ///
    /// A loop error commits no state and leaves the decision retryable.
    fn decide(&self, input: LoopInput) -> Result<LoopDecision, LoopError>;

    /// Closes the loop-owned state after the host abandons an unknown effect.
    ///
    /// This boundary is decision-only: its output cannot request another
    /// effect. A loop that does not support abandonment leaves the operation
    /// blocked by returning the default error.
    ///
    /// # Errors
    ///
    /// A loop error commits no state and leaves the unknown effect blocked.
    fn abandon_unknown_effect(
        &self,
        _input: UnknownEffectInput,
    ) -> Result<UnknownEffectAbandonment, LoopError> {
        Err(LoopError::new(
            "loop plugin does not support unknown-effect abandonment",
        ))
    }
}

/// The only state changes a loop plugin may request.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LoopDecision {
    InvokeEffect {
        checkpoint: Checkpoint,
        binding: String,
        request: Value,
        recovery: EffectRecovery,
    },
    AppendEventsAndContinue {
        checkpoint: Checkpoint,
        events: Vec<NewEvent>,
    },
    WaitForInput {
        checkpoint: Checkpoint,
        events: Vec<NewEvent>,
    },
    Complete {
        checkpoint: Checkpoint,
        events: Vec<NewEvent>,
    },
    Fail {
        checkpoint: Checkpoint,
        events: Vec<NewEvent>,
        reason: String,
    },
}

impl LoopDecision {
    pub(crate) const fn checkpoint(&self) -> &Checkpoint {
        match self {
            Self::InvokeEffect { checkpoint, .. }
            | Self::AppendEventsAndContinue { checkpoint, .. }
            | Self::WaitForInput { checkpoint, .. }
            | Self::Complete { checkpoint, .. }
            | Self::Fail { checkpoint, .. } => checkpoint,
        }
    }
}

/// The frozen identity of all execution behavior required by an operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeManifest {
    pub loop_binding: String,
    pub loop_revision: String,
    pub checkpoint_schema_version: u32,
    pub effect_bindings: BTreeMap<String, String>,
    pub config_digest: String,
}

pub(crate) fn require_compatible_checkpoint(
    manifest: &RuntimeManifest,
    checkpoint: Option<&Checkpoint>,
) -> Result<(), KernelError> {
    let Some(checkpoint) = checkpoint else {
        return Ok(());
    };
    let expected = manifest.checkpoint_schema_version;
    let found = checkpoint.schema_version();
    if found == expected {
        Ok(())
    } else {
        Err(KernelError::CheckpointSchemaMismatch { expected, found })
    }
}

/// A concrete loop implementation with its host-managed compatibility identity.
pub struct LoopBinding {
    name: String,
    revision: String,
    plugin: Arc<dyn LoopPlugin>,
}

impl LoopBinding {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        revision: impl Into<String>,
        plugin: Arc<dyn LoopPlugin>,
    ) -> Self {
        Self {
            name: name.into(),
            revision: revision.into(),
            plugin,
        }
    }
}

/// A concrete effect implementation with its host-managed compatibility identity.
pub struct EffectBinding {
    name: String,
    revision: String,
    adapter: Arc<dyn EffectAdapter>,
}

impl EffectBinding {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        revision: impl Into<String>,
        adapter: Arc<dyn EffectAdapter>,
    ) -> Self {
        Self {
            name: name.into(),
            revision: revision.into(),
            adapter,
        }
    }
}

/// A fully resolved agent runtime offered by a host to the durable kernel.
pub struct Runtime {
    manifest: RuntimeManifest,
    pub(crate) plugin: Arc<dyn LoopPlugin>,
    pub(crate) effects: BTreeMap<String, EffectBinding>,
}

impl Runtime {
    /// Builds a runtime and its exact frozen manifest.
    ///
    /// # Errors
    ///
    /// Rejects empty or duplicate binding identities and empty configuration
    /// digests before any operation can freeze them.
    pub fn new(
        loop_binding: LoopBinding,
        checkpoint_schema_version: u32,
        config_digest: impl Into<String>,
        effects: Vec<EffectBinding>,
    ) -> Result<Self, RuntimeError> {
        require_text(&loop_binding.name, "loop binding")?;
        require_text(&loop_binding.revision, "loop revision")?;
        if checkpoint_schema_version == 0 {
            return Err(RuntimeError::ZeroCheckpointSchema);
        }
        let config_digest = config_digest.into();
        require_text(&config_digest, "configuration digest")?;
        let mut effect_bindings = BTreeMap::new();
        let mut resolved_effects = BTreeMap::new();
        for effect in effects {
            require_text(&effect.name, "effect binding")?;
            require_text(&effect.revision, "effect revision")?;
            if effect_bindings
                .insert(effect.name.clone(), effect.revision.clone())
                .is_some()
            {
                return Err(RuntimeError::DuplicateEffectBinding(effect.name));
            }
            resolved_effects.insert(effect.name.clone(), effect);
        }
        Ok(Self {
            manifest: RuntimeManifest {
                loop_binding: loop_binding.name,
                loop_revision: loop_binding.revision,
                checkpoint_schema_version,
                effect_bindings,
                config_digest,
            },
            plugin: loop_binding.plugin,
            effects: resolved_effects,
        })
    }

    #[must_use]
    pub const fn manifest(&self) -> &RuntimeManifest {
        &self.manifest
    }

    pub(crate) fn resolve_effect(
        &self,
        name: &str,
        revision: &str,
    ) -> Option<Arc<dyn EffectAdapter>> {
        self.effects
            .get(name)
            .filter(|binding| binding.revision == revision)
            .map(|binding| Arc::clone(&binding.adapter))
    }
}

fn require_text(value: &str, field: &'static str) -> Result<(), RuntimeError> {
    if value.is_empty() {
        Err(RuntimeError::EmptyIdentity(field))
    } else {
        Ok(())
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeError {
    #[error("{0} cannot be empty")]
    EmptyIdentity(&'static str),
    #[error("checkpoint schema version must be non-zero")]
    ZeroCheckpointSchema,
    #[error("effect binding `{0}` is configured more than once")]
    DuplicateEffectBinding(String),
}
