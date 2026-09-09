use super::*;
use std::{fs, process::Command};

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
        ])
        .args(args)
        .output()
        .expect("Git fixture");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("Git text")
        .trim()
        .to_owned()
}

pub(crate) fn fixture() -> (tempfile::TempDir, GitRepository, String, String) {
    let dir = tempfile::tempdir().expect("repository");
    git(dir.path(), &["init", "-q"]);
    fs::write(dir.path().join("old.rs"), "fn safe() {}\n").expect("old source");
    fs::write(
        dir.path().join("removed.rs"),
        "check_owner();\noperate();\n",
    )
    .expect("deletion");
    fs::write(
        dir.path().join("caller.rs"),
        format!(
            "let count = 1;\n{}\nratio(count);\n",
            "// unchanged\n".repeat(30)
        ),
    )
    .expect("caller");
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-qm", "base"]);
    let base = git(dir.path(), &["rev-parse", "HEAD"]);
    fs::rename(dir.path().join("old.rs"), dir.path().join("renamed.rs")).expect("rename");
    fs::remove_file(dir.path().join("removed.rs")).expect("delete");
    fs::write(
        dir.path().join("caller.rs"),
        format!(
            "let count = 0;\n{}\nratio(count);\n",
            "// unchanged\n".repeat(30)
        ),
    )
    .expect("changed caller");
    for n in 0..600 {
        fs::write(
            dir.path()
                .join(format!("f{n:04}-{}.txt", "long-name-".repeat(8))),
            "content\n",
        )
        .expect("source");
    }
    fs::write(dir.path().join("huge.txt"), "aλ".repeat(300_000)).expect("long line");
    fs::write(
        dir.path().join("z-bug.rs"),
        "fn ratio(count: u32) -> u32 {\n    10 / count\n}\n",
    )
    .expect("late defect");
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-qm", "head"]);
    let head = git(dir.path(), &["rev-parse", "HEAD"]);
    let repo = GitRepository::open(dir.path()).expect("repository");
    (dir, repo, base, head)
}

#[tokio::test]
async fn complete_inventory_and_evidence_survive_old_review_limits() {
    let (_dir, repo, base, head) = fixture();
    let cancel = CancellationToken::new();
    let changes = repo
        .changes(&base, &head, &cancel)
        .await
        .expect("all changes");
    assert_eq!(changes.len(), 605);
    assert!(
        changes
            .iter()
            .any(|c| c.previous_path.as_deref() == Some("old.rs") && c.path == "renamed.rs")
    );
    assert!(
        changes
            .iter()
            .any(|c| c.path == "removed.rs" && c.status == "D")
    );
    assert!(
        repo.in_diff(&base, &head, "z-bug.rs", GitSide::Head, 2, &cancel)
            .await
            .expect("anchor")
    );
    assert!(
        repo.matches(&head, "z-bug.rs", 2, "    10 / count", &cancel)
            .await
            .expect("evidence")
    );
    assert!(
        repo.in_diff(&base, &head, "removed.rs", GitSide::Base, 1, &cancel)
            .await
            .expect("deleted anchor")
    );
    assert!(
        repo.matches(&base, "removed.rs", 1, "check_owner();", &cancel)
            .await
            .expect("deleted evidence")
    );
    assert!(
        !repo
            .matches(&head, "z-bug.rs", 2, "    invented", &cancel)
            .await
            .expect("false quote")
    );
}

#[tokio::test]
async fn byte_pages_reconstruct_large_utf8_blobs_and_diffs_without_loss() {
    let (_dir, repo, base, head) = fixture();
    let cancel = CancellationToken::new();
    for diff in [false, true] {
        let mut offset = 0;
        let mut result = String::new();
        loop {
            let page = if diff {
                repo.diff(&base, &head, "huge.txt", offset, &cancel).await
            } else {
                repo.show(&head, "huge.txt", offset, &cancel).await
            }
            .expect("page");
            assert_eq!(page.encoding, "utf8");
            result.push_str(&page.content);
            let Some(next) = page.next_offset else {
                break;
            };
            assert!(next > offset);
            offset = next;
        }
        assert!(result.len() > 512 * 1024);
        if diff {
            assert!(result.contains(&format!("+{}", "aλ".repeat(300_000))));
        } else {
            assert_eq!(result, "aλ".repeat(300_000));
        }
    }
}

#[tokio::test]
async fn inspection_uses_immutable_objects_and_rejects_path_or_revision_injection() {
    let (dir, repo, base, head) = fixture();
    let cancel = CancellationToken::new();
    fs::write(dir.path().join("z-bug.rs"), "worktree changed").expect("uncommitted changes");
    assert!(
        repo.matches(&head, "z-bug.rs", 2, "    10 / count", &cancel)
            .await
            .expect("pinned evidence")
    );
    assert!(repo.show("--help", "z-bug.rs", 0, &cancel).await.is_err());
    assert!(repo.show(&head, "../config", 0, &cancel).await.is_err());
    assert!(
        repo.diff(&base, &head, ":(glob)*", 0, &cancel)
            .await
            .expect("literal path")
            .content
            .is_empty()
    );
    cancel.cancel();
    assert_eq!(
        repo.changes(&base, &head, &cancel)
            .await
            .expect_err("cancelled")
            .kind(),
        io::ErrorKind::Interrupted
    );
}

#[tokio::test]
async fn quotations_normalize_terminators_without_accepting_prefixes_or_missing_lines() {
    let (dir, repo, _, _) = fixture();
    let quote = "long quote λ".repeat(800);
    fs::write(
        dir.path().join("crlf.txt"),
        format!("first\r\n\r\n{quote}\r\nlast"),
    )
    .expect("CRLF source");
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-qm", "CRLF"]);
    let head = git(dir.path(), &["rev-parse", "HEAD"]);
    let cancel = CancellationToken::new();
    for (line, text, matches) in [
        (1, "first\n\n", true),
        (1, "first\r\n\r\n", true),
        (1, "firs", false),
        (3, quote.as_str(), true),
        (4, "last", true),
        (4, "last\n\n", false),
        (5, "\n", false),
    ] {
        assert_eq!(
            repo.matches(&head, "crlf.txt", line, text, &cancel)
                .await
                .expect("quotation"),
            matches
        );
    }
}

#[tokio::test]
async fn ordinary_linked_worktrees_reuse_git_tools_with_verified_metadata() {
    let (dir, _, base, head) = fixture();
    let linked = tempfile::tempdir().expect("linked worktree");
    git(
        dir.path(),
        &[
            "worktree",
            "add",
            "--detach",
            linked.path().to_str().expect("path"),
            &head,
        ],
    );
    let repo = GitRepository::open(linked.path()).expect("registered worktree");
    assert_eq!(
        repo.changes(&base, &head, &CancellationToken::new())
            .await
            .expect("inventory")
            .len(),
        605
    );
    let forged = tempfile::tempdir().expect("unrelated workspace");
    fs::copy(linked.path().join(".git"), forged.path().join(".git")).expect("copied pointer");
    assert!(GitRepository::open(forged.path()).is_err());
}
