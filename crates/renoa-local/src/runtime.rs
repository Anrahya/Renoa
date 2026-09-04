use std::{
    num::{NonZeroU32, NonZeroU64},
    path::PathBuf,
    sync::Arc,
};

use renoa_agent_loop::{
    AgentLoopBuildError, AgentLoopConfig, AgentToolBinding, CompactingContextStrategy,
    CompactionLimits, CompactionLimitsError, ContextBinding, ContextSizer, ModelBinding,
    build_runtime as build_agent_runtime,
    build_runtime_with_events as build_observed_agent_runtime,
};
use renoa_kernel::{EffectRecovery, Runtime};
use thiserror::Error;

use crate::{
    AgentProfile, AgentProfileError, BridgeModel, LocalWorkspace, ModelBridgeError, ModelChoice,
    ReasoningLevel, profile::AutomaticCompactionPolicy, skills::SkillRuntimeContext,
};

const MODEL_ROUND_LIMIT: NonZeroU32 = NonZeroU32::new(100).unwrap();
const TOOL_CALL_LIMIT: NonZeroU32 = NonZeroU32::new(16).unwrap();
const MAX_OUTPUT_TOKENS: NonZeroU32 = NonZeroU32::new(32_768).unwrap();
const COMPACTION_ATTEMPT_LIMIT: NonZeroU32 = NonZeroU32::new(2).unwrap();
const MAX_CHECKPOINT_TOKENS: u64 = 16_384;
const MIN_CONTEXT_SAFETY_TOKENS: u64 = 8_192;

/// Provider, model, and instruction inputs for one local coding runtime.
pub struct LocalRuntimeConfig {
    bridge: PathBuf,
    provider: String,
    model: String,
    credential_store: PathBuf,
    instructions: String,
    model_spec: Option<String>,
    reasoning: Option<ReasoningLevel>,
    skill_context: Option<SkillRuntimeContext>,
    automatic_compaction: Option<AutomaticCompactionPolicy>,
}

impl LocalRuntimeConfig {
    /// Selects one profile's versioned behavior and captures its workspace rules.
    ///
    /// # Errors
    ///
    /// Returns an error when the workspace's project instructions are invalid.
    pub fn for_profile(
        bridge: impl Into<PathBuf>,
        provider: impl Into<String>,
        model: impl Into<String>,
        credential_store: impl Into<PathBuf>,
        profile: &AgentProfile,
        workspace: &LocalWorkspace,
    ) -> Result<Self, AgentProfileError> {
        Ok(Self {
            bridge: bridge.into(),
            provider: provider.into(),
            model: model.into(),
            credential_store: credential_store.into(),
            instructions: profile.system_prompt(workspace.root())?,
            model_spec: None,
            reasoning: None,
            skill_context: None,
            automatic_compaction: profile.automatic_compaction(),
        })
    }

    /// Selects Renoa Alpha's built-in coding behavior.
    ///
    /// # Errors
    ///
    /// Returns an error when Alpha's workspace instructions are invalid.
    pub fn for_alpha(
        bridge: impl Into<PathBuf>,
        provider: impl Into<String>,
        model: impl Into<String>,
        credential_store: impl Into<PathBuf>,
        workspace: &LocalWorkspace,
    ) -> Result<Self, AgentProfileError> {
        Self::for_profile(
            bridge,
            provider,
            model,
            credential_store,
            &crate::alpha::alpha_profile(),
            workspace,
        )
    }

    #[must_use]
    pub fn with_discovered_model(mut self, model: &ModelChoice) -> Self {
        model.id().clone_into(&mut self.model);
        self.model_spec = Some(model.encoded_spec());
        self
    }

    #[must_use]
    pub fn with_reasoning(mut self, reasoning: ReasoningLevel) -> Self {
        self.reasoning = Some(reasoning);
        self
    }

    pub(crate) fn with_skill_context(mut self, context: SkillRuntimeContext) -> Self {
        self.skill_context = Some(context);
        self
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LocalRuntimeError {
    #[error(transparent)]
    Model(#[from] ModelBridgeError),
    #[error("model context reserve overflowed u64")]
    ContextReserveOverflow,
    #[error("model context is smaller than its output reserve")]
    ContextWindowTooSmall,
    #[error(
        "automatic compaction trigger ({automatic_compaction_input_tokens}) exceeds the model dispatch limit ({dispatch_limit_tokens})"
    )]
    AutomaticCompactionAboveDispatchLimit {
        automatic_compaction_input_tokens: u64,
        dispatch_limit_tokens: u64,
    },
    #[error("post-compaction target is zero")]
    ZeroCompactionTarget,
    #[error("checkpoint budget is zero")]
    ZeroCheckpointBudget,
    #[error(transparent)]
    ContextLimits(#[from] CompactionLimitsError),
    #[error(transparent)]
    AgentLoop(#[from] AgentLoopBuildError),
}

/// Resolves one full-access local coding runtime for the durable kernel.
///
/// Every tool registered by `LocalWorkspace` is included. This function does
/// not apply a Host permission policy.
///
/// # Errors
///
/// Returns an error when model discovery, context limits, or runtime bindings
/// are invalid.
pub async fn build_local_runtime(
    config: LocalRuntimeConfig,
    workspace: &LocalWorkspace,
) -> Result<Runtime, LocalRuntimeError> {
    build_local_runtime_inner(config, workspace, Vec::new(), None).await
}

/// Resolves the same local runtime while forwarding transient model and tool events.
///
/// The event sink is for live presentation only and is excluded from the
/// kernel's frozen manifest.
///
/// # Errors
///
/// Returns the same resolution errors as [`build_local_runtime`].
pub async fn build_local_runtime_with_events(
    config: LocalRuntimeConfig,
    workspace: &LocalWorkspace,
    events: Arc<dyn renoa_agent::AgentEventSink>,
) -> Result<Runtime, LocalRuntimeError> {
    build_local_runtime_inner(config, workspace, Vec::new(), Some(events)).await
}

pub(crate) async fn build_composed_local_runtime(
    config: LocalRuntimeConfig,
    workspace: &LocalWorkspace,
    extension_tools: Vec<AgentToolBinding>,
    events: Option<Arc<dyn renoa_agent::AgentEventSink>>,
) -> Result<Runtime, LocalRuntimeError> {
    build_local_runtime_inner(config, workspace, extension_tools, events).await
}

async fn build_local_runtime_inner(
    config: LocalRuntimeConfig,
    workspace: &LocalWorkspace,
    extension_tools: Vec<AgentToolBinding>,
    events: Option<Arc<dyn renoa_agent::AgentEventSink>>,
) -> Result<Runtime, LocalRuntimeError> {
    let resolved = resolve_model(config).await?;
    let context = context_binding(
        &resolved.model,
        resolved.skill_context.as_ref(),
        resolved.automatic_compaction,
    )?;
    let model_revision = format!(
        "renoa-model-provider-node/v1/{}/{}/{}/reasoning-{}",
        resolved.provider,
        resolved.model_id,
        resolved.model.binding_id(),
        resolved.model.reasoning().as_str()
    );
    let config = AgentLoopConfig::new(resolved.instructions, MODEL_ROUND_LIMIT, TOOL_CALL_LIMIT);
    let model = ModelBinding::new(model_revision, resolved.model, EffectRecovery::SafeToReplay);
    let mut tools = workspace.kernel_tool_bindings();
    tools.extend(extension_tools);
    match events {
        Some(events) => build_observed_agent_runtime(config, context, model, tools, events),
        None => build_agent_runtime(config, context, model, tools),
    }
    .map_err(Into::into)
}

struct ResolvedModel {
    provider: String,
    model_id: String,
    instructions: String,
    model: Arc<BridgeModel>,
    skill_context: Option<SkillRuntimeContext>,
    automatic_compaction: Option<AutomaticCompactionPolicy>,
}

async fn resolve_model(config: LocalRuntimeConfig) -> Result<ResolvedModel, ModelBridgeError> {
    let mut instructions = config.instructions;
    if let Some(context) = &config.skill_context {
        instructions.push_str("\n\n");
        instructions.push_str(&context.instructions);
    }
    let model = Arc::new(
        BridgeModel::load_with_spec(
            config.bridge,
            config.provider.as_str(),
            config.model.as_str(),
            config.credential_store,
            config.model_spec,
            config.reasoning,
            MAX_OUTPUT_TOKENS,
        )
        .await?,
    );
    Ok(ResolvedModel {
        provider: config.provider,
        model_id: config.model,
        instructions,
        model,
        skill_context: config.skill_context,
        automatic_compaction: config.automatic_compaction,
    })
}

fn context_binding(
    model: &Arc<BridgeModel>,
    skill_context: Option<&SkillRuntimeContext>,
    automatic_compaction: Option<AutomaticCompactionPolicy>,
) -> Result<ContextBinding, LocalRuntimeError> {
    let settings = compaction_settings(model.as_ref(), automatic_compaction)?;
    let limits = CompactionLimits::new(
        settings.context,
        settings.reserved,
        settings.target,
        settings.max_summary,
    )?
    .with_automatic_compaction_input_tokens(settings.automatic_compaction)?;
    let skill_revision = skill_context.map_or("none", |context| context.revision.as_str());
    let revision = format!(
        "renoa.context.compaction.v1/context-{}/reserved-{}/automatic-{}/target-{}/summary-{}/attempts-{}/skills-{}",
        settings.context,
        settings.reserved,
        settings.automatic_compaction,
        settings.target,
        settings.max_summary,
        COMPACTION_ATTEMPT_LIMIT,
        skill_revision,
    );
    let concrete_sizer = Arc::clone(model);
    let sizer: Arc<dyn ContextSizer> = concrete_sizer;
    let strategy = match skill_context {
        Some(context) => CompactingContextStrategy::with_projector(
            limits,
            COMPACTION_ATTEMPT_LIMIT,
            sizer,
            Arc::clone(&context.projector),
        ),
        None => CompactingContextStrategy::new(limits, COMPACTION_ATTEMPT_LIMIT, sizer),
    };
    Ok(ContextBinding::new(revision, Arc::new(strategy)))
}

struct CompactionSettings {
    context: NonZeroU64,
    reserved: u64,
    automatic_compaction: NonZeroU64,
    target: NonZeroU64,
    max_summary: NonZeroU64,
}

fn compaction_settings(
    model: &BridgeModel,
    automatic_compaction: Option<AutomaticCompactionPolicy>,
) -> Result<CompactionSettings, LocalRuntimeError> {
    let context = model.context_window_tokens();
    let safety = (context.get() / 50).max(MIN_CONTEXT_SAFETY_TOKENS);
    let reserved = u64::from(model.max_output_tokens().get())
        .checked_add(safety)
        .ok_or(LocalRuntimeError::ContextReserveOverflow)?;
    let dispatch = context
        .get()
        .checked_sub(reserved)
        .and_then(NonZeroU64::new)
        .ok_or(LocalRuntimeError::ContextWindowTooSmall)?;
    let automatic_compaction_input_tokens =
        automatic_compaction.map_or(dispatch, |policy| policy.trigger_input_tokens);
    if automatic_compaction_input_tokens > dispatch {
        return Err(LocalRuntimeError::AutomaticCompactionAboveDispatchLimit {
            automatic_compaction_input_tokens: automatic_compaction_input_tokens.get(),
            dispatch_limit_tokens: dispatch.get(),
        });
    }
    let target = match automatic_compaction {
        Some(policy) => policy.target_input_tokens,
        None => dispatch
            .get()
            .checked_mul(3)
            .and_then(|value| value.checked_div(5))
            .and_then(NonZeroU64::new)
            .ok_or(LocalRuntimeError::ZeroCompactionTarget)?,
    };
    let max_summary = NonZeroU64::new(MAX_CHECKPOINT_TOKENS.min(target.get() / 4))
        .ok_or(LocalRuntimeError::ZeroCheckpointBudget)?;
    Ok(CompactionSettings {
        context,
        reserved,
        automatic_compaction: automatic_compaction_input_tokens,
        target,
        max_summary,
    })
}
