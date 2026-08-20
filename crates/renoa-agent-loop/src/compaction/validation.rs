use std::num::NonZeroU64;

use renoa_agent::{AssistantContent, ModelRequest, ModelResponse, StopReason, ToolSpec};

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

pub(super) fn summary(
    response: &ModelResponse,
    system_prompt: &str,
    tools: &[ToolSpec],
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
    let footprint = ModelRequest {
        system_prompt: system_prompt.to_owned(),
        messages: vec![checkpoint_message(&summary)],
        tools: tools.to_vec(),
    };
    let estimated = sizer.estimate_input_tokens(&footprint);
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
        AssistantContent, AssistantMetadata, ModelRequest, ModelResponse, StopReason,
    };

    use super::summary;
    use crate::ContextSizer;

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
            "system",
            &[],
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
                "system",
                &[],
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
                "system",
                &[],
                NonZeroU64::new(10).expect("non-zero limit"),
                &FixedSizer(11),
            )
            .expect_err("oversized summary must fail")
            .to_string(),
            "checkpoint alone requires an estimated 11 tokens, above its limit 10"
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
