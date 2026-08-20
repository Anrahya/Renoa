use std::{
    num::{NonZeroU32, NonZeroU64},
    path::PathBuf,
    sync::Arc,
};

use renoa_agent_loop::{
    AgentLoopBuildError, AgentLoopConfig, CompactingContextStrategy, CompactionLimits,
    CompactionLimitsError, ContextBinding, ContextSizer, ModelBinding,
    build_runtime as build_agent_runtime,
};
use renoa_harness::{CompactionPolicy, CompactionPolicyError, RuntimeProfile, RuntimeProfileError};
use renoa_kernel::{EffectRecovery, Runtime};
use thiserror::Error;

use crate::{
    AlphaError, LocalWorkspace, PiModel, PiModelConfigError, PiModelOption, PiReasoningLevel,
};

const MODEL_ATTEMPT_LIMIT: NonZeroU32 = NonZeroU32::new(32).unwrap();
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
    reasoning: Option<PiReasoningLevel>,
}

impl LocalRuntimeConfig {
    #[must_use]
    pub fn new(
        bridge: impl Into<PathBuf>,
        provider: impl Into<String>,
        model: impl Into<String>,
        credential_store: impl Into<PathBuf>,
        instructions: impl Into<String>,
    ) -> Self {
        Self {
            bridge: bridge.into(),
            provider: provider.into(),
            model: model.into(),
            credential_store: credential_store.into(),
            instructions: instructions.into(),
            model_spec: None,
            reasoning: None,
        }
    }

    /// Selects Renoa Alpha's versioned coding behavior and captures workspace rules.
    ///
    /// # Errors
    ///
    /// Returns an error when the workspace's project instructions are invalid.
    pub fn for_alpha(
        bridge: impl Into<PathBuf>,
        provider: impl Into<String>,
        model: impl Into<String>,
        credential_store: impl Into<PathBuf>,
        workspace: &LocalWorkspace,
    ) -> Result<Self, AlphaError> {
        Ok(Self::new(
            bridge,
            provider,
            model,
            credential_store,
            crate::alpha::system_prompt(workspace.root())?,
        ))
    }

    #[must_use]
    pub fn with_discovered_model(mut self, model: &PiModelOption) -> Self {
        model.id().clone_into(&mut self.model);
        self.model_spec = Some(model.encoded_spec());
        self
    }

    #[must_use]
    pub fn with_reasoning(mut self, reasoning: PiReasoningLevel) -> Self {
        self.reasoning = Some(reasoning);
        self
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LocalRuntimeError {
    #[error(transparent)]
    Model(#[from] PiModelConfigError),
    #[error("model context reserve overflowed u64")]
    ContextReserveOverflow,
    #[error("model context is smaller than its output reserve")]
    ContextWindowTooSmall,
    #[error("post-compaction target is zero")]
    ZeroCompactionTarget,
    #[error("checkpoint budget is zero")]
    ZeroCheckpointBudget,
    #[error(transparent)]
    Compaction(#[from] CompactionPolicyError),
    #[error(transparent)]
    Profile(#[from] RuntimeProfileError),
    #[error(transparent)]
    ContextLimits(#[from] CompactionLimitsError),
    #[error(transparent)]
    AgentLoop(#[from] AgentLoopBuildError),
}

/// Resolves one provider model and freezes its local workspace bindings.
///
/// # Errors
///
/// Returns an error when the model, context limits, or tool bindings are invalid.
pub async fn build_local_profile(
    config: LocalRuntimeConfig,
    workspace: &LocalWorkspace,
) -> Result<RuntimeProfile, LocalRuntimeError> {
    let resolved = resolve_model(config).await?;
    let compaction = compaction_policy(resolved.model.as_ref())?;
    let concrete_model = Arc::clone(&resolved.model);
    let harness_model: Arc<dyn renoa_agent::Model> = concrete_model;
    RuntimeProfile::new(
        format!(
            "pi/{}/{}/{}/reasoning-{}/local-{}/compaction-v1",
            resolved.provider,
            resolved.model_id,
            resolved.model.binding_id(),
            resolved.model.reasoning().as_str(),
            workspace.binding_id()
        ),
        harness_model,
        resolved.instructions,
        MODEL_ATTEMPT_LIMIT,
    )
    .with_tools(workspace.tool_bindings(), TOOL_CALL_LIMIT)
    .map(|profile| profile.with_compaction(compaction, resolved.model))
    .map_err(Into::into)
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
    let resolved = resolve_model(config).await?;
    let context = context_binding(&resolved.model)?;
    let model_revision = format!(
        "pi/{}/{}/{}/reasoning-{}",
        resolved.provider,
        resolved.model_id,
        resolved.model.binding_id(),
        resolved.model.reasoning().as_str()
    );
    build_agent_runtime(
        AgentLoopConfig::new(resolved.instructions, MODEL_ATTEMPT_LIMIT, TOOL_CALL_LIMIT),
        context,
        ModelBinding::new(model_revision, resolved.model, EffectRecovery::SafeToReplay),
        workspace.kernel_tool_bindings(),
    )
    .map_err(Into::into)
}

struct ResolvedModel {
    provider: String,
    model_id: String,
    instructions: String,
    model: Arc<PiModel>,
}

async fn resolve_model(config: LocalRuntimeConfig) -> Result<ResolvedModel, PiModelConfigError> {
    let model = Arc::new(
        PiModel::load_with_spec(
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
        instructions: config.instructions,
        model,
    })
}

fn compaction_policy(model: &PiModel) -> Result<CompactionPolicy, LocalRuntimeError> {
    let settings = compaction_settings(model)?;
    Ok(CompactionPolicy::new(
        settings.context,
        settings.reserved,
        settings.target,
        settings.max_summary,
        COMPACTION_ATTEMPT_LIMIT,
    )?)
}

fn context_binding(model: &Arc<PiModel>) -> Result<ContextBinding, LocalRuntimeError> {
    let settings = compaction_settings(model.as_ref())?;
    let limits = CompactionLimits::new(
        settings.context,
        settings.reserved,
        settings.target,
        settings.max_summary,
    )?;
    let revision = format!(
        "renoa.context.compaction.v1/context-{}/reserved-{}/target-{}/summary-{}/attempts-{}",
        settings.context,
        settings.reserved,
        settings.target,
        settings.max_summary,
        COMPACTION_ATTEMPT_LIMIT
    );
    let concrete_sizer = Arc::clone(model);
    let sizer: Arc<dyn ContextSizer> = concrete_sizer;
    Ok(ContextBinding::new(
        revision,
        Arc::new(CompactingContextStrategy::new(
            limits,
            COMPACTION_ATTEMPT_LIMIT,
            sizer,
        )),
    ))
}

struct CompactionSettings {
    context: NonZeroU64,
    reserved: u64,
    target: NonZeroU64,
    max_summary: NonZeroU64,
}

fn compaction_settings(model: &PiModel) -> Result<CompactionSettings, LocalRuntimeError> {
    let context = model.context_window_tokens();
    let safety = (context.get() / 50).max(MIN_CONTEXT_SAFETY_TOKENS);
    let reserved = u64::from(model.max_output_tokens().get())
        .checked_add(safety)
        .ok_or(LocalRuntimeError::ContextReserveOverflow)?;
    let dispatch = context
        .get()
        .checked_sub(reserved)
        .ok_or(LocalRuntimeError::ContextWindowTooSmall)?;
    let target = dispatch
        .checked_mul(3)
        .and_then(|value| value.checked_div(5))
        .and_then(NonZeroU64::new)
        .ok_or(LocalRuntimeError::ZeroCompactionTarget)?;
    let max_summary = NonZeroU64::new(MAX_CHECKPOINT_TOKENS.min(target.get() / 4))
        .ok_or(LocalRuntimeError::ZeroCheckpointBudget)?;
    Ok(CompactionSettings {
        context,
        reserved,
        target,
        max_summary,
    })
}
