//! `share_file` and `stop_sharing` as a model calls them, against a real
//! catalog and workspace: the link it is given, and what each refusal leaves.

use std::{fs, path::PathBuf};

use renoa_agent::{ContentBlock, ToolCall, ToolResult};
use renoa_agent_loop::AgentToolBinding;
use renoa_kernel::{AgentId, SessionId};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::super::{OwnerTurn, bindings};
use crate::{
    host::catalog,
    shares::{MAX_SHARE_BYTES, ShareError, Shares},
};

struct Fixture {
    _directory: tempfile::TempDir,
    database: PathBuf,
    workspace: PathBuf,
    person: Uuid,
    shares: Shares,
    tools: [AgentToolBinding; 5],
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().expect("directory");
    let root = directory.path().canonicalize().expect("canonical root");
    let home = renoa_home::RenoaHome::at(root.join("home")).expect("home");
    let database = home.host_database();
    fs::create_dir_all(database.parent().expect("state directory")).expect("state directory");
    catalog::initialize(&database).expect("catalog");
    let agent = Uuid::new_v4();
    crate::test_agents::insert_agent(&database, &agent.to_string());
    let workspace = root.join("workspace");
    fs::create_dir(&workspace).expect("workspace");
    let person = Uuid::new_v4();
    let tools = bindings(OwnerTurn {
        database: database.clone(),
        agent: AgentId::from_uuid(agent),
        session: SessionId::new(),
        command: None,
        workspace: workspace.clone(),
        principal: Some(person),
    });
    Fixture {
        shares: Shares::open(&home),
        _directory: directory,
        database,
        workspace,
        person,
        tools,
    }
}

impl Fixture {
    async fn call(&self, name: &str, path: &str) -> ToolResult {
        let binding = self
            .tools
            .iter()
            .find(|binding| binding.tool().spec().name == name)
            .expect("the tool is bound");
        renoa_agent::invoke_tool(
            Some(binding.tool().as_ref()),
            ToolCall {
                id: "call".to_owned(),
                name: name.to_owned(),
                arguments: json!({ "path": path }),
                namespace: None,
                thought_signature: None,
            },
            CancellationToken::new(),
            None,
        )
        .await
        .expect("definite result")
    }

    /// The share `share_file` linked to, after checking the link it gave.
    async fn shared(&self, path: &str, name: &str) -> Uuid {
        let output = self.call(super::SHARE, path).await;
        assert!(!output.is_error, "{output:?}");
        let details = output.details.expect("details");
        let id: Uuid = serde_json::from_value(details["share_id"].clone()).expect("share id");
        assert_eq!(
            output.content,
            vec![ContentBlock::text(format!(
                "Shared {}. Put this link in your reply exactly as it is: [{name}](/s/{id})",
                details["path"].as_str().expect("path")
            ))]
        );
        id
    }

    fn text(output: &ToolResult) -> String {
        match output.content.as_slice() {
            [ContentBlock::Text { text, .. }] => text.clone(),
            other => panic!("one text block, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_shared_file_gets_one_link_its_person_can_open_until_it_is_stopped() {
    let fixture = fixture();
    fs::create_dir(fixture.workspace.join("site")).expect("folder");
    fs::write(fixture.workspace.join("site/index.html"), "<p>hi</p>").expect("file");
    let id = fixture.shared("./site/index.html", "index.html").await;
    assert_eq!(
        fixture.shared("site/index.html", "index.html").await,
        id,
        "sharing again keeps the link"
    );
    let described = fixture
        .shares
        .describe(id, fixture.person)
        .await
        .expect("describe");
    assert_eq!(
        (described.name.as_str(), described.media_type),
        ("index.html", "text/html")
    );
    assert!(matches!(
        fixture.shares.describe(id, Uuid::new_v4()).await,
        Err(ShareError::NotFound)
    ));

    let stopped = fixture.call(super::STOP, "site/index.html").await;
    assert_eq!(
        Fixture::text(&stopped),
        "Stopped sharing site/index.html; its link no longer opens."
    );
    assert!(matches!(
        fixture.shares.describe(id, fixture.person).await,
        Err(ShareError::NotFound)
    ));
    let again = fixture.call(super::STOP, "site/index.html").await;
    assert_eq!(
        Fixture::text(&again),
        "site/index.html was not shared, so nothing changed."
    );
}

/// A file already gone is stopped by any spelling of where it was, through a
/// folder that is a link as well.
#[tokio::test]
async fn a_share_of_a_file_already_gone_can_still_be_stopped() {
    let fixture = fixture();
    fs::create_dir_all(fixture.workspace.join("real/inner")).expect("folders");
    std::os::unix::fs::symlink(
        fixture.workspace.join("real"),
        fixture.workspace.join("linked"),
    )
    .expect("linked folder");
    for (shared, stopped) in [
        ("note.txt", "./note.txt"),
        ("real/inner/a.txt", "real//inner/./a.txt"),
        ("linked/b.txt", "linked/b.txt"),
    ] {
        fs::write(fixture.workspace.join(shared), "hi").expect("file");
        let id = fixture
            .shared(shared, shared.rsplit('/').next().expect("name"))
            .await;
        fs::remove_file(fixture.workspace.join(shared)).expect("remove");
        let output = fixture.call(super::STOP, stopped).await;
        assert_eq!(
            output.details.as_ref().expect("details")["stopped"],
            true,
            "{stopped}: {output:?}"
        );
        fs::write(fixture.workspace.join(shared), "back").expect("file again");
        assert!(
            matches!(
                fixture.shares.describe(id, fixture.person).await,
                Err(ShareError::NotFound)
            ),
            "{stopped}: the stopped link stays closed"
        );
    }
}

#[tokio::test]
async fn a_refused_share_stores_nothing() {
    let fixture = fixture();
    let outside = fixture
        .workspace
        .parent()
        .expect("parent")
        .join("secret.txt");
    fs::write(&outside, "secret").expect("outside");
    std::os::unix::fs::symlink(&outside, fixture.workspace.join("link.txt")).expect("link");
    fs::create_dir(fixture.workspace.join("folder")).expect("folder");
    fs::File::create(fixture.workspace.join("big.bin"))
        .expect("file")
        .set_len(MAX_SHARE_BYTES + 1)
        .expect("size");
    for path in [
        "../secret.txt",
        "link.txt",
        "folder",
        "missing.txt",
        "big.bin",
    ] {
        let output = fixture.call(super::SHARE, path).await;
        assert!(output.is_error, "{path}: {output:?}");
    }
    let big = Fixture::text(&fixture.call(super::SHARE, "big.bin").await);
    assert!(big.contains("Make a smaller copy and share that."), "{big}");
    let rows: i64 = rusqlite::Connection::open(&fixture.database)
        .expect("catalog")
        .query_row("SELECT count(*) FROM host_shares", [], |row| row.get(0))
        .expect("count");
    assert_eq!(rows, 0);
}
