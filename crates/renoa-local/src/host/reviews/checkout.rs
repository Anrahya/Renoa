use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use super::{GitHubReviewSnapshot, github::GitHub};
use crate::{
    InspectionSandboxConfig, LocalHostError,
    isolated_workspace::{InspectionSandbox, checked_output},
};

pub(super) struct Checkout {
    root: PathBuf,
    pub(super) sandbox: Arc<InspectionSandbox>,
}

impl Checkout {
    pub(super) async fn remove_abandoned(root: &Path) -> io::Result<()> {
        if tokio::fs::try_exists(root).await? {
            remove_tree(root).await?;
        }
        Ok(())
    }
    pub(super) async fn prepare(
        root: PathBuf,
        config: &InspectionSandboxConfig,
        snapshot: &GitHubReviewSnapshot,
        github: &GitHub,
        cancel: &CancellationToken,
    ) -> Result<Self, LocalHostError> {
        if !tokio::fs::try_exists(root.join(".git")).await? {
            let prepared = materialize(
                &root,
                &snapshot.base_sha,
                &snapshot.head_sha,
                &snapshot.context.merge_base_sha,
                github,
                cancel,
            )
            .await;
            if let Err(error) = prepared {
                remove_tree(&root).await?;
                return Err(error.into());
            }
        }
        match InspectionSandbox::start(config, snapshot.request.id, &root, cancel).await {
            Ok(sandbox) => Ok(Self {
                root,
                sandbox: Arc::new(sandbox),
            }),
            Err(error) => {
                remove_tree(&root).await?;
                Err(error.into())
            }
        }
    }

    pub(super) async fn close(&self) -> Result<(), LocalHostError> {
        remove_tree(&self.root).await?;
        Ok(())
    }
}

pub(super) async fn remove_tree(root: &Path) -> io::Result<()> {
    match tokio::fs::remove_dir_all(root).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub(super) async fn materialize(
    root: &Path,
    base: &str,
    head: &str,
    merge_base: &str,
    github: &GitHub,
    cancel: &CancellationToken,
) -> io::Result<()> {
    tokio::fs::create_dir_all(root).await?;
    let git_dir = root.join(".git");
    let mut init = git(&git_dir);
    init.args(["init", "--bare"]);
    checked_output(init, &[], cancel).await?;
    let mut fetch = git(&git_dir);
    // The helper is constant trusted code. Only Git receives the token through
    // its environment; it is never written into the checkout or command line.
    fetch.arg("-c").arg("credential.helper=!f() { echo username=x-access-token; printf 'password=%s\\n' \"$RENOA_REVIEW_GIT_TOKEN\"; }; f")
        .env("RENOA_REVIEW_GIT_TOKEN", github.installation_token()?)
        .args(["fetch", "--no-tags", "--depth=1", &format!("https://github.com/{}.git", github.repository),
            base, head, merge_base]);
    checked_output(fetch, &[], cancel).await?;
    for (name, sha) in [("base", base), ("head", head), ("merge_base", merge_base)] {
        let mut checkout = git(&git_dir);
        checkout
            .args(["worktree", "add", "--detach"])
            .arg(root.join(name))
            .arg(sha);
        checked_output(checkout, &[], cancel).await?;
    }
    let root = root.to_owned();
    let cancellation = cancel.clone();
    tokio::task::spawn_blocking(move || make_readable(&root, &cancellation))
        .await
        .map_err(io::Error::other)??;
    Ok(())
}

// The Host uses a private umask. The unprivileged sandbox must still be able
// to read its mount; do not follow checkout symlinks while adjusting modes.
fn make_readable(root: &Path, cancel: &CancellationToken) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let mut pending = vec![root.to_owned()];
    while let Some(path) = pending.pop() {
        if cancel.is_cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "checkout cancelled",
            ));
        }
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
            for entry in std::fs::read_dir(path)? {
                pending.push(entry?.path());
            }
        } else if metadata.is_file() {
            std::fs::set_permissions(
                path,
                std::fs::Permissions::from_mode(0o644 | (metadata.permissions().mode() & 0o111)),
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[test]
fn checkout_permissions_do_not_follow_links_outside_the_mount() {
    use std::os::unix::fs::PermissionsExt as _;
    let root = tempfile::tempdir().expect("root");
    let outside = tempfile::NamedTempFile::new().expect("private file");
    std::fs::write(root.path().join("source"), "source").expect("source");
    std::os::unix::fs::symlink(outside.path(), root.path().join("link")).expect("link");
    make_readable(root.path(), &CancellationToken::new()).expect("make readable");
    assert_eq!(
        std::fs::metadata(root.path())
            .expect("root mode")
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert_eq!(
        std::fs::metadata(root.path().join("source"))
            .expect("source mode")
            .permissions()
            .mode()
            & 0o777,
        0o644
    );
    assert_eq!(
        std::fs::metadata(outside.path())
            .expect("private mode")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

fn git(directory: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .arg("--git-dir")
        .arg(directory)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "protocol.file.allow=never",
            "-c",
            "credential.helper=",
        ]);
    command
}
