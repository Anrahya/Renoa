use renoa_agent::{ContentBlock, ToolErrorCode};
use serde_json::{Value, json};

use super::super::output::{json_output, remote_mcp_error_output};
use crate::{mcp::McpRemoteFailure, output::MAX_TOOL_OUTPUT_BYTES};

#[test]
fn manager_results_fail_instead_of_overfilling_model_context() {
    let oversized = "x".repeat(MAX_TOOL_OUTPUT_BYTES + 1);
    let error = json_output(&oversized).expect_err("oversized manager result must fail");

    assert_eq!(error.code(), ToolErrorCode::OutputLimit);
}

#[test]
fn an_unknown_mcp_discovery_result_stays_model_visible() {
    let remote: McpRemoteFailure = serde_json::from_value(json!({
        "kind": "transport",
        "certainty": "unknown",
        "message": "connection closed after dispatch",
        "partial_changes_possible": true,
        "diagnostic": {"code": "ECONNRESET", "detail": "socket closed"}
    }))
    .expect("decode fixture failure");

    let output = remote_mcp_error_output(&remote).expect("unknown result must not stop Alpha");

    assert!(output.is_error);
    let [ContentBlock::Text { text }] = output.content.as_slice() else {
        panic!("unknown result must be one model-visible text block")
    };
    let model: Value = serde_json::from_str(text).expect("decode model-visible uncertainty");
    assert_eq!(model["code"], "mcp_outcome_unknown");
    assert_eq!(model["retryable"], false);
    assert!(
        model["message"]
            .as_str()
            .unwrap()
            .contains("may or may not")
    );
    assert_eq!(
        output.details.unwrap()["mcp"]["failure"]["certainty"],
        "unknown"
    );
}

#[test]
fn unsupported_oauth_registration_reports_the_credential_boundary() {
    let remote: McpRemoteFailure = serde_json::from_value(json!({
        "kind": "protocol",
        "certainty": "definite",
        "message": "The authorization server does not support the selected OAuth client registration mode.",
        "partial_changes_possible": false,
        "diagnostic": {
            "code": "oauth_registration_required",
            "detail": "Configure this connection with pre_registered OAuth credentials or an official Client ID Metadata Document URL."
        }
    }))
    .expect("decode OAuth registration failure");

    let output = remote_mcp_error_output(&remote).expect("failure remains model-visible");

    assert!(output.is_error);
    let [ContentBlock::Text { text }] = output.content.as_slice() else {
        panic!("OAuth setup failure must be one model-visible text block")
    };
    let model: Value = serde_json::from_str(text).expect("decode model-visible setup failure");
    assert_eq!(model["code"], "oauth_registration_required");
    assert_eq!(model["retryable"], false);
    assert_eq!(
        model["message"],
        "The authorization server does not support the selected OAuth client registration mode."
    );
    let next_action = model["next_action"]
        .as_str()
        .expect("setup failure has a next action");
    assert!(next_action.contains("Do not retry dynamic registration"));
    assert!(next_action.contains("outside the agent context"));
    assert!(next_action.contains("has no credential-entry UI"));
    assert!(next_action.contains("never ask the user to paste credential material"));
    assert_eq!(
        output.details.unwrap()["mcp"]["failure"]["diagnostic"]["code"],
        "oauth_registration_required"
    );
}
