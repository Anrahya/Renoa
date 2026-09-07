use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::{
    GitHubReviewError,
    github::{GitHub, Pull, valid_path},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewFile {
    pub filename: String,
    pub status: String,
    pub previous_filename: Option<String>,
    pub patch: Option<String>,
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
    if pull.changed_files > 500 {
        return Err(GitHubReviewError::ContextLimit);
    }
    let mut context = ReviewContext {
        merge_base_sha: github.merge_base(pull, cancel).await?,
        title: pull.title.clone(),
        description: pull.body.clone(),
        files: Vec::new(),
        head_paths: Vec::new(),
        base_instructions: BTreeMap::new(),
        checks: Vec::new(),
        limitations: vec![
            "Tests were not executed by this reviewer; CI status is observed evidence only."
                .to_owned(),
            "Inline findings support added head lines only; deletion-only defects may need manual review.".to_owned(),
            "CI context includes check runs, not legacy commit statuses or full logs.".to_owned(),
        ],
    };
    for page in 1..=6 {
        let files: Vec<ReviewFile> = github
            .repo_json(
                &["pulls", &pull.number.to_string(), "files"],
                &[("per_page", "100"), ("page", &page.to_string())],
                cancel,
            )
            .await?;
        let count = files.len();
        context.files.extend(files);
        if count < 100 {
            break;
        }
    }
    if context.files.len() != pull.changed_files {
        return Err(GitHubReviewError::MovingPull);
    }
    let mut paths = BTreeSet::new();
    let mut remaining = 256 * 1024_usize;
    for file in &mut context.files {
        valid_path(&file.filename)?;
        if !paths.insert(file.filename.clone()) {
            return Err(GitHubReviewError::MovingPull);
        }
        if let Some(previous) = &file.previous_filename {
            valid_path(previous)?;
        }
        if file
            .patch
            .as_ref()
            .is_some_and(|patch| patch.len() > remaining)
        {
            file.patch = None;
        }
        if let Some(patch) = &file.patch {
            remaining -= patch.len();
        } else {
            context.limitations.push(format!(
                "Diff unavailable or beyond 256 KiB diff budget: {}",
                file.filename
            ));
        }
    }
    context.load_tree(github, &pull.head.sha, cancel).await?;
    context
        .load_instructions(github, &pull.base.sha, &paths, cancel)
        .await?;
    context.load_checks(github, &pull.head.sha, cancel).await?;
    if serde_json::to_vec(&context)?.len() > 512 * 1024 {
        return Err(GitHubReviewError::ContextLimit);
    }
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
    async fn load_tree(
        &mut self,
        github: &GitHub,
        sha: &str,
        cancel: &CancellationToken,
    ) -> Result<(), GitHubReviewError> {
        #[derive(Deserialize)]
        struct Tree {
            tree: Vec<Entry>,
            truncated: bool,
        }
        #[derive(Deserialize)]
        struct Entry {
            path: String,
            mode: String,
            r#type: String,
        }
        let tree: Tree = github
            .repo_json(&["git", "trees", sha], &[("recursive", "1")], cancel)
            .await?;
        if tree.truncated {
            self.limitations
                .push("Head path inventory is truncated by GitHub.".to_owned());
        }
        for entry in tree.tree {
            if entry.r#type == "blob" && matches!(entry.mode.as_str(), "100644" | "100755") {
                valid_path(&entry.path)?;
                self.head_paths.push(entry.path);
            }
        }
        self.head_paths.sort();
        Ok(())
    }

    async fn load_instructions(
        &mut self,
        github: &GitHub,
        sha: &str,
        paths: &BTreeSet<String>,
        cancel: &CancellationToken,
    ) -> Result<(), GitHubReviewError> {
        let mut instructions = BTreeSet::from(["AGENTS.md".to_owned()]);
        for path in paths {
            for (offset, _) in path.match_indices('/') {
                instructions.insert(format!("{}/AGENTS.md", &path[..offset]));
            }
        }
        for (index, path) in instructions.iter().enumerate() {
            if index >= 32 {
                self.limitations.push(
                    "Only the first 32 applicable base AGENTS.md paths were checked.".to_owned(),
                );
                break;
            }
            match github.source(path, sha, cancel).await {
                Ok(text) => {
                    self.base_instructions.insert(path.clone(), text);
                }
                Err(GitHubReviewError::Api { status: 404, .. }) => {}
                Err(GitHubReviewError::ContextLimit) => {
                    self.limitations
                        .push(format!("Base instructions exceed 64 KiB: {path}"));
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
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
        match github
            .repo_json::<Checks>(
                &["commits", sha, "check-runs"],
                &[("per_page", "100")],
                cancel,
            )
            .await
        {
            Ok(checks) => {
                if checks.total_count > checks.check_runs.len() {
                    self.limitations
                        .push("CI check list is partial (first 100).".to_owned());
                }
                self.checks = checks.check_runs;
            }
            Err(GitHubReviewError::Api {
                status: 403 | 404, ..
            }) => self
                .limitations
                .push("CI checks were inaccessible.".to_owned()),
            Err(error) => return Err(error),
        }
        Ok(())
    }
}
