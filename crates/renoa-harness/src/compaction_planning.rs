use std::collections::HashMap;

use renoa_agent::{AssistantContent, Message, ModelRequest};
use uuid::Uuid;

use crate::{
    ContextSizer, HarnessError, OperationId,
    checkpoint::ContextEntry,
    checkpoint_format::summary_request,
    compaction::{CompactionPlan, CompactionSource, FrozenCompaction},
};

pub(crate) fn select_plan(
    active_operation_id: OperationId,
    source: &CompactionSource,
    frozen: FrozenCompaction,
    sizer: &dyn ContextSizer,
) -> Result<Option<CompactionPlan>, HarnessError> {
    let dispatch_limit = frozen.dispatch_limit()?;
    let tail_budget = frozen
        .target_input_tokens
        .checked_sub(frozen.max_summary_tokens)
        .ok_or_else(|| {
            HarnessError::Corrupt("frozen checkpoint summary exceeds its target".to_owned())
        })?;
    let cuts = safe_cut_indices(&source.entries, active_operation_id)?;
    let Some((last_fitting, last_request)) =
        last_fitting_summary(source, &cuts, dispatch_limit, sizer)?
    else {
        return Ok(None);
    };
    let selected = first_tail_within_budget(
        source,
        active_operation_id,
        &cuts,
        last_fitting,
        tail_budget,
        sizer,
    )?
    .unwrap_or(last_fitting);
    let request = if selected == last_fitting {
        last_request
    } else {
        summary_at(source, cuts[selected])?
    };
    let covered_through_sequence = source.entries[cuts[selected]].sequence;
    Ok(Some(CompactionPlan {
        request,
        checkpoint_id: Uuid::new_v4(),
        previous_checkpoint_id: source
            .checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.checkpoint_id),
        covered_through_sequence,
    }))
}

fn last_fitting_summary(
    source: &CompactionSource,
    cuts: &[usize],
    dispatch_limit: u64,
    sizer: &dyn ContextSizer,
) -> Result<Option<(usize, ModelRequest)>, HarnessError> {
    let mut first_unknown = 0;
    let mut first_oversized = cuts.len();
    let mut best = None;
    while first_unknown < first_oversized {
        let position = first_unknown + (first_oversized - first_unknown) / 2;
        let request = summary_at(source, cuts[position])?;
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
    source: &CompactionSource,
    active_operation_id: OperationId,
    cuts: &[usize],
    last_fitting: usize,
    tail_budget: u64,
    sizer: &dyn ContextSizer,
) -> Result<Option<usize>, HarnessError> {
    let mut first_unknown = 0;
    let mut first_fitting = last_fitting + 1;
    while first_unknown < first_fitting {
        let position = first_unknown + (first_fitting - first_unknown) / 2;
        let tail = tail_request(source, active_operation_id, cuts[position])?;
        if sizer.estimate_input_tokens(&tail) <= tail_budget {
            first_fitting = position;
        } else {
            first_unknown = position + 1;
        }
    }
    Ok((first_fitting <= last_fitting).then_some(first_fitting))
}

fn summary_at(source: &CompactionSource, cut_index: usize) -> Result<ModelRequest, HarnessError> {
    summary_request(
        source
            .checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.summary.as_str()),
        &source.entries[..=cut_index],
    )
}

fn safe_cut_indices(
    entries: &[ContextEntry],
    active_operation_id: OperationId,
) -> Result<Vec<usize>, HarnessError> {
    let mut cuts = Vec::new();
    let mut operation = None;
    let mut pending = HashMap::<String, String>::new();
    for (index, entry) in entries.iter().enumerate() {
        if operation != Some(entry.operation_id) {
            if !pending.is_empty() {
                return Err(HarnessError::Corrupt(
                    "conversation changes operation inside an unresolved tool group".to_owned(),
                ));
            }
            operation = Some(entry.operation_id);
        }
        let completed_tool_group = update_tool_group(&entry.message, &mut pending)?;
        let ends_operation = entries
            .get(index + 1)
            .is_none_or(|next| next.operation_id != entry.operation_id);
        if ends_operation && !pending.is_empty() {
            return Err(HarnessError::Corrupt(
                "conversation operation ends with unresolved tool calls".to_owned(),
            ));
        }
        if (entry.operation_id != active_operation_id && ends_operation)
            || (entry.operation_id == active_operation_id && completed_tool_group)
        {
            cuts.push(index);
        }
    }
    Ok(cuts)
}

fn update_tool_group(
    message: &Message,
    pending: &mut HashMap<String, String>,
) -> Result<bool, HarnessError> {
    match message {
        Message::User { .. } => {
            if !pending.is_empty() {
                return Err(HarnessError::Corrupt(
                    "user message appears inside an unresolved tool group".to_owned(),
                ));
            }
        }
        Message::Assistant { content, .. } => {
            if !pending.is_empty() {
                return Err(HarnessError::Corrupt(
                    "assistant message appears before all tool results".to_owned(),
                ));
            }
            for block in content {
                if let AssistantContent::ToolCall { call } = block
                    && pending.insert(call.id.clone(), call.name.clone()).is_some()
                {
                    return Err(HarnessError::Corrupt(
                        "assistant tool-call identifiers are duplicated".to_owned(),
                    ));
                }
            }
        }
        Message::Tool { result } => {
            let expected = pending.remove(&result.call_id).ok_or_else(|| {
                HarnessError::Corrupt("tool result has no pending call".to_owned())
            })?;
            if expected != result.name {
                return Err(HarnessError::Corrupt(
                    "tool result name does not match its call".to_owned(),
                ));
            }
            return Ok(pending.is_empty());
        }
    }
    Ok(false)
}

fn tail_request(
    source: &CompactionSource,
    active_operation_id: OperationId,
    cut_index: usize,
) -> Result<ModelRequest, HarnessError> {
    let covered = source.entries[cut_index].sequence;
    let mut messages = Vec::new();
    let anchor = &source.active_user_anchor;
    if anchor.operation_id != active_operation_id {
        return Err(HarnessError::Corrupt(
            "compaction source has the wrong active user anchor".to_owned(),
        ));
    }
    if !matches!(anchor.message, Message::User { .. }) {
        return Err(HarnessError::Corrupt(
            "active operation does not start with a user message".to_owned(),
        ));
    }
    if anchor.sequence <= covered {
        messages.push(anchor.message.clone());
    }
    messages.extend(
        source.entries[cut_index + 1..]
            .iter()
            .map(|entry| entry.message.clone()),
    );
    Ok(ModelRequest {
        system_prompt: source.progress.runtime.system_prompt.clone(),
        messages,
        tools: source
            .progress
            .runtime
            .tools
            .iter()
            .map(|tool| tool.spec.clone())
            .collect(),
    })
}
