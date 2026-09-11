use std::num::NonZeroU64;

use renoa_agent::{AssistantContent, ModelRequest, ModelResponse, StopReason};

use super::{ContextSizer, checkpoint_message};
use crate::context::CompactionValidationError;

const REQUIRED_HEADINGS: [&str; 7] = [
    "## Goal and user intent",
    "## Hard constraints and preferences",
    "## Completed work",
    "## Current state and blockers",
    "## Decisions and rationale",
    "## Exact working facts",
    "## Next action and unresolved questions",
];

/// Sizes one completed summary response against its exact checkpoint budget.
///
/// The measurement covers the activated checkpoint alone. The post-compaction
/// target already bounds the system prompt and every tool schema through the
/// retained-tail budget, so charging that fixed request overhead here would
/// make the limit unreachable whenever the prompt and tools alone exceed it.
fn checkpoint_footprint(summary: &str) -> ModelRequest {
    ModelRequest {
        system_prompt: String::new(),
        messages: vec![checkpoint_message(summary)],
        tools: Vec::new(),
    }
}

pub(super) fn summary(
    response: &ModelResponse,
    max_summary_tokens: NonZeroU64,
    sizer: &dyn ContextSizer,
) -> Result<String, CompactionValidationError> {
    if response.stop_reason != StopReason::Stop {
        return invalid("compaction response did not stop normally");
    }
    if response
        .content
        .iter()
        .any(|content| matches!(content, AssistantContent::ToolCall { .. }))
    {
        return invalid("compaction response attempted to call a tool");
    }
    let summary = response
        .content
        .iter()
        .filter_map(|content| match content {
            AssistantContent::Text { text, .. } => Some(text.as_str()),
            AssistantContent::Reasoning { .. } | AssistantContent::ToolCall { .. } => None,
        })
        .collect::<String>();
    validate_sections(&summary)?;
    let estimated = sizer.estimate_input_tokens(&checkpoint_footprint(&summary));
    if estimated > max_summary_tokens.get() {
        return invalid(format!(
            "checkpoint alone requires an estimated {estimated} tokens, above its limit {}",
            max_summary_tokens.get()
        ));
    }
    Ok(summary)
}

fn validate_sections(summary: &str) -> Result<(), CompactionValidationError> {
    let summary = summary.trim();
    if summary.is_empty() {
        return invalid("compaction response was empty");
    }
    let mut lines = summary.lines().peekable();
    for heading in REQUIRED_HEADINGS {
        let actual = lines.next().map(str::trim).ok_or_else(|| {
            CompactionValidationError::new(format!("compaction response is missing '{heading}'"))
        })?;
        if actual != heading {
            return invalid(format!(
                "compaction response expected '{heading}', found '{actual}'"
            ));
        }
        let mut has_content = false;
        while lines
            .peek()
            .is_some_and(|line| !line.trim().starts_with("## "))
        {
            if let Some(line) = lines.next() {
                has_content |= !line.trim().is_empty();
            }
        }
        if !has_content {
            return invalid(format!("compaction section '{heading}' is empty"));
        }
    }
    if let Some(extra) = lines.next() {
        return invalid(format!(
            "compaction response contains an unexpected heading '{}'",
            extra.trim()
        ));
    }
    Ok(())
}

fn invalid<T>(message: impl Into<String>) -> Result<T, CompactionValidationError> {
    Err(CompactionValidationError::new(message))
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use renoa_agent::{
        AssistantContent, AssistantMetadata, ModelRequest, ModelResponse, StopReason, ToolSpec,
    };
    use serde_json::json;

    use super::summary;
    use crate::{ContextSizer, compaction::checkpoint_message};

    const VALID: &str = "## Goal and user intent\nContinue the task.\n\
## Hard constraints and preferences\nKeep exact facts.\n\
## Completed work\nRead the repository.\n\
## Current state and blockers\nNo blocker.\n\
## Decisions and rationale\nUse the durable path.\n\
## Exact working facts\nThe file exists.\n\
## Next action and unresolved questions\nRun the tests.";

    #[test]
    fn valid_bounded_summary_is_accepted_exactly() {
        let response = response(VALID, StopReason::Stop);

        let accepted = summary(
            &response,
            NonZeroU64::new(10).expect("non-zero limit"),
            &FixedSizer(10),
        )
        .expect("valid summary");

        assert_eq!(accepted, VALID);
    }

    #[test]
    fn length_stop_and_oversized_checkpoint_are_rejected() {
        let length = response(VALID, StopReason::Length);
        assert_eq!(
            summary(
                &length,
                NonZeroU64::new(10).expect("non-zero limit"),
                &FixedSizer(1),
            )
            .expect_err("length stop must fail")
            .to_string(),
            "compaction response did not stop normally"
        );

        let complete = response(VALID, StopReason::Stop);
        assert_eq!(
            summary(
                &complete,
                NonZeroU64::new(10).expect("non-zero limit"),
                &FixedSizer(11),
            )
            .expect_err("oversized summary must fail")
            .to_string(),
            "checkpoint alone requires an estimated 11 tokens, above its limit 10"
        );
    }

    /// Models the shipped provider estimator's fixed shape: one request frame,
    /// one message and content frame per message, one frame per tool, and
    /// three bytes per estimated token. Fixed request overhead is derived from
    /// the request, so a request carrying neither a prompt nor tools
    /// contributes none of it.
    struct RequestShapeSizer;

    const REQUEST_FRAME_TOKENS: u64 = 64;
    const MESSAGE_FRAME_TOKENS: u64 = 12;
    const CONTENT_FRAME_TOKENS: u64 = 4;
    const TOOL_FRAME_TOKENS: u64 = 24;

    impl ContextSizer for RequestShapeSizer {
        fn estimate_input_tokens(&self, request: &ModelRequest) -> u64 {
            let tools = request
                .tools
                .iter()
                .map(|tool| {
                    TOOL_FRAME_TOKENS
                        .saturating_add(estimated_bytes(tool.name.len()))
                        .saturating_add(estimated_bytes(tool.description.len()))
                        .saturating_add(
                            serde_json::to_vec(&tool.input_schema)
                                .map_or(u64::MAX, |schema| estimated_bytes(schema.len())),
                        )
                })
                .sum::<u64>();
            let messages = request
                .messages
                .iter()
                .map(|message| {
                    MESSAGE_FRAME_TOKENS
                        .saturating_add(CONTENT_FRAME_TOKENS)
                        .saturating_add(
                            serde_json::to_vec(message)
                                .map_or(u64::MAX, |encoded| estimated_bytes(encoded.len())),
                        )
                })
                .sum::<u64>();
            REQUEST_FRAME_TOKENS
                .saturating_add(estimated_bytes(request.system_prompt.len()))
                .saturating_add(tools)
                .saturating_add(messages)
        }
    }

    fn estimated_bytes(length: usize) -> u64 {
        u64::try_from(length).unwrap_or(u64::MAX).div_ceil(3)
    }

    /// Returns the activated request shape of a profile whose system prompt and
    /// tool schemas alone consume ten thousand estimated tokens.
    fn oversized_request_shape() -> (String, Vec<ToolSpec>) {
        let tools = (0..19)
            .map(|index| ToolSpec {
                name: format!("tool_{index}"),
                description: "d".repeat(1_228),
                input_schema: json!({ "type": "object" }),
            })
            .collect::<Vec<_>>();
        ("p".repeat(12_164), tools)
    }

    #[test]
    fn a_summary_within_its_budget_survives_an_unsatisfiable_request_shape() {
        let (system_prompt, tools) = oversized_request_shape();
        let response = response(VALID, StopReason::Stop);

        // Charging that fixed overhead to the checkpoint made every attempt
        // fail, which permanently blocked the conversation at its trigger.
        let charged_to_the_checkpoint = RequestShapeSizer.estimate_input_tokens(&ModelRequest {
            system_prompt: system_prompt.clone(),
            messages: vec![checkpoint_message(VALID)],
            tools: tools.clone(),
        });
        assert!(
            charged_to_the_checkpoint > 10_000,
            "the pre-fix measurement must exceed the checkpoint budget: {charged_to_the_checkpoint}"
        );

        let accepted = summary(
            &response,
            NonZeroU64::new(10_000).expect("non-zero limit"),
            &RequestShapeSizer,
        )
        .expect("a summary within its own budget must be accepted");

        assert_eq!(accepted, VALID);
    }

    #[test]
    fn a_summary_above_its_budget_is_still_rejected_under_the_same_shape() {
        let response = response(VALID, StopReason::Stop);

        let error = summary(
            &response,
            NonZeroU64::new(1).expect("non-zero limit"),
            &RequestShapeSizer,
        )
        .expect_err("an oversized checkpoint must still fail");

        assert!(
            error
                .to_string()
                .starts_with("checkpoint alone requires an estimated "),
            "unexpected message: {error}"
        );
    }

    fn response(text: &str, stop_reason: StopReason) -> ModelResponse {
        ModelResponse {
            content: vec![AssistantContent::text(text)],
            stop_reason,
            usage: None,
            metadata: AssistantMetadata::default(),
        }
    }

    struct FixedSizer(u64);

    impl ContextSizer for FixedSizer {
        fn estimate_input_tokens(&self, _request: &ModelRequest) -> u64 {
            self.0
        }
    }
}
