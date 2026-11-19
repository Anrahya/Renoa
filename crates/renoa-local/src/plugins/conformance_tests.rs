use std::{fs, path::Path};

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

#[test]
fn the_first_party_google_drive_package_is_a_supported_agent_plugin() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/google-drive");
    let inspection = super::inspect::inspect(&root)
        .expect("inspect the repository Google Drive package")
        .inspection;

    assert_eq!(inspection.metadata().name(), "renoa.google-drive");
    assert_eq!(inspection.metadata().version(), Some("0.1.0"));
    assert_eq!(
        inspection.digest(),
        "6bba1577cd76622829c00ee8c00aca52f209aa067c0dcfd493bbfc0bad5f5a2c"
    );
    assert!(inspection.notices().is_empty());
    let [server] = inspection.mcp_servers() else {
        panic!("Google Drive package must expose exactly one supported MCP server");
    };
    assert_eq!(server.id(), "drive");
    assert_eq!(server.endpoint(), "https://drive.renoa.live/mcp");
    assert!(server.request_headers().is_empty());
}

#[test]
fn the_first_party_browse_package_is_a_supported_stdio_agent_plugin() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/browse");
    let inspection = super::inspect::inspect(&root)
        .expect("inspect the repository browse package")
        .inspection;

    assert_eq!(inspection.metadata().name(), "renoa.browse");
    assert_eq!(inspection.metadata().version(), Some("0.1.0"));
    assert!(
        inspection.notices().is_empty(),
        "unexpected notices: {:?}",
        inspection.notices()
    );
    let [server] = inspection.mcp_servers() else {
        panic!("browse package must expose exactly one supported MCP server");
    };
    assert_eq!(server.id(), "browse");
    assert!(crate::mcp::is_stdio_endpoint(server.endpoint()));
    assert!(server.request_headers().is_empty());
    let stdio = server.stdio().expect("browse server uses stdio transport");
    assert_eq!(stdio.command, "browse");
    assert_eq!(stdio.args, vec!["mcp"]);
    assert_eq!(
        stdio
            .env
            .get("OPENCODE_BROWSER_AUTO_UPDATE")
            .map(String::as_str),
        Some("off"),
        "browse keeps the installed version instead of updating itself"
    );
    assert_eq!(stdio.cwd, None);

    let captured_skill =
        crate::skills::package::capture(&root.join("skills/browse"), Some("browse"))
            .expect("capture browse plugin skill");
    assert_eq!(captured_skill.metadata.name, "browse");
}
