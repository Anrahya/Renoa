//! An agent turning an ability on for itself reaches the owner's feed, in the
//! transaction that turns it on; nobody else's change does.

use super::activation::{act, install, package};
use super::*;
use crate::plugins::{
    activation::{SelectedBy, admit_connection_selection},
    host::{HostPluginId, state},
};

/// The ability items in the feed, oldest first: title, agent and subject.
fn abilities(root: &Path) -> Vec<(String, String, String)> {
    rusqlite::Connection::open(root.join("host.sqlite3"))
        .expect("catalog")
        .prepare(
            "SELECT title, agent_id, subject_id FROM host_feed_items
             WHERE kind = 'ability' AND action = 'turn_off' ORDER BY sequence",
        )
        .expect("prepare")
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("items")
}

#[tokio::test]
async fn an_agent_turning_a_plugin_on_for_itself_reports_one_ability_per_act() {
    let root = tempfile::tempdir().expect("fixture");
    let (manager, _) = manager(root.path());
    package(&root.path().join("source"), "review", "Review files", None);
    let digest = install(&manager, root.path(), "source").await;
    let agent = test_agent_id(1);
    let activate = || PluginRequest::Activate {
        package_digest: digest.clone(),
    };
    let enable = || PluginRequest::EnablePlugin {
        plugin_id: digest.clone(),
    };
    let turned_on = (
        "Turned on same-name".to_owned(),
        agent.to_string(),
        digest.clone(),
    );
    act(&manager, root.path(), agent, "on", activate())
        .await
        .expect("activate");
    act(&manager, root.path(), agent, "on", activate())
        .await
        .expect("a replayed activation");
    act(&manager, root.path(), agent, "again", activate())
        .await
        .expect("activating the active revision again");
    assert_eq!(abilities(root.path()), std::slice::from_ref(&turned_on));
    act(
        &manager,
        root.path(),
        agent,
        "off",
        PluginRequest::Deactivate {
            plugin_id: digest.clone(),
        },
    )
    .await
    .expect("deactivate");
    assert_eq!(
        abilities(root.path()).len(),
        1,
        "turning off is not reported"
    );
    act(&manager, root.path(), agent, "back", enable())
        .await
        .expect("enable");
    act(&manager, root.path(), agent, "still", enable())
        .await
        .expect("enabling an enabled plugin");
    assert_eq!(abilities(root.path()), [turned_on.clone(), turned_on]);
}

#[tokio::test]
async fn a_replacement_reports_only_when_it_turns_a_disabled_plugin_on() {
    let root = tempfile::tempdir().expect("fixture");
    let (manager, _) = manager(root.path());
    package(&root.path().join("first"), "review", "FIRST", None);
    package(&root.path().join("next"), "review", "NEXT", None);
    let first = install(&manager, root.path(), "first").await;
    let next = install(&manager, root.path(), "next").await;
    let agent = test_agent_id(1);
    let replace = |from: &str, to: &str| PluginRequest::ReplacePlugin {
        plugin_id: first.clone(),
        package_digest: to.to_owned(),
        expected_digest: from.to_owned(),
    };
    let activate = PluginRequest::Activate {
        package_digest: first.clone(),
    };
    act(&manager, root.path(), agent, "on", activate)
        .await
        .expect("activate");
    act(&manager, root.path(), agent, "next", replace(&first, &next))
        .await
        .expect("replace an enabled plugin");
    assert_eq!(
        abilities(root.path()).len(),
        1,
        "replacing an enabled plugin turns nothing on"
    );
    let deactivate = PluginRequest::Deactivate {
        plugin_id: first.clone(),
    };
    act(&manager, root.path(), agent, "off", deactivate)
        .await
        .expect("deactivate");
    act(&manager, root.path(), agent, "back", replace(&next, &first))
        .await
        .expect("replace a disabled plugin");
    assert_eq!(
        abilities(root.path()),
        [
            (
                "Turned on same-name".to_owned(),
                agent.to_string(),
                first.clone()
            ),
            (
                "Turned on same-name".to_owned(),
                agent.to_string(),
                first.clone()
            ),
        ],
        "replacing a disabled plugin turns it on"
    );
}

#[tokio::test]
async fn an_agent_turning_a_compiled_plugin_back_on_reports_it_once() {
    let root = tempfile::tempdir().expect("fixture");
    let (manager, _) = manager(root.path());
    let database = manager.mcp_catalog().path().to_path_buf();
    let agent = test_agent_id(1);
    let memory = HostPluginId::Memory;
    state::change(&database, agent, memory, true, "default")
        .expect("turning on a plugin already on by default");
    state::change(&database, agent, memory, false, "off").expect("off");
    assert!(abilities(root.path()).is_empty());
    for _ in 0..2 {
        state::change(&database, agent, memory, true, "on").expect("on, then its replay");
    }
    assert_eq!(
        abilities(root.path()),
        [(
            "Turned on Memory".to_owned(),
            agent.to_string(),
            "renoa.memory".to_owned()
        )]
    );
}

#[tokio::test]
async fn only_an_agents_own_account_selection_that_creates_an_activation_reports_it() {
    let root = tempfile::tempdir().expect("fixture");
    let (manager, _) = manager(root.path());
    let endpoint = "http://127.0.0.1:43127/mcp";
    package(
        &root.path().join("source"),
        "review",
        "Review files",
        Some(endpoint),
    );
    let digest = install(&manager, root.path(), "source").await;
    let mcp = manager.mcp_catalog();
    for connection in ["mine", "chosen-for-it"] {
        mcp.register_direct_connection(
            &crate::plugins::manager::integration_id(&digest, "main"),
            connection,
            endpoint,
        )
        .expect("register account");
        mcp.publish_catalog(&crate::mcp::tests::snapshot(
            connection,
            endpoint,
            &["scan"],
        ))
        .expect("publish account tools");
    }
    let chosen_for = test_agent_id(2);
    let mut catalog =
        rusqlite::Connection::open(root.path().join("host.sqlite3")).expect("catalog");
    let tx = catalog.transaction().expect("transaction");
    admit_connection_selection(
        &tx,
        &chosen_for.to_string(),
        "chosen-for-it",
        SelectedBy::OwnerOrCreator,
    )
    .expect("an owner's selection");
    tx.commit().expect("commit");
    assert!(abilities(root.path()).is_empty());

    let agent = test_agent_id(1);
    for _ in 0..2 {
        mcp.enable_agent_connection(&agent.to_string(), "mine")
            .expect("the agent selects its account, then again");
    }
    assert_eq!(
        abilities(root.path()),
        [("Turned on same-name".to_owned(), agent.to_string(), digest)]
    );
}

/// Only `write_activation` writes an agent's plugin activations in production
/// code, and it writes the feed's item, so no turn-on can skip the feed.
#[test]
fn every_write_of_an_agents_plugin_activation_goes_through_the_feed() {
    fn writes(source: &str) -> usize {
        ["INTO host_agent_plugins", "UPDATE host_agent_plugins"]
            .into_iter()
            .map(|statement| {
                source
                    .match_indices(statement)
                    .filter(|(at, _)| {
                        source[at + statement.len()..]
                            .chars()
                            .next()
                            .is_some_and(|next| next == '(' || next.is_whitespace())
                    })
                    .count()
            })
            .sum()
    }
    fn visit(directory: &Path, found: &mut Vec<(String, usize)>) {
        for entry in fs::read_dir(directory).expect("source directory") {
            let path = entry.expect("entry").path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_owned();
            if path.is_dir() {
                if name != "tests" {
                    visit(&path, found);
                }
            } else if path.extension().is_some_and(|extension| extension == "rs")
                && name != "tests.rs"
                && !name.ends_with("_tests.rs")
            {
                let count = writes(&fs::read_to_string(&path).expect("source"));
                if count > 0 {
                    found.push((path.display().to_string(), count));
                }
            }
        }
    }
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    visit(&source, &mut found);
    let activation = source.join("plugins/activation.rs");
    assert_eq!(found, [(activation.display().to_string(), 1)]);
    let text = fs::read_to_string(&activation).expect("activation source");
    let start = text.find("fn write_activation(").expect("the writer");
    let end = start + text[start..].find("\n}\n").expect("its end");
    assert_eq!(
        writes(&text[start..end]),
        1,
        "the one write is in the writer"
    );
    assert!(text[start..end].contains("ability_turned_on("));
}
