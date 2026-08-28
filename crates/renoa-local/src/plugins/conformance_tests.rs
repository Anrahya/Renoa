use std::fs;

use serde_json::json;
use tempfile::tempdir;

#[test]
fn an_mcp_fixed_location_with_the_wrong_kind_is_reported_and_isolated() {
    let directory = tempdir().expect("temporary plugin fixture");
    fs::write(
        directory.path().join("plugin.json"),
        serde_json::to_vec(&json!({
            "$schema": super::inspect::PLUGIN_SCHEMA,
            "name": "wrong-mcp-kind"
        }))
        .expect("encode manifest"),
    )
    .expect("write manifest");
    fs::create_dir(directory.path().join("mcp.json")).expect("create invalid MCP directory");
    fs::write(directory.path().join("mcp.json/server.json"), b"{}").expect("write nested fixture");

    let inspection = super::inspect::inspect(directory.path())
        .expect("wrong optional component kind must not reject the package")
        .inspection;

    assert!(inspection.mcp_servers().is_empty());
    assert!(inspection.notices().iter().any(|notice| {
        notice.component() == "mcp" && notice.reason().contains("not a real file")
    }));
}
