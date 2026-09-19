use std::fs;

use renoa_kernel::AgentId;
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use super::{AgentDocumentStore, Document, DocumentDefaults};

const DEFAULTS: DocumentDefaults = DocumentDefaults {
    soul: "Be careful.\n",
    user: "Asia/Kolkata.\n",
};

fn both() -> crate::AgentDocuments {
    crate::AgentDocuments {
        soul: true,
        user: true,
    }
}

#[test]
fn publication_creates_private_regular_files_and_is_adoptable() {
    let directory = tempdir().expect("temporary data directory");
    let agent = AgentId::new();
    AgentDocumentStore::publish(directory.path(), agent, both(), DEFAULTS)
        .expect("publish documents");

    let root = directory.path().join("agents").join(agent.to_string());
    for file in ["SOUL.md", "USER.md"] {
        let metadata = fs::symlink_metadata(root.join(file)).expect("published document");
        assert!(metadata.file_type().is_file());
        assert!(!metadata.file_type().is_symlink());
    }

    // A retry adopts the same publication instead of failing.
    AgentDocumentStore::publish(directory.path(), agent, both(), DEFAULTS)
        .expect("adopt matching publication");
}

#[test]
fn conflicting_pre_existing_content_fails_closed() {
    let directory = tempdir().expect("temporary data directory");
    let agent = AgentId::new();
    let root = directory.path().join("agents").join(agent.to_string());
    fs::create_dir_all(&root).expect("create document root");
    fs::write(root.join("SOUL.md"), "operator-written\n").expect("write conflicting document");

    let error = AgentDocumentStore::publish(directory.path(), agent, both(), DEFAULTS)
        .expect_err("conflicting content must fail");
    assert!(
        error
            .to_string()
            .contains("already exists with different content"),
        "unexpected error: {error}"
    );
    assert_eq!(
        fs::read_to_string(root.join("SOUL.md")).expect("read document"),
        "operator-written\n"
    );
    assert!(!root.join("USER.md").exists());
}

#[test]
fn render_includes_only_enabled_documents_and_open_requires_them() {
    let directory = tempdir().expect("temporary data directory");
    let agent = AgentId::new();
    let enabled = crate::AgentDocuments {
        soul: true,
        user: false,
    };
    AgentDocumentStore::publish(directory.path(), agent, enabled, DEFAULTS).expect("publish");

    let store = AgentDocumentStore::open(directory.path(), agent, enabled).expect("open documents");
    let rendered = store.render().expect("render documents");
    assert!(rendered.contains("source=\"SOUL.md\""));
    assert!(!rendered.contains("USER.md"));

    let missing = AgentDocumentStore::open(directory.path(), agent, both())
        .expect_err("an enabled missing document must fail closed");
    assert!(
        missing.to_string().contains("regular file"),
        "unexpected: {missing}"
    );
}

#[test]
fn open_rejects_a_document_root_outside_the_data_directory() {
    let directory = tempdir().expect("temporary data directory");
    let elsewhere = tempdir().expect("temporary escape target");
    let agent = AgentId::new();
    fs::create_dir_all(directory.path().join("agents")).expect("create agents directory");
    std::os::unix::fs::symlink(
        elsewhere.path(),
        directory.path().join("agents").join(agent.to_string()),
    )
    .expect("link escape");

    let error = AgentDocumentStore::open(directory.path(), agent, both())
        .expect_err("an escaping document root must fail closed");
    assert!(error.to_string().contains("outside"), "unexpected: {error}");
}

#[tokio::test]
async fn content_hash_cas_rejects_a_stale_revision_and_accepts_the_current_one() {
    let directory = tempdir().expect("temporary data directory");
    let agent = AgentId::new();
    AgentDocumentStore::publish(directory.path(), agent, both(), DEFAULTS).expect("publish");
    let store = AgentDocumentStore::open(directory.path(), agent, both()).expect("open documents");

    let soul = Document::Soul;
    let stale = store
        .update(
            soul,
            "0".repeat(64).as_str(),
            "new\n",
            &CancellationToken::new(),
        )
        .await
        .expect_err("a stale revision must conflict");
    assert!(
        stale.to_string().contains("changed after this turn began"),
        "unexpected conflict: {stale}"
    );

    let current = store
        .update(
            soul,
            &super::files::revision(DEFAULTS.soul.as_bytes()),
            "new soul\n",
            &CancellationToken::new(),
        )
        .await
        .expect("update with the current revision");
    assert_eq!(current.len(), 64);

    let rendered = store.render().expect("render");
    assert!(rendered.contains("new soul"));
    assert!(rendered.contains("Asia/Kolkata."));
}
