use std::{
    num::{NonZeroU32, NonZeroU64},
    path::PathBuf,
    sync::Arc,
};

use renoa_harness::{CompactionPolicy, CompactionPolicyError, RuntimeProfile, RuntimeProfileError};
use thiserror::Error;

use crate::{LocalWorkspace, PiModel, PiModelConfigError, PiModelOption, PiReasoningLevel};

const MODEL_ATTEMPT_LIMIT: NonZeroU32 = NonZeroU32::new(32).unwrap();
const TOOL_CALL_LIMIT: NonZeroU32 = NonZeroU32::new(16).unwrap();
const MAX_OUTPUT_TOKENS: NonZeroU32 = NonZeroU32::new(32_768).unwrap();
const COMPACTION_ATTEMPT_LIMIT: NonZeroU32 = NonZeroU32::new(2).unwrap();
const MAX_CHECKPOINT_TOKENS: u64 = 16_384;
const MIN_CONTEXT_SAFETY_TOKENS: u64 = 8_192;

/// Provider and instruction inputs for one local coding runtime.
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
    let model = Arc::new(
        PiModel::load_with_spec(
            config.bridge,
            &config.provider,
            &config.model,
            config.credential_store,
            config.model_spec,
            config.reasoning,
            MAX_OUTPUT_TOKENS,
        )
        .await?,
    );
    let compaction = compaction_policy(&model)?;
    RuntimeProfile::new(
        format!(
            "pi/{}/{}/{}/reasoning-{}/local-{}/compaction-v1",
            config.provider,
            config.model,
            model.binding_id(),
            model.reasoning().as_str(),
            workspace.binding_id()
        ),
        model.clone(),
        config.instructions,
        MODEL_ATTEMPT_LIMIT,
    )
    .with_tools(workspace.tool_bindings(), TOOL_CALL_LIMIT)
    .map(|profile| profile.with_compaction(compaction, model))
    .map_err(Into::into)
}

fn compaction_policy(model: &PiModel) -> Result<CompactionPolicy, LocalRuntimeError> {
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
    Ok(CompactionPolicy::new(
        context,
        reserved,
        target,
        max_summary,
        COMPACTION_ATTEMPT_LIMIT,
    )?)
}
