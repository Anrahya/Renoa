use std::collections::BTreeMap;

use renoa_agent::{Message, ToolCall, ToolResult};
use renoa_kernel::{
    CancellationInput, CancellationTransition, EffectBatchFacts, EffectFact, EffectOutcome,
    EffectRecovery, EffectRequest, LoopDecision, LoopError, OperationId, SettledEffect,
    SettledEffectBatch, UnknownEffectAbandonment, UnknownEffectInput,
};

use super::{
    AgentLoop, LoopPhase, checkpoint, decode,
    effect_input::{require_effect, require_effect_identity, require_effect_request_identity},
    encode,
    interruption::{
        cancelled, require_cancellation_effect_identity, require_unknown_effect_identity,
    },
    message_events, unavailable_result,
};
use crate::{
    code_mode::{
        CODE_STEP_EFFECT_BINDING, CodeCallBatch, CodeModeInput, CodeRun, CodeStep, CodeStepOutput,
        CodeStepRequest, MAX_CODE_BYTES, MAX_CODE_RESULT_BYTES, MAX_CODE_SNAPSHOT_BYTES,
        MAX_CODE_WAVE_RESULTS_BYTES, model_result, nested_tool_call, python_result,
    },
    pending_tools::PendingToolCalls,
};

impl AgentLoop {
    pub(super) fn validate_code_phase(
        &self,
        phase: &LoopPhase,
        operation_id: OperationId,
    ) -> Result<(), LoopError> {
        let (LoopPhase::AwaitingCodeStep {
            model_turns,
            pending,
            run,
            ..
        }
        | LoopPhase::AwaitingCodeCalls {
            model_turns,
            pending,
            run,
            ..
        }) = phase
        else {
            return Ok(());
        };
        let code = self.code_executor()?;
        if pending.current().name != code.spec.name
            || run.run_id != CodeRun::new(operation_id, *model_turns, pending.current()).run_id
        {
            return Err(LoopError::new(
                "Code Mode checkpoint differs from its outer call identity",
            ));
        }
        Ok(())
    }

    pub(super) fn request_code_mode(
        model_turns: u32,
        pending: PendingToolCalls,
        operation_id: OperationId,
    ) -> Result<LoopDecision, LoopError> {
        let call = pending.current();
        let input = match serde_json::from_value::<CodeModeInput>(call.arguments.clone()) {
            Ok(input)
                if !input.source.trim().is_empty() && input.source.len() <= MAX_CODE_BYTES =>
            {
                input
            }
            _ => {
                let result = unavailable_result(
                    call,
                    "Code Mode requires nonempty Python source of at most 64 KiB.",
                );
                return Self::append_tool_result(model_turns, pending, result);
            }
        };
        let run = CodeRun::new(operation_id, model_turns, call);
        let request = CodeStepRequest {
            run_id: run.run_id.clone(),
            step: CodeStep::Start {
                source: input.source,
            },
        };
        Self::invoke_code_step(model_turns, pending, run, request)
    }

    fn invoke_code_step(
        model_turns: u32,
        pending: PendingToolCalls,
        run: CodeRun,
        request: CodeStepRequest,
    ) -> Result<LoopDecision, LoopError> {
        let encoded = encode("Code Mode step request", &request)?;
        Ok(LoopDecision::InvokeEffects {
            checkpoint: checkpoint(LoopPhase::AwaitingCodeStep {
                model_turns,
                pending,
                run,
                request,
            })?,
            effects: vec![EffectRequest {
                binding: CODE_STEP_EFFECT_BINDING.to_owned(),
                request: encoded,
                recovery: EffectRecovery::SafeToReplay,
            }],
        })
    }

    pub(super) fn settle_code_step(
        &self,
        model_turns: u32,
        pending: PendingToolCalls,
        mut run: CodeRun,
        request: &CodeStepRequest,
        effect_batch: Option<SettledEffectBatch>,
    ) -> Result<LoopDecision, LoopError> {
        let effect = require_effect(effect_batch, "Code Mode step")?;
        require_effect_identity(
            &effect,
            CODE_STEP_EFFECT_BINDING,
            &encode("Code Mode step request", request)?,
        )?;
        let output = match effect.outcome {
            EffectOutcome::Success(value) => {
                decode::<CodeStepOutput>("Code Mode step output", value)?
            }
            EffectOutcome::Failure { message } => return Self::fail_tool(&pending, message),
            _ => {
                return Err(LoopError::new(
                    "Code Mode step outcome version is unsupported",
                ));
            }
        };
        match output {
            CodeStepOutput::Completed { result, is_error } => {
                let encoded_bytes = serde_json::to_vec(&result)
                    .map_err(|error| {
                        LoopError::new(format!("Code Mode final result encoding failed: {error}"))
                    })?
                    .len();
                if encoded_bytes > MAX_CODE_RESULT_BYTES {
                    let error = unavailable_result(
                        pending.current(),
                        "Code Mode final result exceeds 1 MiB; return a smaller value.",
                    );
                    return Self::append_tool_result(model_turns, pending, error);
                }
                let result = model_result(pending.current(), &result, is_error)?;
                Self::append_tool_result(model_turns, pending, result)
            }
            CodeStepOutput::Suspended { snapshot, calls } => {
                if snapshot.is_empty() || snapshot.len() > MAX_CODE_SNAPSHOT_BYTES {
                    return Self::fail_tool(
                        &pending,
                        "Code Mode snapshot is empty or exceeds 2 MiB".to_owned(),
                    );
                }
                let calls = match CodeCallBatch::new(calls) {
                    Ok(calls) => calls,
                    Err(error) => return Self::fail_tool(&pending, error.to_string()),
                };
                if let Err(error) = run.add_wave(calls.len()) {
                    return Self::fail_tool(&pending, error.to_string());
                }
                let code = self
                    .code_mode
                    .as_ref()
                    .ok_or_else(|| LoopError::new("Code Mode binding is no longer configured"))?;
                let effects = calls
                    .ordered()
                    .map(|call| {
                        Ok(EffectRequest {
                            binding: code.nested_binding.clone(),
                            request: encode(
                                "Code Mode MCP request",
                                nested_tool_call(&run, call, &code.nested_name),
                            )?,
                            recovery: code.nested_recovery,
                        })
                    })
                    .collect::<Result<Vec<_>, LoopError>>()?;
                Ok(LoopDecision::InvokeEffects {
                    checkpoint: checkpoint(LoopPhase::AwaitingCodeCalls {
                        model_turns,
                        pending,
                        run,
                        snapshot,
                        calls,
                    })?,
                    effects,
                })
            }
        }
    }

    pub(super) fn settle_code_calls(
        &self,
        model_turns: u32,
        pending: PendingToolCalls,
        run: CodeRun,
        snapshot: String,
        calls: &CodeCallBatch,
        effect_batch: Option<SettledEffectBatch>,
    ) -> Result<LoopDecision, LoopError> {
        let batch = effect_batch
            .ok_or_else(|| LoopError::new("Code Mode MCP checkpoint has no settled batch"))?;
        let effects = self.validate_settled_code_batch(&run, calls, &batch)?;
        let mut results = BTreeMap::new();
        let mut result_bytes = 0_usize;
        for call in calls.ordered() {
            let identity = run.call_id(call.call_id);
            let effect = effects
                .get(&identity)
                .ok_or_else(|| LoopError::new("Code Mode MCP result is missing"))?;
            let result = match &effect.outcome {
                EffectOutcome::Success(value) => {
                    decode::<ToolResult>("Code Mode MCP tool result", value.clone())?
                }
                EffectOutcome::Failure { message } => {
                    return Self::fail_tool(&pending, message.clone());
                }
                _ => {
                    return Err(LoopError::new(
                        "Code Mode MCP outcome version is unsupported",
                    ));
                }
            };
            if result.call_id != identity || result.name != self.code_executor()?.nested_name {
                return Err(LoopError::new(
                    "Code Mode MCP result identity differs from its request",
                ));
            }
            let python_value = python_result(&result);
            let encoded_bytes = serde_json::to_vec(&python_value)
                .map_err(|error| {
                    LoopError::new(format!("Code Mode MCP result encoding failed: {error}"))
                })?
                .len();
            result_bytes = result_bytes.saturating_add(encoded_bytes);
            if result_bytes > MAX_CODE_WAVE_RESULTS_BYTES {
                let result = unavailable_result(
                    pending.current(),
                    "MCP results exceed Code Mode's 2 MiB per-wave limit; request less data.",
                );
                return Self::append_tool_result(model_turns, pending, result);
            }
            results.insert(call.call_id.to_string(), python_value);
        }
        let request = CodeStepRequest {
            run_id: run.run_id.clone(),
            step: CodeStep::Resume { snapshot, results },
        };
        Self::invoke_code_step(model_turns, pending, run, request)
    }

    fn validate_settled_code_batch<'a>(
        &self,
        run: &CodeRun,
        calls: &CodeCallBatch,
        batch: &'a SettledEffectBatch,
    ) -> Result<BTreeMap<String, &'a SettledEffect>, LoopError> {
        let code = self.code_executor()?;
        if batch.effects.len() != calls.len() {
            return Err(LoopError::new(
                "Code Mode MCP result count differs from its call wave",
            ));
        }
        let mut by_id = BTreeMap::new();
        for effect in &batch.effects {
            let request = decode::<ToolCall>("Code Mode MCP request", effect.request.clone())?;
            if by_id.insert(request.id.clone(), effect).is_some() {
                return Err(LoopError::new(
                    "Code Mode MCP result repeats a call identity",
                ));
            }
        }
        for call in calls.ordered() {
            let expected = nested_tool_call(run, call, &code.nested_name);
            let effect = by_id
                .get(&expected.id)
                .ok_or_else(|| LoopError::new("Code Mode MCP result is missing a call identity"))?;
            require_effect_identity(
                effect,
                &code.nested_binding,
                &encode("Code Mode MCP request", expected)?,
            )?;
        }
        Ok(by_id)
    }

    fn code_executor(&self) -> Result<&super::LoopCodeMode, LoopError> {
        self.code_mode
            .as_ref()
            .ok_or_else(|| LoopError::new("Code Mode binding is no longer configured"))
    }

    pub(super) fn abandon_unknown_code_step(
        &self,
        pending: &PendingToolCalls,
        request: &CodeStepRequest,
        input: &UnknownEffectInput,
    ) -> Result<UnknownEffectAbandonment, LoopError> {
        self.code_executor()?;
        require_unknown_effect_identity(
            &input.effect_batch,
            CODE_STEP_EFFECT_BINDING,
            &encode("Code Mode step request", request)?,
        )?;
        Ok(UnknownEffectAbandonment {
            checkpoint: checkpoint(LoopPhase::Terminal)?,
            events: code_unavailable_events(
                pending,
                "Code Mode evaluation ended without a recoverable result.",
            )?,
        })
    }

    pub(super) fn abandon_unknown_code_calls(
        &self,
        pending: &PendingToolCalls,
        run: &CodeRun,
        calls: &CodeCallBatch,
        input: &UnknownEffectInput,
    ) -> Result<UnknownEffectAbandonment, LoopError> {
        self.validate_code_facts(run, calls, &input.effect_batch, true)?;
        Ok(UnknownEffectAbandonment {
            checkpoint: checkpoint(LoopPhase::Terminal)?,
            events: code_unavailable_events(
                pending,
                "An MCP call may have finished, but Code Mode could not recover its result.",
            )?,
        })
    }

    pub(super) fn cancel_code_step(
        &self,
        pending: &PendingToolCalls,
        request: &CodeStepRequest,
        input: &CancellationInput,
    ) -> Result<CancellationTransition, LoopError> {
        self.code_executor()?;
        let batch = input
            .effect_batch
            .as_ref()
            .ok_or_else(|| LoopError::new("Code Mode step cancellation has no effect batch"))?;
        require_cancellation_effect_identity(
            batch,
            CODE_STEP_EFFECT_BINDING,
            &encode("Code Mode step request", request)?,
        )?;
        cancelled(code_unavailable_events(
            pending,
            "Code Mode was cancelled before its result entered conversation history.",
        )?)
    }

    pub(super) fn cancel_code_calls(
        &self,
        pending: &PendingToolCalls,
        run: &CodeRun,
        calls: &CodeCallBatch,
        input: &CancellationInput,
    ) -> Result<CancellationTransition, LoopError> {
        let batch = input
            .effect_batch
            .as_ref()
            .ok_or_else(|| LoopError::new("Code Mode MCP cancellation has no effect batch"))?;
        self.validate_code_facts(run, calls, batch, false)?;
        cancelled(code_unavailable_events(
            pending,
            "Code Mode was cancelled before its MCP results were resumed in Python.",
        )?)
    }

    fn validate_code_facts(
        &self,
        run: &CodeRun,
        calls: &CodeCallBatch,
        batch: &EffectBatchFacts,
        require_unknown: bool,
    ) -> Result<(), LoopError> {
        let code = self.code_executor()?;
        if batch.effects.len() != calls.len() {
            return Err(LoopError::new(
                "Code Mode MCP effect facts differ from its call wave",
            ));
        }
        let mut by_id = BTreeMap::new();
        let mut has_unknown = false;
        for fact in &batch.effects {
            let (kind, binding, request) = match fact {
                EffectFact::NotDispatched(effect) => {
                    ("not-dispatched", &effect.binding, &effect.request)
                }
                EffectFact::Settled(effect) => ("settled", &effect.binding, &effect.request),
                EffectFact::OutcomeUnknown(effect) => {
                    has_unknown = true;
                    ("unknown", &effect.binding, &effect.request)
                }
                _ => {
                    return Err(LoopError::new(
                        "Code Mode MCP effect fact version is unsupported",
                    ));
                }
            };
            let call = decode::<ToolCall>("Code Mode MCP fact request", request.clone())?;
            if by_id
                .insert(call.id.clone(), (kind, binding, request))
                .is_some()
            {
                return Err(LoopError::new(
                    "Code Mode MCP effect facts repeat a call identity",
                ));
            }
        }
        for call in calls.ordered() {
            let expected = nested_tool_call(run, call, &code.nested_name);
            let (kind, binding, request) = by_id
                .get(&expected.id)
                .ok_or_else(|| LoopError::new("Code Mode MCP effect fact is missing"))?;
            require_effect_request_identity(
                kind,
                binding,
                request,
                &code.nested_binding,
                &encode("Code Mode MCP request", expected)?,
            )?;
        }
        if require_unknown && !has_unknown {
            return Err(LoopError::new(
                "Code Mode abandonment has no unknown MCP call",
            ));
        }
        Ok(())
    }
}

fn code_unavailable_events(
    pending: &PendingToolCalls,
    current_message: &str,
) -> Result<Vec<renoa_kernel::NewEvent>, LoopError> {
    let current = Message::Tool {
        result: unavailable_result(pending.current(), current_message),
    };
    let later = pending.remaining().map(|call| Message::Tool {
        result: unavailable_result(
            call,
            "Tool call was not run because Code Mode did not complete.",
        ),
    });
    message_events(std::iter::once(current).chain(later))
}
