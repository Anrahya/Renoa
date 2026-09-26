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

mod github;
pub(crate) use github::GithubSourceClient;

impl super::PluginManager {
    pub(crate) async fn capture_source(
        &self,
        source: PluginSource,
        workspace: &Path,
        cancellation: CancellationToken,
    ) -> Result<CapturedPlugin, PluginError> {
        require_active(&cancellation)?;
        let staging = match &source {
            PluginSource::Github {
                repository,
                commit,
                path,
            } => Some(
                self.github_source
                    .download(repository, commit, path.as_deref(), cancellation.clone())
                    .await?,
            ),
            _ => None,
        };
        let workspace = workspace.to_path_buf();
        tokio::task::spawn_blocking(move || {
            require_active(&cancellation)?;
            let captured =
                match source {
                    PluginSource::Package { source_path } => {
                        inspect::inspect(&resolve_path(&workspace, source_path)?)?
                    }
                    PluginSource::Skill { source_path } => {
                        capture_skill(&resolve_path(&workspace, source_path)?, None)?
                    }
                    PluginSource::Github {
                        repository,
                        commit,
                        path,
                    } => {
                        let staging = staging.expect("GitHub source was downloaded");
                        let root = staging.path().join("source");
                        if root.join("plugin.json").try_exists().map_err(|source| {
                            PluginError::Io {
                                action: "inspect GitHub source",
                                path: root.clone(),
                                source,
                            }
                        })? {
                            inspect::inspect(&root)?
                        } else {
                            let mut provenance = url::Url::parse(&repository)
                                .expect("GitHub URL was validated before downloading");
                            let mut segments = provenance
                                .path_segments_mut()
                                .expect("GitHub URL has path segments");
                            segments.extend(["tree", &commit]);
                            if let Some(path) = path.as_deref() {
                                segments.extend(path.split('/'));
                            }
                            drop(segments);
                            let provenance = provenance.to_string();
                            capture_skill(&root, Some(provenance))?
                        }
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
                        inspect::inspect(staging.path())?
                    }
                    PluginSource::Installed { .. } => {
                        return Err(PluginError::Invalid(
                            "installed sources must be resolved through the library".to_owned(),
                        ));
                    }
                };
            require_active(&cancellation)?;
            Ok(captured)
        })
        .await?
    }
}

pub(super) fn receipt(source: &PluginSource) -> PluginSourceReceipt {
    match source {
        PluginSource::Package { .. } => PluginSourceReceipt::Package,
        PluginSource::Skill { .. } => PluginSourceReceipt::Skill,
        PluginSource::Github { .. } => PluginSourceReceipt::Github,
        PluginSource::Mcp { .. } => PluginSourceReceipt::Mcp,
        PluginSource::Installed { .. } => PluginSourceReceipt::Installed,
    }
}

pub(super) fn validate(source: &PluginSource) -> Result<(), PluginError> {
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
            path,
        } => github::validate(repository, commit, path.as_deref()).map(|_| ()),
        _ => Ok(()),
    }
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
