use std::{collections::HashSet, num::NonZeroU32, sync::Arc};

use renoa_agent::{Model, Tool, ToolSpec};
use renoa_kernel::{EffectBinding, EffectRecovery, LoopBinding, Runtime, RuntimeError};
use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    adapters::{ModelAdapter, ToolAdapter},
    decision::{AgentLoop, LoopTool},
};

pub(crate) const CHECKPOINT_SCHEMA_VERSION: u32 = 1;
pub(crate) const LOOP_BINDING: &str = "renoa.agent.model-tool-loop";
pub(crate) const LOOP_REVISION: &str = "3";
pub(crate) const MODEL_EFFECT_BINDING: &str = "renoa.agent.model";
const HEX: &[u8; 16] = b"0123456789abcdef";

/// Durable behavior limits and instructions for one model/tool loop runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentLoopConfig {
    pub(crate) system_prompt: String,
    pub(crate) max_model_turns: NonZeroU32,
    pub(crate) max_tool_calls_per_turn: NonZeroU32,
}

impl AgentLoopConfig {
    #[must_use]
    pub fn new(
        system_prompt: impl Into<String>,
        max_model_turns: NonZeroU32,
        max_tool_calls_per_turn: NonZeroU32,
    ) -> Self {
        Self {
            system_prompt: system_prompt.into(),
            max_model_turns,
            max_tool_calls_per_turn,
        }
    }
}

/// One replaceable provider adapter plus its recovery-compatible identity.
pub struct ModelBinding {
    revision: String,
    model: Arc<dyn Model>,
    recovery: EffectRecovery,
}

impl ModelBinding {
    /// Binds a provider-neutral model implementation to one stable revision.
    #[must_use]
    pub fn new(
        revision: impl Into<String>,
        model: Arc<dyn Model>,
        recovery: EffectRecovery,
    ) -> Self {
        Self {
            revision: revision.into(),
            model,
            recovery,
        }
    }
}

/// One replaceable tool adapter plus its recovery-compatible identity.
pub struct AgentToolBinding {
    revision: String,
    tool: Arc<dyn Tool>,
    recovery: EffectRecovery,
}

impl AgentToolBinding {
    /// Binds a provider-neutral tool implementation to one stable revision.
    #[must_use]
    pub fn new(revision: impl Into<String>, tool: Arc<dyn Tool>, recovery: EffectRecovery) -> Self {
        Self {
            revision: revision.into(),
            tool,
            recovery,
        }
    }
}

/// Builds the concrete runtime offered to `renoa-kernel`.
///
/// The resulting manifest binds the exact instructions, limits, tool order,
/// tool specifications, recovery declarations, and implementation revisions.
///
/// # Errors
///
/// Rejects empty or duplicate tool identities and any invalid kernel runtime
/// binding before an operation can activate.
pub fn build_runtime(
    config: AgentLoopConfig,
    model: ModelBinding,
    tools: Vec<AgentToolBinding>,
) -> Result<Runtime, AgentLoopBuildError> {
    if model.revision.is_empty() {
        return Err(AgentLoopBuildError::EmptyModelRevision);
    }

    let mut names = HashSet::with_capacity(tools.len());
    let mut loop_tools = Vec::with_capacity(tools.len());
    let mut tool_adapters = Vec::with_capacity(tools.len());
    let mut digest_tools = Vec::with_capacity(tools.len());
    for binding in tools {
        let spec = binding.tool.spec().clone();
        if spec.name.is_empty() {
            return Err(AgentLoopBuildError::EmptyToolName);
        }
        if binding.revision.is_empty() {
            return Err(AgentLoopBuildError::EmptyToolRevision(spec.name));
        }
        if !names.insert(spec.name.clone()) {
            return Err(AgentLoopBuildError::DuplicateToolName(spec.name));
        }
        let effect_binding = tool_effect_binding(&spec.name);
        digest_tools.push(DigestTool {
            revision: binding.revision.clone(),
            spec: spec.clone(),
            recovery: binding.recovery,
        });
        loop_tools.push(LoopTool {
            spec,
            effect_binding: effect_binding.clone(),
            recovery: binding.recovery,
        });
        tool_adapters.push(EffectBinding::new(
            effect_binding,
            binding.revision,
            Arc::new(ToolAdapter::new(binding.tool)),
        ));
    }

    let config_digest = digest_configuration(&config, model.recovery, &digest_tools)?;
    let loop_plugin = Arc::new(AgentLoop::new(config, model.recovery, loop_tools));
    let mut effects = Vec::with_capacity(tool_adapters.len() + 1);
    effects.push(EffectBinding::new(
        MODEL_EFFECT_BINDING,
        model.revision,
        Arc::new(ModelAdapter::new(model.model)),
    ));
    effects.extend(tool_adapters);
    Runtime::new(
        LoopBinding::new(LOOP_BINDING, LOOP_REVISION, loop_plugin),
        CHECKPOINT_SCHEMA_VERSION,
        config_digest,
        effects,
    )
    .map_err(Into::into)
}

pub(crate) fn tool_effect_binding(tool_name: &str) -> String {
    format!("renoa.agent.tool/{tool_name}")
}

#[derive(Serialize)]
struct DigestConfiguration<'a> {
    system_prompt: &'a str,
    max_model_turns: u32,
    max_tool_calls_per_turn: u32,
    model_recovery: EffectRecovery,
    tools: &'a [DigestTool],
}

#[derive(Serialize)]
struct DigestTool {
    revision: String,
    spec: ToolSpec,
    recovery: EffectRecovery,
}

fn digest_configuration(
    config: &AgentLoopConfig,
    model_recovery: EffectRecovery,
    tools: &[DigestTool],
) -> Result<String, AgentLoopBuildError> {
    let encoded = serde_json::to_vec(&DigestConfiguration {
        system_prompt: &config.system_prompt,
        max_model_turns: config.max_model_turns.get(),
        max_tool_calls_per_turn: config.max_tool_calls_per_turn.get(),
        model_recovery,
        tools,
    })
    .map_err(AgentLoopBuildError::ConfigurationEncoding)?;
    let digest = Sha256::digest(encoded);
    let mut output = String::with_capacity(64);
    for byte in digest {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(output)
}

/// Invalid model/tool runtime composition.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AgentLoopBuildError {
    #[error("model binding revision cannot be empty")]
    EmptyModelRevision,
    #[error("tool name cannot be empty")]
    EmptyToolName,
    #[error("tool `{0}` has an empty binding revision")]
    EmptyToolRevision(String),
    #[error("tool name `{0}` is configured more than once")]
    DuplicateToolName(String),
    #[error("agent-loop configuration cannot be encoded: {0}")]
    ConfigurationEncoding(#[source] serde_json::Error),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
}
