use crate::host::reviews::{GitHubReviewError, GitHubReviewSnapshot, github::GitHub};
use renoa_agent::{
    BoxFuture, ContentBlock, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates,
};
use renoa_agent_loop::AgentToolBinding;
use renoa_kernel::EffectRecovery;
use serde::Deserialize;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
pub(crate) fn bindings(github: GitHub, snapshot: &GitHubReviewSnapshot) -> Vec<AgentToolBinding> {
    let tool = SourceTool { github, base: snapshot.base_sha.clone(), merge_base: snapshot.context.merge_base_sha.clone(), head: snapshot.head_sha.clone(), spec: ToolSpec {
        name: "review_source".to_owned(),
        description: "Read up to 200 numbered lines of UTF-8 repository text (files up to 64 KiB) at frozen base tip, merge_base (before the PR changes) or head. Use the head path inventory to locate callers and tests. No network URLs or shell commands.".to_owned(),
        input_schema: serde_json::json!({"type":"object","additionalProperties":false,"required":["path","revision","start_line","line_count"],"properties":{"path":{"type":"string"},"revision":{"enum":["base","merge_base","head"]},"start_line":{"type":"integer","minimum":1},"line_count":{"type":"integer","minimum":1,"maximum":200}}}),
    }};

    vec![AgentToolBinding::new(
        "renoa.review.source/v1",
        Arc::new(tool),
        EffectRecovery::SafeToReplay,
    )]
}
struct SourceTool {
    github: GitHub,
    base: String,
    merge_base: String,
    head: String,
    spec: ToolSpec,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    path: String,
    revision: Revision,
    start_line: usize,
    line_count: usize,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Revision {
    Base,
    MergeBase,
    Head,
}

impl Tool for SourceTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }
    fn execute(
        &self,
        call: ToolCall,
        cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let read: Read = serde_json::from_value(call.arguments)
                .map_err(|error| ToolError::invalid_input(error.to_string()))?;
            if read.start_line == 0 || !(1..=200).contains(&read.line_count) {
                return Err(ToolError::invalid_input(
                    "provide a positive start line and 1..200 lines",
                ));
            }
            let sha = match read.revision {
                Revision::Base => &self.base,
                Revision::MergeBase => &self.merge_base,
                Revision::Head => &self.head,
            };
            let source = self
                .github
                .source(&read.path, sha, &cancellation)
                .await
                .map_err(|error| match error {
                    GitHubReviewError::Cancelled => {
                        ToolError::cancelled("source read cancelled", false)
                    }
                    _ => ToolError::unavailable(error.to_string()),
                })?;
            let lines: Vec<_> = source
                .lines()
                .enumerate()
                .skip(read.start_line - 1)
                .take(read.line_count)
                .map(|(index, line)| format!("{}: {line}", index + 1))
                .collect();
            let output = serde_json::to_string(&serde_json::json!({"path":read.path,"commit":sha,"total_lines":source.lines().count(),"lines":lines})).map_err(|error| ToolError::internal(error.to_string()))?;
            if output.len() > 32 * 1024 {
                return Err(ToolError::output_limit(
                    "source excerpt exceeds 32 KiB; request fewer lines",
                ));
            }
            Ok(ToolOutput {
                content: vec![ContentBlock::text(output)],
                details: None,
                is_error: false,
            })
        })
    }
}
