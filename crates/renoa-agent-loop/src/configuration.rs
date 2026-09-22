use std::{collections::HashSet, num::NonZeroU32, sync::Arc};

use renoa_agent::{AgentEventSink, Model, Tool, ToolSpec};
use renoa_kernel::{EffectBinding, EffectRecovery, LoopBinding, Runtime, RuntimeError};
use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    adapters::{ModelAdapter, ToolAdapter},
    code_mode::{CODE_MODE_TOOL, CODE_STEP_EFFECT_BINDING, spec as code_mode_spec},
    context::{ContextStrategy, FullHistoryStrategy},
    decision::{AgentLoop, LoopCodeMode, LoopTool},
};

pub(crate) const CHECKPOINT_SCHEMA_VERSION: u32 = 4;
pub(crate) const LOOP_BINDING: &str = "renoa.agent.model-tool-loop";
pub(crate) const LOOP_REVISION: &str = "12";
pub(crate) const MODEL_EFFECT_BINDING: &str = "renoa.agent.model";
const FULL_HISTORY_CONTEXT_REVISION: &str = "renoa.context.full-history.v1";
const HEX: &[u8; 16] = b"0123456789abcdef";

/// Durable behavior limits and instructions for one model/tool loop runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentLoopConfig {
    pub(crate) system_prompt: String,
    pub(crate) max_model_turns: Option<NonZeroU32>,
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
            max_model_turns: Some(max_model_turns),
            max_tool_calls_per_turn,
        }
    }

    /// Runs until completion, cancellation or failure, without a model-turn budget.
    #[must_use]
    pub fn until_complete(
        system_prompt: impl Into<String>,
        max_tool_calls_per_turn: NonZeroU32,
    ) -> Self {
        Self {
            system_prompt: system_prompt.into(),
            max_model_turns: None,
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

/// One replaceable context strategy plus its recovery-compatible identity.
pub struct ContextBinding {
    revision: String,
    strategy: Arc<dyn ContextStrategy>,
}

impl ContextBinding {
    /// Binds a context strategy to one stable behavior revision.
    #[must_use]
    pub fn new(revision: impl Into<String>, strategy: Arc<dyn ContextStrategy>) -> Self {
        Self {
            revision: revision.into(),
            strategy,
        }
    }

    /// Uses the built-in strategy that exposes the complete durable transcript.
    #[must_use]
    pub fn full_history() -> Self {
        Self::new(FULL_HISTORY_CONTEXT_REVISION, Arc::new(FullHistoryStrategy))
    }
}

/// One replaceable tool adapter plus its recovery-compatible identity.
pub struct AgentToolBinding {
    revision: String,
    tool: Arc<dyn Tool>,
    recovery: EffectRecovery,
}

/// The visible Python tool, its pure evaluator, and its hidden MCP executor.
pub struct CodeModeBinding {
    revision: String,
    evaluator: Arc<dyn renoa_kernel::EffectAdapter>,
    nested: AgentToolBinding,
}

impl CodeModeBinding {
    #[must_use]
    pub fn new(
        revision: impl Into<String>,
        evaluator: Arc<dyn renoa_kernel::EffectAdapter>,
        nested: AgentToolBinding,
    ) -> Self {
        Self {
            revision: revision.into(),
            evaluator,
            nested,
        }
    }
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

    /// Returns the model-visible name of the bound tool.
    #[must_use]
    pub fn tool_name(&self) -> &str {
        self.tool.spec().name.as_str()
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
    context: ContextBinding,
    model: ModelBinding,
    tools: Vec<AgentToolBinding>,
) -> Result<Runtime, AgentLoopBuildError> {
    build_runtime_inner(config, context, model, tools, None, None)
}

/// Builds a runtime whose transient model and tool events are forwarded to a host observer.
///
/// The observer is not part of the frozen runtime manifest and receives only
/// transient copies of runtime events. It is intended for live surfaces such
/// as ACP; authoritative history remains in kernel semantic events.
///
/// # Errors
///
/// Applies the same binding validation as [`build_runtime`].
pub fn build_runtime_with_events(
    config: AgentLoopConfig,
    context: ContextBinding,
    model: ModelBinding,
    tools: Vec<AgentToolBinding>,
    events: Arc<dyn AgentEventSink>,
) -> Result<Runtime, AgentLoopBuildError> {
    build_runtime_inner(config, context, model, tools, None, Some(events))
}

/// Builds a runtime with a single model-visible Code Mode capability and a
/// hidden, independently durable MCP execution binding.
///
/// # Errors
///
/// Rejects an invalid evaluator identity or a nested binding that is not the
/// Host's MCP executor.
pub fn build_runtime_with_code_mode(
    config: AgentLoopConfig,
    context: ContextBinding,
    model: ModelBinding,
    tools: Vec<AgentToolBinding>,
    code_mode: CodeModeBinding,
) -> Result<Runtime, AgentLoopBuildError> {
    build_runtime_inner(config, context, model, tools, Some(code_mode), None)
}

/// The observed form of [`build_runtime_with_code_mode`].
///
/// # Errors
///
/// Applies the same binding validation.
pub fn build_runtime_with_code_mode_and_events(
    config: AgentLoopConfig,
    context: ContextBinding,
    model: ModelBinding,
    tools: Vec<AgentToolBinding>,
    code_mode: CodeModeBinding,
    events: Arc<dyn AgentEventSink>,
) -> Result<Runtime, AgentLoopBuildError> {
    build_runtime_inner(config, context, model, tools, Some(code_mode), Some(events))
}

fn build_runtime_inner(
    config: AgentLoopConfig,
    context: ContextBinding,
    model: ModelBinding,
    tools: Vec<AgentToolBinding>,
    code_mode: Option<CodeModeBinding>,
    events: Option<Arc<dyn AgentEventSink>>,
) -> Result<Runtime, AgentLoopBuildError> {
    if context.revision.is_empty() {
        return Err(AgentLoopBuildError::EmptyContextRevision);
    }
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
            Arc::new(ToolAdapter::new(
                binding.tool,
                events.as_ref().map(Arc::clone),
            )),
        ));
    }

    let code_mode = code_mode
        .map(|binding| prepare_code_mode(binding, &names, events.as_ref()))
        .transpose()?;
    let config_digest = digest_configuration(
        &config,
        &context.revision,
        model.recovery,
        &digest_tools,
        code_mode.as_ref().map(|prepared| &prepared.digest),
    )?;
    let loop_plugin = Arc::new(AgentLoop::new(
        config,
        context.strategy,
        model.recovery,
        loop_tools,
        code_mode.as_ref().map(|prepared| LoopCodeMode {
            spec: prepared.spec.clone(),
            nested_name: prepared.nested_name.clone(),
            nested_binding: prepared.nested_binding.clone(),
            nested_recovery: prepared.nested_recovery,
        }),
    ));
    let mut effects = Vec::with_capacity(tool_adapters.len() + 3);
    effects.push(EffectBinding::new(
        MODEL_EFFECT_BINDING,
        model.revision,
        Arc::new(ModelAdapter::new(model.model, events)),
    ));
    effects.extend(tool_adapters);
    if let Some(prepared) = code_mode {
        effects.extend(prepared.effects);
    }
    Runtime::new(
        LoopBinding::new(LOOP_BINDING, LOOP_REVISION, loop_plugin),
        CHECKPOINT_SCHEMA_VERSION,
        config_digest,
        effects,
    )
    .map_err(Into::into)
}

struct PreparedCodeMode {
    spec: ToolSpec,
    nested_name: String,
    nested_binding: String,
    nested_recovery: EffectRecovery,
    digest: DigestCodeMode,
    effects: [EffectBinding; 2],
}

fn prepare_code_mode(
    binding: CodeModeBinding,
    direct_names: &HashSet<String>,
    events: Option<&Arc<dyn AgentEventSink>>,
) -> Result<PreparedCodeMode, AgentLoopBuildError> {
    if binding.revision.is_empty() {
        return Err(AgentLoopBuildError::EmptyCodeModeRevision);
    }
    let nested_spec = binding.nested.tool.spec().clone();
    if nested_spec.name != "tool_execute" {
        return Err(AgentLoopBuildError::InvalidCodeModeExecutor(
            nested_spec.name,
        ));
    }
    if binding.nested.revision.is_empty() {
        return Err(AgentLoopBuildError::EmptyToolRevision(nested_spec.name));
    }
    if binding.nested.recovery != EffectRecovery::NeverReplay {
        return Err(AgentLoopBuildError::ReplayableCodeModeExecutor);
    }
    if direct_names.contains(CODE_MODE_TOOL) {
        return Err(AgentLoopBuildError::DuplicateToolName(
            CODE_MODE_TOOL.to_owned(),
        ));
    }
    if direct_names.contains(&nested_spec.name) {
        return Err(AgentLoopBuildError::DuplicateToolName(nested_spec.name));
    }
    let nested_binding = tool_effect_binding(&nested_spec.name);
    let spec = code_mode_spec();
    let digest = DigestCodeMode {
        revision: binding.revision.clone(),
        spec: spec.clone(),
        nested_revision: binding.nested.revision.clone(),
        nested_spec: nested_spec.clone(),
        nested_recovery: binding.nested.recovery,
    };
    let effects = [
        EffectBinding::new(
            CODE_STEP_EFFECT_BINDING,
            binding.revision,
            binding.evaluator,
        ),
        EffectBinding::new(
            nested_binding.clone(),
            binding.nested.revision,
            Arc::new(ToolAdapter::new(
                binding.nested.tool,
                events.map(Arc::clone),
            )),
        ),
    ];
    Ok(PreparedCodeMode {
        spec,
        nested_name: nested_spec.name,
        nested_binding,
        nested_recovery: binding.nested.recovery,
        digest,
        effects,
    })
}

pub(crate) fn tool_effect_binding(tool_name: &str) -> String {
    format!("renoa.agent.tool/{tool_name}")
}

#[derive(Serialize)]
struct DigestConfiguration<'a> {
    system_prompt: &'a str,
    max_model_turns: Option<NonZeroU32>,
    max_tool_calls_per_turn: u32,
    context_revision: &'a str,
    model_recovery: EffectRecovery,
    tools: &'a [DigestTool],
    code_mode: Option<&'a DigestCodeMode>,
}

#[derive(Serialize)]
struct DigestTool {
    revision: String,
    spec: ToolSpec,
    recovery: EffectRecovery,
}

#[derive(Serialize)]
struct DigestCodeMode {
    revision: String,
    spec: ToolSpec,
    nested_revision: String,
    nested_spec: ToolSpec,
    nested_recovery: EffectRecovery,
}

fn digest_configuration(
    config: &AgentLoopConfig,
    context_revision: &str,
    model_recovery: EffectRecovery,
    tools: &[DigestTool],
    code_mode: Option<&DigestCodeMode>,
) -> Result<String, AgentLoopBuildError> {
    let encoded = serde_json::to_vec(&DigestConfiguration {
        system_prompt: &config.system_prompt,
        max_model_turns: config.max_model_turns,
        max_tool_calls_per_turn: config.max_tool_calls_per_turn.get(),
        context_revision,
        model_recovery,
        tools,
        code_mode,
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
    #[error("context binding revision cannot be empty")]
    EmptyContextRevision,
    #[error("model binding revision cannot be empty")]
    EmptyModelRevision,
    #[error("tool name cannot be empty")]
    EmptyToolName,
    #[error("tool `{0}` has an empty binding revision")]
    EmptyToolRevision(String),
    #[error("tool name `{0}` is configured more than once")]
    DuplicateToolName(String),
    #[error("Code Mode evaluator revision cannot be empty")]
    EmptyCodeModeRevision,
    #[error("Code Mode requires the hidden `tool_execute` executor, not `{0}`")]
    InvalidCodeModeExecutor(String),
    #[error("Code Mode's hidden MCP executor must be NeverReplay")]
    ReplayableCodeModeExecutor,
    #[error("agent-loop configuration cannot be encoded: {0}")]
    ConfigurationEncoding(#[source] serde_json::Error),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
}
