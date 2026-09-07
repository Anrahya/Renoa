use std::collections::BTreeSet;

use renoa_agent::{Message, TokenUsage};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::{GitHubReviewError, GitHubReviewSnapshot, github::GitHub};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubReviewReport {
    pub findings: Vec<GitHubReviewFinding>,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubReviewFinding {
    pub path: String,
    /// RIGHT-side line in the pinned head; must be an added diff line.
    pub line: u32,
    pub title: String,
    pub trigger: String,
    pub consequence: String,
    pub correction: String,
    pub evidence: GitHubReviewEvidence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubReviewEvidence {
    pub path: String,
    pub start_line: u32,
    /// Exact consecutive source lines at the reviewed head.
    pub quote: String,
}

pub(super) fn parse(output: &str) -> Result<GitHubReviewReport, GitHubReviewError> {
    if output.len() > 64 * 1024 {
        return Err(GitHubReviewError::ContextLimit);
    }
    let report: GitHubReviewReport = serde_json::from_str(output)?;
    if report.findings.len() > 20 || report.limitations.len() > 50 {
        return Err(GitHubReviewError::Invalid(
            "review result exceeds finding/limitation limit".to_owned(),
        ));
    }
    Ok(report)
}

pub(super) async fn validate(
    mut report: GitHubReviewReport,
    snapshot: &GitHubReviewSnapshot,
    github: &GitHub,
    cancel: &CancellationToken,
) -> Result<GitHubReviewReport, GitHubReviewError> {
    let mut accepted = Vec::new();
    let mut anchors = BTreeSet::new();
    for finding in report.findings {
        let anchored = snapshot
            .context
            .files
            .iter()
            .find(|file| file.filename == finding.path)
            .and_then(|file| file.patch.as_deref())
            .is_some_and(|patch| added_lines(patch).contains(&finding.line));
        let bounded = [
            &finding.title,
            &finding.trigger,
            &finding.consequence,
            &finding.correction,
            &finding.evidence.quote,
        ]
        .iter()
        .all(|value| !value.trim().is_empty() && value.len() <= 4096);
        if !anchored
            || !bounded
            || finding.evidence.start_line == 0
            || !snapshot.context.head_paths.contains(&finding.evidence.path)
        {
            report.limitations.push(
                "Rejected a candidate with invalid anchor, source path or required evidence."
                    .to_owned(),
            );
            continue;
        }
        let source = github
            .source(&finding.evidence.path, &snapshot.head_sha, cancel)
            .await?;
        let quote: Vec<_> = finding.evidence.quote.lines().collect();
        let start = usize::try_from(finding.evidence.start_line - 1)
            .map_err(|_| GitHubReviewError::ContextLimit)?;
        if quote.is_empty()
            || source
                .lines()
                .skip(start)
                .take(quote.len())
                .collect::<Vec<_>>()
                != quote
        {
            report.limitations.push(
                "Rejected a candidate whose quoted evidence did not match the pinned source."
                    .to_owned(),
            );
            continue;
        }
        if anchors.insert((finding.path.clone(), finding.line)) {
            accepted.push(finding);
        } else {
            report
                .limitations
                .push("Removed a duplicate finding at the same source anchor.".to_owned());
        }
    }
    report.findings = accepted;
    report
        .limitations
        .extend(snapshot.context.limitations.iter().cloned());
    report.limitations.sort();
    report.limitations.dedup();
    Ok(report)
}

fn added_lines(patch: &str) -> BTreeSet<u32> {
    let mut next = None;
    let mut result = BTreeSet::new();
    for line in patch.lines() {
        if line.starts_with("@@ ") {
            next = line
                .split_whitespace()
                .nth(2)
                .and_then(|value| value.strip_prefix('+'))
                .and_then(|value| value.split(',').next())
                .and_then(|value| value.parse::<u32>().ok());
        } else if let Some(number) = next {
            if line.starts_with('+') {
                result.insert(number);
            }
            if line.starts_with(['+', ' ']) {
                next = number.checked_add(1);
            }
        }
    }
    result
}

pub(super) fn usage(history: &[crate::LocalHistoryEntry]) -> Option<TokenUsage> {
    let mut total = TokenUsage::default();
    let mut observed = false;
    for entry in history {
        if let Message::Assistant { usage, .. } = &entry.message {
            let usage = (*usage)?;
            observed = true;
            total.input = total.input.checked_add(usage.input)?;
            total.output = total.output.checked_add(usage.output)?;
            total.cache_read = total.cache_read.checked_add(usage.cache_read)?;
            total.cache_write = total.cache_write.checked_add(usage.cache_write)?;
        }
    }
    observed.then_some(total)
}
