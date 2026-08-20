use std::collections::HashMap;

use renoa_agent::{AssistantContent, Message, ModelRequest, ToolSpec};

use super::{
    CompactionCheckpoint, CompactionLimits, CompactionPlan, CompactionPlanningError, ContextSizer,
    format::summary_request,
};
use crate::{ContextEntry, ContextInput};

pub(super) fn select_plan(
    context: &ContextInput,
    checkpoint: Option<CompactionCheckpoint<'_>>,
    system_prompt: &str,
    tools: &[ToolSpec],
    limits: CompactionLimits,
    sizer: &dyn ContextSizer,
) -> Result<Option<CompactionPlan>, CompactionPlanningError> {
    let all_entries = context.entries().collect::<Vec<_>>();
    let anchor = active_user_anchor(&all_entries, context.active_operation_id())?;
    let entries = entries_after_checkpoint(&all_entries, checkpoint)?;
    let cuts = safe_cut_indices(entries, context.active_operation_id())?;
    let Some((last_fitting, last_request)) = last_fitting_summary(
        entries,
        checkpoint,
        &cuts,
        limits.dispatch_limit_tokens().get(),
        sizer,
    )?
    else {
        return Ok(None);
    };
    let request_shape = TailRequestShape {
        system_prompt,
        tools,
    };
    let selected = first_tail_within_budget(
        entries,
        anchor,
        &cuts,
        last_fitting,
        limits.tail_budget_tokens().get(),
        request_shape,
        sizer,
    )
    .unwrap_or(last_fitting);
    let summary_request = if selected == last_fitting {
        last_request
    } else {
        summary_at(entries, checkpoint, cuts[selected])?
    };
    Ok(Some(CompactionPlan {
        summary_request,
        covered_through_sequence: entries[cuts[selected]].sequence(),
    }))
}

pub(super) fn validate_boundary(
    context: &ContextInput,
    covered_through_sequence: u64,
) -> Result<(), CompactionPlanningError> {
    let all_entries = context.entries().collect::<Vec<_>>();
    active_user_anchor(&all_entries, context.active_operation_id())?;
    let entries = entries_after_checkpoint(&all_entries, context.active_checkpoint())?;
    let cuts = safe_cut_indices(entries, context.active_operation_id())?;
    if cuts
        .iter()
        .any(|index| entries[*index].sequence() == covered_through_sequence)
    {
        Ok(())
    } else {
        Err(CompactionPlanningError::InvalidPlan(
            "covered boundary is not a safe transcript cut".to_owned(),
        ))
    }
}

fn active_user_anchor<'a>(
    entries: &'a [ContextEntry<'a>],
    active_operation_id: renoa_kernel::OperationId,
) -> Result<ContextEntry<'a>, CompactionPlanningError> {
    let anchor = entries
        .iter()
        .copied()
        .find(|entry| entry.operation_id() == active_operation_id)
        .ok_or(CompactionPlanningError::MissingActiveUser)?;
    if !matches!(anchor.message(), Message::User { .. }) {
        return Err(CompactionPlanningError::InvalidActiveUser);
    }
    Ok(anchor)
}

fn entries_after_checkpoint<'a>(
    entries: &'a [ContextEntry<'a>],
    checkpoint: Option<CompactionCheckpoint<'_>>,
) -> Result<&'a [ContextEntry<'a>], CompactionPlanningError> {
    let Some(checkpoint) = checkpoint else {
        return Ok(entries);
    };
    if checkpoint.summary().trim().is_empty() {
        return Err(CompactionPlanningError::InvalidCheckpoint(
            "summary is empty".to_owned(),
        ));
    }
    let boundary = entries
        .iter()
        .position(|entry| entry.sequence() == checkpoint.covered_through_sequence())
        .ok_or_else(|| {
            CompactionPlanningError::InvalidCheckpoint(
                "covered sequence is not a durable message".to_owned(),
            )
        })?;
    Ok(&entries[boundary + 1..])
}

fn last_fitting_summary(
    entries: &[ContextEntry<'_>],
    checkpoint: Option<CompactionCheckpoint<'_>>,
    cuts: &[usize],
    dispatch_limit: u64,
    sizer: &dyn ContextSizer,
) -> Result<Option<(usize, ModelRequest)>, CompactionPlanningError> {
    let mut first_unknown = 0;
    let mut first_oversized = cuts.len();
    let mut best = None;
    while first_unknown < first_oversized {
        let position = first_unknown + (first_oversized - first_unknown) / 2;
        let request = summary_at(entries, checkpoint, cuts[position])?;
        if sizer.estimate_input_tokens(&request) <= dispatch_limit {
            best = Some((position, request));
            first_unknown = position + 1;
        } else {
            first_oversized = position;
        }
    }
    Ok(best)
}

fn first_tail_within_budget(
    entries: &[ContextEntry<'_>],
    anchor: ContextEntry<'_>,
    cuts: &[usize],
    last_fitting: usize,
    tail_budget: u64,
    request_shape: TailRequestShape<'_>,
    sizer: &dyn ContextSizer,
) -> Option<usize> {
    let mut first_unknown = 0;
    let mut first_fitting = last_fitting + 1;
    while first_unknown < first_fitting {
        let position = first_unknown + (first_fitting - first_unknown) / 2;
        let request = tail_request(entries, anchor, cuts[position], request_shape);
        if sizer.estimate_input_tokens(&request) <= tail_budget {
            first_fitting = position;
        } else {
            first_unknown = position + 1;
        }
    }
    (first_fitting <= last_fitting).then_some(first_fitting)
}

fn summary_at(
    entries: &[ContextEntry<'_>],
    checkpoint: Option<CompactionCheckpoint<'_>>,
    cut_index: usize,
) -> Result<ModelRequest, CompactionPlanningError> {
    summary_request(
        checkpoint.map(CompactionCheckpoint::summary),
        &entries[..=cut_index],
    )
}

fn safe_cut_indices(
    entries: &[ContextEntry<'_>],
    active_operation_id: renoa_kernel::OperationId,
) -> Result<Vec<usize>, CompactionPlanningError> {
    let mut cuts = Vec::new();
    let mut operation = None;
    let mut pending = HashMap::<String, String>::new();
    for (index, entry) in entries.iter().enumerate() {
        if operation != Some(entry.operation_id()) {
            if !pending.is_empty() {
                return invalid_history(
                    "conversation changes operation inside an unresolved tool group",
                );
            }
            operation = Some(entry.operation_id());
        }
        let completed_tool_group = update_tool_group(entry.message(), &mut pending)?;
        let ends_operation = entries
            .get(index + 1)
            .is_none_or(|next| next.operation_id() != entry.operation_id());
        if ends_operation && !pending.is_empty() {
            return invalid_history("conversation operation ends with unresolved tool calls");
        }
        if (entry.operation_id() != active_operation_id && ends_operation)
            || (entry.operation_id() == active_operation_id && completed_tool_group)
        {
            cuts.push(index);
        }
    }
    Ok(cuts)
}

fn update_tool_group(
    message: &Message,
    pending: &mut HashMap<String, String>,
) -> Result<bool, CompactionPlanningError> {
    match message {
        Message::User { .. } => {
            if !pending.is_empty() {
                return invalid_history("user message appears inside an unresolved tool group");
            }
        }
        Message::Assistant { content, .. } => {
            if !pending.is_empty() {
                return invalid_history(
                    "assistant message appears before all tool results are present",
                );
            }
            for block in content {
                if let AssistantContent::ToolCall { call } = block
                    && pending.insert(call.id.clone(), call.name.clone()).is_some()
                {
                    return invalid_history("assistant tool-call identifiers are duplicated");
                }
            }
        }
        Message::Tool { result } => {
            let expected = pending
                .remove(&result.call_id)
                .ok_or_else(|| invalid_history_error("tool result has no pending call"))?;
            if expected != result.name {
                return invalid_history("tool result name does not match its call");
            }
            return Ok(pending.is_empty());
        }
    }
    Ok(false)
}

fn tail_request(
    entries: &[ContextEntry<'_>],
    anchor: ContextEntry<'_>,
    cut_index: usize,
    request_shape: TailRequestShape<'_>,
) -> ModelRequest {
    let covered = entries[cut_index].sequence();
    let mut messages = Vec::new();
    if anchor.sequence() <= covered {
        messages.push(anchor.message().clone());
    }
    messages.extend(
        entries[cut_index + 1..]
            .iter()
            .map(|entry| entry.message().clone()),
    );
    ModelRequest {
        system_prompt: request_shape.system_prompt.to_owned(),
        messages,
        tools: request_shape.tools.to_vec(),
    }
}

#[derive(Clone, Copy)]
struct TailRequestShape<'a> {
    system_prompt: &'a str,
    tools: &'a [ToolSpec],
}

fn invalid_history<T>(message: &str) -> Result<T, CompactionPlanningError> {
    Err(invalid_history_error(message))
}

fn invalid_history_error(message: &str) -> CompactionPlanningError {
    CompactionPlanningError::InvalidHistory(message.to_owned())
}
