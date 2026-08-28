use std::fs;

use serde_json::json;
use tempfile::tempdir;

use super::store::PluginStore;
use crate::host::catalog;

#[test]
fn a_published_tree_recovers_when_its_database_commit_was_lost() {
    let directory = tempdir().expect("temporary plugin store");
    let database = directory.path().join("host.sqlite3");
    catalog::initialize(&database).expect("initialize Host catalog");
    let source = directory.path().join("source");
    fs::create_dir(&source).expect("create plugin source");
    fs::write(
        source.join("plugin.json"),
        serde_json::to_vec(&json!({
            "$schema": super::inspect::PLUGIN_SCHEMA,
            "name": "recovery-fixture"
        }))
        .expect("encode manifest"),
    )
    .expect("write manifest");
    let store = PluginStore::initialize(database.clone(), directory.path().join("packages"))
        .expect("initialize plugin store");
    let inspected = super::inspect::inspect(&source)
        .expect("inspect source")
        .inspection;
    let installed = store
        .install(&source, inspected.digest())
        .expect("install source");
    rusqlite::Connection::open(&database)
        .expect("open Host catalog")
        .execute_batch(
            "DELETE FROM plugin_mcp_servers;
             DELETE FROM installed_plugins;",
        )
        .expect("simulate a lost metadata commit");
    fs::remove_dir_all(&source).expect("remove mutable source");

    assert_eq!(
        store
            .install(&source, inspected.digest())
            .expect("recover from the immutable published tree"),
        installed
    );
}
