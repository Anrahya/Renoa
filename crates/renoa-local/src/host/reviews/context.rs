use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::{
    GitHubReviewError,
    github::{GitHub, Pull},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewFile {
    pub filename: String,
    pub status: String,
    pub previous_filename: Option<String>,
    /// Historical API snapshots retain patches. New snapshots use Git objects.
    pub patch: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewSource {
    #[default]
    ApiSnapshot,
    GitCommits,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewContext {
    /// GitHub's PR diff is relative to the common ancestor, not the current base tip.
    pub merge_base_sha: String,
    pub title: String,
    pub description: Option<String>,
    pub files: Vec<ReviewFile>,
    pub head_paths: Vec<String>,
    pub base_instructions: BTreeMap<String, String>,
    pub checks: Vec<ReviewCheck>,
    pub limitations: Vec<String>,
    #[serde(default)]
    pub source: ReviewSource,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewCheck {
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
}

pub(super) async fn gather(
    github: &GitHub,
    pull: &Pull,
    cancel: &CancellationToken,
) -> Result<ReviewContext, GitHubReviewError> {
    let mut context = ReviewContext {
        merge_base_sha: github.merge_base(pull, cancel).await?,
        title: pull.title.clone(),
        description: pull.body.clone(),
        files: Vec::new(),
        head_paths: Vec::new(),
        base_instructions: BTreeMap::new(),
        checks: Vec::new(),
        source: ReviewSource::GitCommits,
        limitations: vec![
            "Tests were not executed by this reviewer; CI status is observed evidence only."
                .to_owned(),
            "CI context includes check runs, not legacy commit statuses or full logs.".to_owned(),
        ],
    };
    context.load_checks(github, &pull.head.sha, cancel).await?;
    let current = github.pull(pull.number, cancel).await?;
    if current.base != pull.base
        || current.head != pull.head
        || current.state != pull.state
        || current.draft != pull.draft
    {
        return Err(GitHubReviewError::MovingPull);
    }
    Ok(context)
}

impl ReviewContext {
    pub(super) async fn load_inventory(
        &mut self,
        root: &Path,
        head: &str,
        cancel: &CancellationToken,
    ) -> Result<(), GitHubReviewError> {
        let root = root.to_owned();
        let repository =
            tokio::task::spawn_blocking(move || crate::git_repository::GitRepository::open(&root))
                .await
                .map_err(|error| GitHubReviewError::Invalid(error.to_string()))??;
        self.files = repository
            .changes(&self.merge_base_sha, head, cancel)
            .await?
            .into_iter()
            .map(|change| ReviewFile {
                filename: change.path,
                previous_filename: change.previous_path,
                status: change.status,
                patch: None,
            })
            .collect();
        Ok(())
    }

    pub(super) fn prompt(&self) -> Result<serde_json::Value, GitHubReviewError> {
        if self.source == ReviewSource::ApiSnapshot {
            return Ok(serde_json::to_value(self)?);
        }
        // The full inventory is durable, but not repeated in every model input.
        // The shared Git tools page through the very same immutable commit pair.
        Ok(
            serde_json::json!({"title":self.title,"description":self.description,
            "merge_base_sha":self.merge_base_sha,"changed_files":self.files.len(),
            "checks":self.checks,"limitations":self.limitations,
            "instructions":"Start by reading AGENTS.md at base_sha with git_show. Check applicable ancestor AGENTS.md files at base_sha as you inspect changed paths. Missing instruction files are normal. Use git_changes with merge_base_sha and head_sha, follow every inventory page, and use git_diff/git_show to investigate the relevant changes. Source and diff pages remain accessible after compaction."}),
        )
    }

    pub(super) fn inventory_limitation<'a>(
        &self,
        head: &str,
        results: impl Iterator<Item = &'a renoa_agent::ToolResult>,
    ) -> Option<String> {
        if self.source != ReviewSource::GitCommits {
            return None;
        }
        let mut retrieved = BTreeSet::new();
        for result in results.filter(|r| r.name == "git_changes" && !r.is_error) {
            for content in &result.content {
                let renoa_agent::ContentBlock::Text { text } = content else {
                    continue;
                };
                let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
                    continue;
                };
                if value["base"] != self.merge_base_sha || value["head"] != head {
                    continue;
                }
                if let Some(changes) = value["changes"].as_array() {
                    retrieved.extend(
                        changes
                            .iter()
                            .filter_map(|c| c["path"].as_str().map(str::to_owned)),
                    );
                }
            }
        }
        let seen = self
            .files
            .iter()
            .filter(|f| retrieved.contains(&f.filename))
            .count();
        (seen < self.files.len()).then(|| format!(
            "The reviewer retrieved {seen} of {} changed paths from the pinned Git inventory. Coverage is partial; unlisted changes may not have been considered. Retrieval alone does not prove a file was reviewed.", self.files.len()
        ))
    }

    async fn load_checks(
        &mut self,
        github: &GitHub,
        sha: &str,
        cancel: &CancellationToken,
    ) -> Result<(), GitHubReviewError> {
        #[derive(Deserialize)]
        struct Checks {
            total_count: usize,
            check_runs: Vec<ReviewCheck>,
        }
        let mut page = 1_u64;
        loop {
            let page_string = page.to_string();
            match github
                .repo_json::<Checks>(
                    &["commits", sha, "check-runs"],
                    &[("per_page", "100"), ("page", &page_string)],
                    cancel,
                )
                .await
            {
                Ok(checks) => {
                    let count = checks.check_runs.len();
                    self.checks.extend(checks.check_runs);
                    if self.checks.len() >= checks.total_count {
                        break;
                    }
                    if count == 0 {
                        self.limitations.push("CI check list changed during pagination; collected checks are partial.".to_owned());
                        break;
                    }
                }
                Err(GitHubReviewError::Api {
                    status: 403 | 404, ..
                }) => {
                    self.limitations
                        .push("CI checks were inaccessible.".to_owned());
                    break;
                }
                Err(error) => return Err(error),
            }
            page = page.checked_add(1).ok_or(GitHubReviewError::ContextLimit)?;
        }
        Ok(())
    }

    /// Historical API-shaped fixtures exercise model, admission and recovery
    /// boundaries. Git-backed execution is covered separately with real commits.
    #[cfg(test)]
    pub(super) async fn fixture_inventory(
        &mut self,
        github: &GitHub,
        pull: &Pull,
        cancel: &CancellationToken,
    ) -> Result<(), GitHubReviewError> {
        self.source = ReviewSource::ApiSnapshot;
        self.files = github
            .repo_json(
                &["pulls", &pull.number.to_string(), "files"],
                &[("per_page", "100")],
                cancel,
            )
            .await?;
        self.head_paths = self.files.iter().map(|f| f.filename.clone()).collect();
        self.base_instructions.insert(
            "AGENTS.md".to_owned(),
            github.source("AGENTS.md", &pull.base.sha, cancel).await?,
        );
        Ok(())
    }
}
