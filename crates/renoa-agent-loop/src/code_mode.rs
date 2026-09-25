use std::collections::{BTreeMap, HashSet, VecDeque};
use std::fmt::Write as _;

use renoa_agent::{ContentBlock, ToolCall, ToolResult, ToolSpec};
use renoa_kernel::LoopError;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub(crate) const CODE_MODE_TOOL: &str = "code_mode";
pub(crate) const CODE_STEP_EFFECT_BINDING: &str = "renoa.agent.code-mode.step";
pub(crate) const MAX_CODE_BYTES: usize = 64 * 1024;
pub const MAX_CODE_CALLS_PER_WAVE: usize = 32;
pub const MAX_CODE_MCP_ARGUMENT_BYTES: usize = 256 * 1024;
pub const MAX_CODE_MCP_REFERENCE_BYTES: usize = 1024;
pub(crate) const MAX_CODE_CALLS_PER_RUN: u32 = 128;
pub(crate) const MAX_CODE_WAVES: u32 = 32;
pub const MAX_CODE_SNAPSHOT_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_CODE_RESULT_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_CODE_WAVE_RESULTS_BYTES: usize = 2 * 1024 * 1024;

/// The only model-supplied part of a Code Mode run.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CodeModeInput {
    pub(crate) source: String,
}

pub(crate) fn spec() -> ToolSpec {
    ToolSpec {
        name: CODE_MODE_TOOL.to_owned(),
        description: "Run Python for MCP work. Search for MCP tools with plugin_search. Use a returned input_schema to form arguments; if it is absent, call plugin_search with only reference to get the complete schema first. In Python, call await mcp(reference, arguments), or use asyncio.gather for independent calls. Each mcp result is a dictionary with content, details, and is_error. Only the final Python value is returned; each MCP call is durably recorded before dispatch."
            .to_owned(),
        input_schema: json!({
            "type": "object",
            "properties": {"source": {"type": "string", "description": "Python source; the final expression becomes the result."}},
            "required": ["source"],
            "additionalProperties": false
        }),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeStepRequest {
    pub run_id: String,
    pub step: CodeStep,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CodeStep {
    Start {
        source: String,
    },
    Resume {
        snapshot: String,
        results: BTreeMap<String, Value>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum CodeStepOutput {
    Completed {
        result: Value,
        is_error: bool,
    },
    Suspended {
        snapshot: String,
        calls: Vec<CodeMcpCall>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeMcpCall {
    pub call_id: u32,
    pub reference: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CodeRun {
    pub(crate) run_id: String,
    pub(crate) wave: u32,
    pub(crate) total_calls: u32,
}

impl CodeRun {
    pub(crate) fn new(
        operation_id: renoa_kernel::OperationId,
        model_turns: u32,
        call: &ToolCall,
    ) -> Self {
        let mut digest = Sha256::new();
        digest.update(operation_id.to_string());
        digest.update([0]);
        digest.update(model_turns.to_be_bytes());
        digest.update([0]);
        digest.update(call.id.as_bytes());
        let mut run_id = String::with_capacity(67);
        run_id.push_str("cm-");
        for byte in digest.finalize() {
            write!(&mut run_id, "{byte:02x}").expect("writing to a String cannot fail");
        }
        Self {
            run_id,
            wave: 0,
            total_calls: 0,
        }
    }

    pub(crate) fn add_wave(&mut self, calls: usize) -> Result<(), LoopError> {
        if calls == 0 || calls > MAX_CODE_CALLS_PER_WAVE {
            return Err(LoopError::new(
                "Code Mode produced an invalid number of MCP calls",
            ));
        }
        let wave = self
            .wave
            .checked_add(1)
            .ok_or_else(|| LoopError::new("Code Mode wave counter overflowed"))?;
        let total_calls = self
            .total_calls
            .checked_add(
                u32::try_from(calls)
                    .map_err(|_| LoopError::new("Code Mode call count overflowed"))?,
            )
            .ok_or_else(|| LoopError::new("Code Mode call counter overflowed"))?;
        if wave > MAX_CODE_WAVES || total_calls > MAX_CODE_CALLS_PER_RUN {
            return Err(LoopError::new("Code Mode exceeded its MCP call budget"));
        }
        self.wave = wave;
        self.total_calls = total_calls;
        Ok(())
    }

    pub(crate) fn call_id(&self, call_id: u32) -> String {
        format!("{}:{}:{}", self.run_id, self.wave, call_id)
    }

    pub(crate) fn validate_step(&self, request: &CodeStepRequest) -> Result<(), LoopError> {
        if request.run_id != self.run_id
            || self.wave > MAX_CODE_WAVES
            || self.total_calls > MAX_CODE_CALLS_PER_RUN
        {
            return Err(LoopError::new(
                "Code Mode step identity or budget is invalid",
            ));
        }
        let valid = match &request.step {
            CodeStep::Start { source } => {
                self.wave == 0
                    && self.total_calls == 0
                    && !source.trim().is_empty()
                    && source.len() <= MAX_CODE_BYTES
            }
            CodeStep::Resume { snapshot, results } => {
                self.wave > 0
                    && self.total_calls > 0
                    && !snapshot.is_empty()
                    && snapshot.len() <= MAX_CODE_SNAPSHOT_BYTES
                    && !results.is_empty()
                    && results.len() <= MAX_CODE_CALLS_PER_WAVE
                    && results.keys().all(|key| {
                        key.parse::<u32>()
                            .is_ok_and(|call_id| call_id.to_string() == *key)
                    })
            }
        };
        if !valid {
            return Err(LoopError::new("Code Mode step checkpoint is invalid"));
        }
        Ok(())
    }

    pub(crate) fn validate_call_wave(
        &self,
        snapshot: &str,
        calls: &CodeCallBatch,
    ) -> Result<(), LoopError> {
        calls.validate()?;
        let call_count = u32::try_from(calls.len())
            .map_err(|_| LoopError::new("Code Mode call count overflowed"))?;
        if self.wave == 0
            || self.wave > MAX_CODE_WAVES
            || self.total_calls < call_count
            || self.total_calls > MAX_CODE_CALLS_PER_RUN
            || snapshot.is_empty()
            || snapshot.len() > MAX_CODE_SNAPSHOT_BYTES
        {
            return Err(LoopError::new(
                "Code Mode call checkpoint or budget is invalid",
            ));
        }
        Ok(())
    }
}

/// An identity-indexed wave plus the evaluator's declaration order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CodeCallBatch {
    order: VecDeque<u32>,
    calls: BTreeMap<String, CodeMcpCall>,
}

impl CodeCallBatch {
    pub(crate) fn new(calls: Vec<CodeMcpCall>) -> Result<Self, LoopError> {
        let mut order = VecDeque::with_capacity(calls.len());
        let mut indexed = BTreeMap::new();
        for call in calls {
            order.push_back(call.call_id);
            if indexed.insert(call.call_id.to_string(), call).is_some() {
                return Err(LoopError::new("Code Mode repeated a Monty call identity"));
            }
        }
        let batch = Self {
            order,
            calls: indexed,
        };
        batch.validate()?;
        Ok(batch)
    }

    pub(crate) fn validate(&self) -> Result<(), LoopError> {
        if self.order.is_empty() || self.order.len() > MAX_CODE_CALLS_PER_WAVE {
            return Err(LoopError::new("Code Mode has an invalid MCP call wave"));
        }
        if self.order.len() != self.calls.len() {
            return Err(LoopError::new(
                "Code Mode call order differs from its identity map",
            ));
        }
        let mut observed = HashSet::with_capacity(self.order.len());
        for call_id in &self.order {
            if !observed.insert(*call_id) {
                return Err(LoopError::new("Code Mode call order repeats an identity"));
            }
            let call = self
                .calls
                .get(&call_id.to_string())
                .ok_or_else(|| LoopError::new("Code Mode call order has a missing identity"))?;
            if call.call_id != *call_id
                || call.reference.is_empty()
                || call.reference.len() > MAX_CODE_MCP_REFERENCE_BYTES
                || !call.arguments.is_object()
                || serde_json::to_vec(&call.arguments)
                    .map_err(|error| {
                        LoopError::new(format!("Code Mode MCP arguments are invalid: {error}"))
                    })?
                    .len()
                    > MAX_CODE_MCP_ARGUMENT_BYTES
            {
                return Err(LoopError::new("Code Mode has an invalid MCP call"));
            }
        }
        Ok(())
    }

    pub(crate) fn len(&self) -> usize {
        self.order.len()
    }

    pub(crate) fn ordered(&self) -> impl Iterator<Item = &CodeMcpCall> {
        self.order.iter().map(|id| {
            self.calls
                .get(&id.to_string())
                .expect("Code Mode call batch is validated")
        })
    }
}

pub(crate) fn nested_tool_call(run: &CodeRun, call: &CodeMcpCall, name: &str) -> ToolCall {
    ToolCall {
        id: run.call_id(call.call_id),
        name: name.to_owned(),
        arguments: json!({"reference": call.reference, "arguments": call.arguments}),
        thought_signature: None,
        namespace: None,
    }
}

pub(crate) fn python_result(result: &ToolResult) -> Value {
    json!({
        "content": result.content,
        "details": result.details,
        "is_error": result.is_error,
    })
}

pub(crate) fn model_result(
    call: &ToolCall,
    result: &Value,
    is_error: bool,
) -> Result<ToolResult, LoopError> {
    let content = serde_json::to_string(&result)
        .map_err(|error| LoopError::new(format!("Code Mode result encoding failed: {error}")))?;
    Ok(ToolResult {
        call_id: call.id.clone(),
        name: call.name.clone(),
        content: vec![ContentBlock::text(content)],
        details: None,
        is_error,
    })
}
