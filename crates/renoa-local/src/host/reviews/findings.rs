use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::{GitHubReviewError, GitHubReviewSnapshot, reviewer::ReviewTools};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubReviewReport {
    pub findings: Vec<GitHubReviewFinding>,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubReviewFinding {
    /// Legacy persisted reports have no assigned priority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<ReviewPriority>,
    pub path: String,
    /// One-based source line at the selected immutable comparison side.
    pub line: u32,
    #[serde(default)]
    pub side: crate::GitSide,
    /// Whether the validated source location is in the comparison's diff.
    /// False findings remain publishable in the review body. Historical reports
    /// were validated against added lines and therefore default to true.
    #[serde(default = "historical_in_diff")]
    pub in_diff: bool,
    pub title: String,
    pub trigger: String,
    pub consequence: String,
    pub correction: String,
    pub evidence: GitHubReviewEvidence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ReviewPriority {
    P0,
    P1,
    P2,
    P3,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubReviewEvidence {
    pub path: String,
    pub start_line: u32,
    #[serde(default)]
    pub side: crate::GitSide,
    /// Exact consecutive source lines at the selected immutable side.
    pub quote: String,
}

fn historical_in_diff() -> bool {
    true
}

pub(super) fn parse(output: &str) -> Result<GitHubReviewReport, GitHubReviewError> {
    let report: GitHubReviewReport = serde_json::from_str(output)?;
    if report
        .findings
        .iter()
        .any(|finding| finding.priority.is_none())
    {
        return Err(GitHubReviewError::Invalid(
            "new review findings require a P0–P3 priority".to_owned(),
        ));
    }
    Ok(report)
}

pub(super) async fn validate(
    report: GitHubReviewReport,
    snapshot: &GitHubReviewSnapshot,
    tools: &ReviewTools<'_>,
    cancel: &CancellationToken,
) -> Result<GitHubReviewReport, GitHubReviewError> {
    match tools.source {
        super::reviewer::ReviewToolSource::Sandbox(container) => {
            let root = container.checkout().to_owned();
            let repository = tokio::task::spawn_blocking(move || {
                crate::git_repository::GitRepository::open(&root)
            })
            .await
            .map_err(|error| GitHubReviewError::Invalid(error.to_string()))??;
            validate_git(report, snapshot, &repository, cancel).await
        }
        #[cfg(test)]
        super::reviewer::ReviewToolSource::Workspace(workspace) => {
            let repository = crate::git_repository::GitRepository::open(workspace.root())?;
            validate_git(report, snapshot, &repository, cancel).await
        }
        #[cfg(test)]
        super::reviewer::ReviewToolSource::Fixture(_) => {
            validate_fixture(report, snapshot, tools, cancel).await
        }
    }
}

pub(super) async fn validate_git(
    mut report: GitHubReviewReport,
    snapshot: &GitHubReviewSnapshot,
    repository: &crate::git_repository::GitRepository,
    cancel: &CancellationToken,
) -> Result<GitHubReviewReport, GitHubReviewError> {
    let changes = repository
        .changes(&snapshot.context.merge_base_sha, &snapshot.head_sha, cancel)
        .await?;
    let mut accepted = Vec::new();
    let mut anchors = BTreeSet::new();
    for mut finding in report.findings {
        let is_changed_path = changes.iter().any(|change| {
            change.path == finding.path || change.previous_path.as_ref() == Some(&finding.path)
        });
        let commit = match finding.side {
            crate::GitSide::Base => &snapshot.context.merge_base_sha,
            crate::GitSide::Head => &snapshot.head_sha,
        };
        let evidence_commit = match finding.evidence.side {
            crate::GitSide::Base => &snapshot.context.merge_base_sha,
            crate::GitSide::Head => &snapshot.head_sha,
        };
        if !is_changed_path
            || crate::git_repository::relative(&finding.evidence.path).is_err()
            || [
                &finding.title,
                &finding.trigger,
                &finding.consequence,
                &finding.correction,
                &finding.evidence.quote,
            ]
            .iter()
            .any(|s| s.trim().is_empty())
            || !repository
                .has_line(commit, &finding.path, finding.line, cancel)
                .await?
            || !repository
                .matches(
                    evidence_commit,
                    &finding.evidence.path,
                    finding.evidence.start_line,
                    &finding.evidence.quote,
                    cancel,
                )
                .await?
        {
            report.limitations.push("Rejected a candidate whose source location or evidence could not be verified against the pinned commits.".to_owned());
            continue;
        }
        finding.in_diff = repository
            .in_diff(
                &snapshot.context.merge_base_sha,
                &snapshot.head_sha,
                &finding.path,
                finding.side,
                finding.line,
                cancel,
            )
            .await?;
        if changes
            .iter()
            .any(|change| change.previous_path.as_ref() == Some(&finding.path))
        {
            // GitHub addresses renamed files by their new path. Preserve a base
            // source location in the body instead of posting it to the wrong file.
            finding.in_diff = false;
        }
        if anchors.insert((finding.path.clone(), finding.side, finding.line)) {
            accepted.push(finding);
        }
    }
    accepted.sort_by_key(|finding| finding.priority);
    report.findings = accepted;
    report
        .limitations
        .extend(snapshot.context.limitations.iter().cloned());
    report.limitations.sort();
    report.limitations.dedup();
    Ok(report)
}

#[cfg(test)]
async fn validate_fixture(
    mut report: GitHubReviewReport,
    snapshot: &GitHubReviewSnapshot,
    tools: &ReviewTools<'_>,
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
        .all(|value| !value.trim().is_empty());
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
        if !matches_evidence(&finding.evidence, tools, cancel).await? {
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
    accepted.sort_by_key(|finding| finding.priority);
    report.findings = accepted;
    report
        .limitations
        .extend(snapshot.context.limitations.iter().cloned());
    report.limitations.sort();
    report.limitations.dedup();
    Ok(report)
}

#[cfg(test)]
async fn matches_evidence(
    evidence: &GitHubReviewEvidence,
    tools: &ReviewTools<'_>,
    cancel: &CancellationToken,
) -> Result<bool, GitHubReviewError> {
    let quote: Vec<_> = evidence.quote.lines().collect();
    let start =
        usize::try_from(evidence.start_line - 1).map_err(|_| GitHubReviewError::ContextLimit)?;
    let matches = match tools.source {
        super::reviewer::ReviewToolSource::Workspace(_) => {
            return Err(GitHubReviewError::Invalid(
                "Git workspace uses object evidence validation".to_owned(),
            ));
        }
        super::reviewer::ReviewToolSource::Sandbox(container) => {
            let root = container.checkout().join("head");
            let path = crate::workspace::existing_file(&root, &evidence.path)
                .await
                .map_err(|error| GitHubReviewError::Invalid(error.to_string()))?;
            let expected = evidence.quote.clone();
            let cancellation = cancel.clone();
            tokio::task::spawn_blocking(move || {
                matches_text_evidence(&path, start, &expected, &cancellation)
            })
            .await
            .map_err(|error| GitHubReviewError::Invalid(error.to_string()))??
        }
        #[cfg(test)]
        super::reviewer::ReviewToolSource::Fixture(snapshot) => {
            tools
                .github
                .source(&evidence.path, &snapshot.head_sha, cancel)
                .await?
                .lines()
                .skip(start)
                .take(quote.len())
                .collect::<Vec<_>>()
                == quote
        }
    };

    Ok(!quote.is_empty() && matches)
}

#[cfg(test)]
fn matches_text_evidence(
    path: &std::path::Path,
    start: usize,
    expected: &str,
    cancel: &CancellationToken,
) -> std::io::Result<bool> {
    use std::io::{self, BufRead as _};
    let mut lines = io::BufReader::new(std::fs::File::open(path)?).lines();
    let mut next_line = || {
        if cancel.is_cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "evidence lookup cancelled",
            ));
        }
        match lines.next().transpose() {
            // A binary line cannot substantiate a textual quotation. Reject this
            // candidate without aborting validation of the rest of the report.
            Err(error) if error.kind() == io::ErrorKind::InvalidData => Ok(None),
            result => result,
        }
    };
    for _ in 0..start {
        if next_line()?.is_none() {
            return Ok(false);
        }
    }
    for expected in expected.lines() {
        if next_line()?.as_deref() != Some(expected) {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
#[test]
fn binary_evidence_is_rejected_without_hiding_io_or_cancellation_failures() {
    let file = tempfile::NamedTempFile::new().expect("source");
    let cancel = CancellationToken::new();
    std::fs::write(file.path(), b"first\n\xff\nwanted\n").expect("binary source");
    for start in [1, 2] {
        assert!(
            !matches_text_evidence(file.path(), start, "wanted", &cancel)
                .expect("reject binary evidence")
        );
    }
    std::fs::write(file.path(), b"first\nwanted\n").expect("text source");
    assert!(matches_text_evidence(file.path(), 1, "wanted", &cancel).expect("accept exact text"));
    cancel.cancel();
    assert_eq!(
        matches_text_evidence(file.path(), 1, "wanted", &cancel)
            .expect_err("cancel read")
            .kind(),
        std::io::ErrorKind::Interrupted
    );
    let missing = file.path().with_extension("missing");
    assert_eq!(
        matches_text_evidence(&missing, 1, "wanted", &CancellationToken::new())
            .expect_err("missing source remains an IO error")
            .kind(),
        std::io::ErrorKind::NotFound
    );
}

#[cfg(test)]
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
