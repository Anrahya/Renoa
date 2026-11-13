//! Source adapters capture an immutable tree before the library publishes it.

use std::{
    fs,
    path::{Path, PathBuf},
};

use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use super::{
    CapturedPlugin, PluginError, PluginSourceReceipt, RemoteMcpSource, api::PluginSource,
    generated::GeneratedMcpPlugin, inspect,
};

mod discovery;
mod github;
mod skills_sh;
pub(crate) use github::GithubSourceClient;

struct GithubIntake {
    repository: String,
    commit: Option<String>,
    r#ref: Option<String>,
    path: Option<String>,
    skill: Option<String>,
}

impl super::PluginManager {
    pub(crate) async fn capture_source(
        &self,
        source: PluginSource,
        workspace: &Path,
        cancellation: CancellationToken,
    ) -> Result<CapturedPlugin, PluginError> {
        require_active(&cancellation)?;
        let normalized = normalize_remote(source)?;
        let download = match normalized {
            Ok(github) => {
                let resolved_commit = match github.commit {
                    Some(commit) => commit,
                    None => {
                        self.github_source
                            .resolve_commit(
                                &github.repository,
                                github.r#ref.as_deref(),
                                cancellation.clone(),
                            )
                            .await?
                    }
                };
                let staging = self
                    .github_source
                    .download(
                        &github.repository,
                        &resolved_commit,
                        github.path.as_deref(),
                        cancellation.clone(),
                    )
                    .await?;
                Ok((
                    github.repository,
                    resolved_commit,
                    github.path,
                    github.skill,
                    staging,
                ))
            }
            Err(local_or_mcp) => Err(local_or_mcp),
        };
        let workspace = workspace.to_path_buf();
        tokio::task::spawn_blocking(move || {
            require_active(&cancellation)?;
            let captured = match download {
                Ok((repository, commit, path, skill, staging)) => capture_github_staged(
                    repository,
                    commit,
                    path.as_deref(),
                    skill.as_deref(),
                    &staging,
                )?,
                Err(local_or_mcp) => capture_local_or_mcp(local_or_mcp, &workspace)?,
            };
            require_active(&cancellation)?;
            Ok(captured)
        })
        .await?
    }
}

fn capture_github_staged(
    repository: String,
    commit: String,
    path: Option<&str>,
    skill: Option<&str>,
    staging: &TempDir,
) -> Result<CapturedPlugin, PluginError> {
    let root = staging.path().join("source");
    let captured = match discovery::select(&root, &repository, &commit, path, skill)? {
        discovery::SelectedPackage::Plugin {
            root,
            path: resolved,
        } => {
            let mut captured = inspect::inspect(&root)?;
            captured.inspection.source = Some(PluginSource::Github {
                repository,
                commit: Some(commit),
                r#ref: None,
                path: resolved,
                skill: None,
            });
            captured
        }
        discovery::SelectedPackage::Skill {
            root,
            path: resolved,
        } => {
            let provenance = github_provenance(&repository, &commit, resolved.as_deref());
            let mut captured = capture_skill(&root, Some(provenance))?;
            captured.inspection.source = Some(PluginSource::Github {
                repository,
                commit: Some(commit),
                r#ref: None,
                path: resolved,
                skill: None,
            });
            captured
        }
    };
    Ok(captured)
}

fn capture_local_or_mcp(
    source: PluginSource,
    workspace: &Path,
) -> Result<CapturedPlugin, PluginError> {
    match source {
        PluginSource::Package { source_path } => {
            inspect::inspect(&resolve_path(workspace, source_path)?)
        }
        PluginSource::Skill { source_path } => {
            capture_skill(&resolve_path(workspace, source_path)?, None)
        }
        PluginSource::Mcp {
            name,
            description,
            server,
            endpoint,
            documentation,
            headers,
        } => {
            let generated = GeneratedMcpPlugin::from_researched(RemoteMcpSource::new(
                name,
                description,
                server,
                endpoint,
                documentation,
                headers
                    .into_iter()
                    .map(|header| (header.name, header.value))
                    .collect(),
            ))?;
            let staging = scratch()?;
            generated.write(staging.path())?;
            inspect::inspect(staging.path())
        }
        PluginSource::Installed { .. } => Err(PluginError::Invalid(
            "installed sources must be resolved through the library".to_owned(),
        )),
        PluginSource::Github { .. } | PluginSource::SkillsSh { .. } => {
            unreachable!("remote sources were normalized before blocking capture")
        }
    }
}

pub(super) fn receipt(source: &PluginSource) -> PluginSourceReceipt {
    match source {
        PluginSource::Package { .. } => PluginSourceReceipt::Package,
        PluginSource::Skill { .. } => PluginSourceReceipt::Skill,
        PluginSource::Github { .. } | PluginSource::SkillsSh { .. } => PluginSourceReceipt::Github,
        PluginSource::Mcp { .. } => PluginSourceReceipt::Mcp,
        PluginSource::Installed { .. } => PluginSourceReceipt::Installed,
    }
}

pub(super) fn validate_inspect(source: &PluginSource) -> Result<(), PluginError> {
    match source {
        PluginSource::Package { source_path } | PluginSource::Skill { source_path }
            if source_path.as_os_str().is_empty() =>
        {
            Err(PluginError::Invalid(
                "source_path must not be empty".to_owned(),
            ))
        }
        PluginSource::Github {
            repository,
            commit,
            r#ref,
            path,
            skill,
        } => {
            github::validate_locator(repository, path.as_deref())?;
            if commit.is_some() && r#ref.is_some() {
                return Err(PluginError::Invalid(format!(
                    "specify either commit or ref in inspect, not both; retry inspect with {{\"action\":\"inspect\",\"source\":{{\"kind\":\"github\",\"repository\":{},\"ref\":{}}}}}",
                    serde_json::json!(repository),
                    serde_json::json!(r#ref.as_deref().unwrap_or("main")),
                )));
            }
            if let Some(commit) = commit {
                github::validate_commit(commit)?;
            }
            if let Some(reference) = r#ref {
                github::validate_ref(reference)?;
            }
            if let Some(skill) = skill {
                discovery::validate_skill_selector(skill)?;
            }
            Ok(())
        }
        PluginSource::SkillsSh { url } => skills_sh::parse(url).map(|_| ()),
        _ => Ok(()),
    }
}

pub(super) fn validate_pinned(source: &PluginSource) -> Result<(), PluginError> {
    match source {
        PluginSource::Package { source_path } | PluginSource::Skill { source_path }
            if source_path.as_os_str().is_empty() =>
        {
            Err(PluginError::Invalid(
                "source_path must not be empty".to_owned(),
            ))
        }
        PluginSource::Github {
            repository,
            commit,
            r#ref,
            path,
            skill,
        } => {
            if r#ref.is_some() || skill.is_some() {
                return Err(PluginError::Invalid(format!(
                    "add and install accept only pinned github sources (repository, commit, optional path), not ref or skill; run {{\"action\":\"inspect\",\"source\":{{\"kind\":\"github\",\"repository\":{}{}}}}} and pass the returned source and digest as expected_digest",
                    serde_json::json!(repository),
                    if let Some(reference) = r#ref {
                        format!(",\"ref\":{}", serde_json::json!(reference))
                    } else {
                        String::new()
                    },
                )));
            }
            let Some(commit) = commit.as_deref() else {
                return Err(PluginError::Invalid(format!(
                    "add and install require a resolved 40-character commit SHA; run {{\"action\":\"inspect\",\"source\":{{\"kind\":\"github\",\"repository\":{}}}}} first, then pass the returned pinned source and digest as expected_digest",
                    serde_json::json!(repository),
                )));
            };
            github::validate(repository, commit, path.as_deref()).map(|_| ())
        }
        PluginSource::SkillsSh { url } => Err(PluginError::Invalid(format!(
            "skills_sh sources must be resolved with inspect before add or install; run {{\"action\":\"inspect\",\"source\":{{\"kind\":\"skills_sh\",\"url\":{}}}}} and pass the returned pinned github source and digest as expected_digest",
            serde_json::json!(url),
        ))),
        _ => Ok(()),
    }
}

fn normalize_remote(
    source: PluginSource,
) -> Result<Result<GithubIntake, PluginSource>, PluginError> {
    match source {
        PluginSource::Github {
            repository,
            commit,
            r#ref,
            path,
            skill,
        } => Ok(Ok(GithubIntake {
            repository,
            commit,
            r#ref,
            path,
            skill,
        })),
        PluginSource::SkillsSh { url } => {
            let target = skills_sh::parse(&url)?;
            Ok(Ok(GithubIntake {
                repository: target.repository,
                commit: None,
                r#ref: None,
                path: None,
                skill: Some(target.skill),
            }))
        }
        other => Ok(Err(other)),
    }
}

fn github_provenance(repository: &str, commit: &str, path: Option<&str>) -> String {
    let mut provenance =
        url::Url::parse(repository).expect("GitHub URL was validated before downloading");
    let mut segments = provenance
        .path_segments_mut()
        .expect("GitHub URL has path segments");
    segments.extend(["tree", commit]);
    if let Some(path) = path {
        segments.extend(path.split('/'));
    }
    drop(segments);
    provenance.to_string()
}

fn capture_skill(root: &Path, repository: Option<String>) -> Result<CapturedPlugin, PluginError> {
    let skill = crate::skills::package::capture(root, None)?;
    let staging = scratch()?;
    let mut manifest = serde_json::json!({"$schema":inspect::PLUGIN_SCHEMA, "name":skill.metadata.name, "description":skill.metadata.description});
    if let Some(repository) = repository {
        manifest["repository"] = repository.into();
    }
    write_file(
        &staging.path().join("plugin.json"),
        &serde_json::to_vec(&manifest)?,
    )?;
    let skill_root = staging.path().join("skills").join(&skill.metadata.name);
    for file in skill.files {
        let target = skill_root.join(&file.relative);
        write_file(&target, &file.bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(
                &target,
                fs::Permissions::from_mode(if file.executable { 0o700 } else { 0o600 }),
            )
            .map_err(|source| PluginError::Io {
                action: "set skill file permissions",
                path: target,
                source,
            })?;
        }
    }
    inspect::inspect(staging.path())
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), PluginError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| PluginError::Io {
            action: "create source staging",
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::write(path, bytes).map_err(|source| PluginError::Io {
        action: "write source staging",
        path: path.to_path_buf(),
        source,
    })
}

fn scratch() -> Result<TempDir, PluginError> {
    tempfile::Builder::new()
        .prefix("renoa-plugin-intake-")
        .tempdir()
        .map_err(|source| PluginError::Io {
            action: "create plugin intake staging",
            path: std::env::temp_dir(),
            source,
        })
}

fn resolve_path(workspace: &Path, path: PathBuf) -> Result<PathBuf, PluginError> {
    if path.as_os_str().is_empty() {
        return Err(PluginError::Invalid(
            "source_path must not be empty".to_owned(),
        ));
    }
    Ok(if path.is_absolute() {
        path
    } else {
        workspace.join(path)
    })
}

pub(super) fn require_active(cancellation: &CancellationToken) -> Result<(), PluginError> {
    if cancellation.is_cancelled() {
        Err(PluginError::Cancelled)
    } else {
        Ok(())
    }
}
