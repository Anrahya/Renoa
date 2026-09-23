use std::collections::VecDeque;

use renoa_agent::{ToolCall, validate_tool_call_ids};
use renoa_kernel::LoopError;
use serde::{Deserialize, Serialize};

/// The current top-level call and the calls that still follow it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PendingToolCalls {
    current: ToolCall,
    remaining: VecDeque<ToolCall>,
}

impl PendingToolCalls {
    pub(crate) fn new(calls: Vec<ToolCall>) -> Result<Self, LoopError> {
        let mut calls = calls.into_iter();
        let current = calls
            .next()
            .ok_or_else(|| LoopError::new("tool checkpoint has no pending call"))?;
        let pending = Self {
            current,
            remaining: calls.collect(),
        };
        pending.validate()?;
        Ok(pending)
    }

    pub(crate) fn validate(&self) -> Result<(), LoopError> {
        validate_tool_call_ids(self.iter().map(|call| call.id.as_str()))
            .map_err(|error| LoopError::new(format!("tool checkpoint {error}")))
    }

    pub(crate) const fn current(&self) -> &ToolCall {
        &self.current
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &ToolCall> {
        std::iter::once(&self.current).chain(self.remaining.iter())
    }

    pub(crate) fn remaining(&self) -> impl Iterator<Item = &ToolCall> {
        self.remaining.iter()
    }

    pub(crate) fn advance(mut self) -> Option<Self> {
        self.remaining.pop_front().map(|current| Self {
            current,
            remaining: self.remaining,
        })
    }
}
