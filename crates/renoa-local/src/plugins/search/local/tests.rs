use std::collections::BTreeMap;

use serde_json::json;

use super::{Inventory, Page};
use crate::plugins::{InstalledPlugin, PluginListReport, PluginMcpServer, PluginMetadata};

#[test]
fn one_package_card_contains_related_servers_and_skill_only_packages_remain_visible() {
    let digest = "a".repeat(64);
    let google = InstalledPlugin {
        digest: digest.clone(),
        metadata: PluginMetadata {
            name: "google".to_owned(),
            version: Some("1.0.0".to_owned()),
            description: Some("Google services".to_owned()),
            homepage: None,
            repository: None,
            license: None,
        },
        mcp_servers: ["drive", "gmail"]
            .into_iter()
            .map(|id| PluginMcpServer {
                id: id.to_owned(),
                endpoint: format!("https://example.test/{id}"),
                request_headers: BTreeMap::new(),
            })
            .collect(),
        notices: Vec::new(),
    };
    let skill_only = InstalledPlugin {
        digest: "b".repeat(64),
        metadata: PluginMetadata {
            name: "writing".to_owned(),
            version: None,
            description: Some("Writing workflow".to_owned()),
            homepage: None,
            repository: None,
            license: None,
        },
        mcp_servers: Vec::new(),
        notices: Vec::new(),
    };
    let packages = PluginListReport::new(vec![google, skill_only], Vec::new());
    let inventory = Inventory::new(&packages, &[], &[], Vec::new(), false);
    for query in ["drive", "gmail"] {
        let result = serde_json::to_value(inventory.search(query, 0).expect("search package"))
            .expect("encode search page");
        assert_eq!(result["total"], 1);
        assert_eq!(result["items"][0]["id"], digest);
        assert_eq!(result["items"][0]["mcp_servers"], 2);
    }
    let browse = serde_json::to_value(inventory.search("*", 0).expect("browse packages"))
        .expect("encode browse page");
    assert_eq!(browse["total"], 2);
    assert!(
        browse["items"]
            .as_array()
            .expect("cards")
            .iter()
            .any(|card| card["name"] == "writing" && card["mcp_servers"] == 0)
    );
    let details = serde_json::to_value(inventory.inspect(&digest, 0).expect("inspect package"))
        .expect("encode facts");
    assert_eq!(
        details["items"],
        json!([{"kind":"mcp_server","server":"drive"},{"kind":"mcp_server","server":"gmail"}])
    );
}

#[test]
fn two_hundred_results_shrink_at_the_output_boundary_and_continue_exactly() {
    let items = (0..200)
        .map(|index| format!("{index:04}-{}", "x".repeat(900)))
        .collect::<Vec<_>>();
    let page = Page::new(items, 200, 0, false).expect("bounded page");
    let value = serde_json::to_value(&page).expect("encode page");
    let count = value["items"].as_array().expect("items").len();
    assert!(count > 0 && count < 200);
    assert_eq!(value["next_offset"], count);
    assert!(
        serde_json::to_vec(&page).expect("encode bytes").len()
            <= crate::output::MAX_TOOL_OUTPUT_BYTES
    );
}
