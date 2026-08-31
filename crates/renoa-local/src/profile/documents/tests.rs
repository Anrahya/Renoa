use std::{fs, path::Path};

use renoa_agent::{ToolCall, invoke_tool};
use serde_json::json;
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use super::*;

fn documents(directory: &Path) -> ProfileDocuments {
    ProfileDocuments::initialize(
        directory,
        &AgentProfileId::new("renoa.personal.test.v1").expect("profile id"),
        ProfileDocumentDefaults {
            soul: "Original soul.\n",
            user: "Original user.\n",
        },
    )
    .expect("profile documents")
}

#[test]
fn initialization_does_not_replace_existing_documents() {
    let directory = tempdir().expect("temporary Host data");
    let first = documents(directory.path());
    fs::write(first.path(Document::Soul), "Custom soul.\n").expect("customize soul");

    let second = documents(directory.path());

    assert_eq!(
        fs::read_to_string(second.path(Document::Soul)).expect("read soul"),
        "Custom soul.\n"
    );
}

#[test]
fn initialization_rejects_non_utf8_documents() {
    let directory = tempdir().expect("temporary Host data");
    let first = documents(directory.path());
    fs::write(first.path(Document::User), [0xff]).expect("write invalid user profile");

    let error = ProfileDocuments::initialize(
        directory.path(),
        &AgentProfileId::new("renoa.personal.test.v1").expect("profile id"),
        ProfileDocumentDefaults {
            soul: "Ignored soul.\n",
            user: "Ignored user.\n",
        },
    )
    .expect_err("reject invalid user profile");

    assert!(matches!(
        error,
        AgentProfileError::DocumentInvalidUtf8 { .. }
    ));
}

#[cfg(unix)]
#[test]
fn initialization_rejects_symlinked_documents() {
    use std::os::unix::fs::symlink;

    let directory = tempdir().expect("temporary Host data");
    let first = documents(directory.path());
    let soul = first.path(Document::Soul);
    let external = directory.path().join("external.md");
    fs::write(&external, "External soul.\n").expect("write external soul");
    fs::remove_file(&soul).expect("remove original soul");
    symlink(&external, &soul).expect("link external soul");

    let error = ProfileDocuments::initialize(
        directory.path(),
        &AgentProfileId::new("renoa.personal.test.v1").expect("profile id"),
        ProfileDocumentDefaults {
            soul: "Ignored soul.\n",
            user: "Ignored user.\n",
        },
    )
    .expect_err("reject linked soul");

    assert!(matches!(error, AgentProfileError::DocumentNotFile { .. }));
}

#[tokio::test]
async fn update_requires_the_rendered_revision_and_changes_the_next_render() {
    let directory = tempdir().expect("temporary Host data");
    let documents = documents(directory.path());
    let before = documents.read(Document::User).expect("read user");
    let tool = ProfileUpdateTool::new(
        AgentProfileId::new("renoa.personal.test.v1").expect("profile id"),
        documents.clone(),
    );

    let output = invoke_tool(
        Some(&tool),
        ToolCall {
            id: "update-user".to_owned(),
            name: TOOL_NAME.to_owned(),
            arguments: json!({
                "document": "user",
                "expected_revision": before.revision,
                "content": "Works at night.\n"
            }),
            thought_signature: None,
            namespace: None,
        },
        CancellationToken::new(),
        None,
    )
    .await
    .expect("update user profile");

    assert!(!output.is_error);
    let repeated = documents
        .update(
            Document::User,
            &before.revision,
            "Works at night.\n",
            &CancellationToken::new(),
        )
        .await
        .expect("repeat the same profile update");
    let rendered = documents.render().expect("render updated documents");
    assert!(rendered.contains(&repeated));
    assert!(rendered.contains("Works at night."));
    assert!(!rendered.contains("Original user."));
}

#[tokio::test]
async fn stale_update_preserves_the_newer_document() {
    let directory = tempdir().expect("temporary Host data");
    let documents = documents(directory.path());
    let stale = documents.read(Document::Soul).expect("read soul");
    fs::write(documents.path(Document::Soul), "Newer soul.\n").expect("change soul");

    let error = documents
        .update(
            Document::Soul,
            &stale.revision,
            "Stale replacement.\n",
            &CancellationToken::new(),
        )
        .await
        .expect_err("reject stale update");

    assert!(error.to_string().contains("changed after this turn began"));
    assert_eq!(
        fs::read_to_string(documents.path(Document::Soul)).expect("read soul"),
        "Newer soul.\n"
    );
}
