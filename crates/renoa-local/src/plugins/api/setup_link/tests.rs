use renoa_agent::{ContentBlock, ToolOutput};
use serde_json::{Value, json};

use super::{InvalidSetupLink, SetupLink, SetupLinkKind};
use crate::PluginCredentialKind;

const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn update(progress: &Value) -> ToolOutput {
    ToolOutput {
        content: vec![ContentBlock::text(progress.to_string())],
        details: None,
        is_error: false,
        closing: None,
    }
}

fn authorization(url: &str) -> Value {
    json!({
        "status": "authorization_required",
        "connection": "plugin.digest.default",
        "display_name": "notion-mcp",
        "authorization_url": url,
        "expires_at_ms": 123,
        "message": "Open it"
    })
}

fn credential(url: &str, kind: &str) -> Value {
    json!({
        "status": "credential_required",
        "credential": "credential.test",
        "credential_kind": kind,
        "setup_url": url,
        "expires_at_ms": 456,
        "message": "Enter it"
    })
}

fn check(progress: &Value) -> Option<Result<SetupLink, InvalidSetupLink>> {
    SetupLink::from_update("plugin_manage", &update(progress))
}

#[test]
fn a_sign_in_link_is_named_for_its_service() {
    let link = check(&authorization(
        "https://provider.example/authorize?state=one",
    ))
    .expect("setup progress")
    .expect("a valid link");
    assert_eq!(link.kind, SetupLinkKind::Authorization);
    assert_eq!(link.label, "Notion MCP");
    assert_eq!(
        link.url.as_str(),
        "https://provider.example/authorize?state=one"
    );
    assert_eq!(link.expires_at_ms, Some(123));
}

#[test]
fn a_credential_link_carries_its_key_in_the_fragment() {
    let token_url = format!("https://renoa.example/setup/relay#v=1&key={KEY}&token={KEY}");
    let link = check(&credential(&token_url, "api_token"))
        .expect("setup progress")
        .expect("a valid link");
    assert_eq!(
        link.kind,
        SetupLinkKind::Credentials(PluginCredentialKind::ApiToken)
    );
    assert_eq!(link.label, "credential.test");
    assert_eq!(link.expires_at_ms, Some(456));
    let client_url = format!(
        "https://renoa.example/setup/relay#v=1&key={KEY}&token={KEY}&issuer=https%3A%2F%2Faccounts.example"
    );
    assert!(matches!(
        check(&credential(&client_url, "oauth_client")),
        Some(Ok(_))
    ));
}

#[test]
fn an_unsafe_link_is_refused_and_not_shown() {
    let refused = [
        authorization("http://provider.example/authorize"),
        authorization("https://user:pass@provider.example/authorize"),
        authorization("https://provider.example/authorize#fragment"),
        authorization("https://provider.example/authorize?\u{7}"),
        authorization(&format!(
            "https://provider.example/authorize?q={}",
            "é".repeat(6_000)
        )),
        {
            let mut expired = authorization("https://provider.example/authorize");
            expired["expires_at_ms"] = json!(0);
            expired
        },
        credential(
            &format!("https://renoa.example/setup?x=1#v=1&key={KEY}&token={KEY}"),
            "api_token",
        ),
        credential("https://renoa.example/setup#v=1&token=x", "api_token"),
        credential(
            &format!(
                "https://renoa.example/setup#v=1&key={}&token={KEY}",
                KEY.to_uppercase()
            ),
            "api_token",
        ),
        credential(
            &format!("https://renoa.example/setup#v=1&key={KEY}&token={KEY}&extra=1"),
            "api_token",
        ),
        credential(
            &format!("https://renoa.example/setup#v=1&v=1&key={KEY}&token={KEY}"),
            "api_token",
        ),
        credential(
            &format!(
                "https://renoa.example/setup#v=1&key={KEY}&token={KEY}&issuer=https%3A%2F%2Fa.example"
            ),
            "api_token",
        ),
        credential(
            &format!(
                "https://renoa.example/setup#v=1&key={KEY}&token={KEY}&issuer=http%3A%2F%2Fa.example"
            ),
            "oauth_client",
        ),
    ];
    for progress in refused {
        assert_eq!(check(&progress), Some(Err(InvalidSetupLink)), "{progress}");
    }
}

#[test]
fn other_updates_are_not_setup_links() {
    let link = authorization("https://provider.example/authorize");
    assert_eq!(SetupLink::from_update("other_tool", &update(&link)), None);
    let mut failed = update(&link);
    failed.is_error = true;
    assert_eq!(SetupLink::from_update("plugin_manage", &failed), None);
    assert_eq!(check(&json!({"status": "catalog_loaded"})), None);
}

/// One owner: no surface keeps its own copy of the link rules.
#[test]
fn no_surface_keeps_its_own_link_check() {
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace crates");
    let mut copies = Vec::new();
    let mut directories = vec![crates.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(&directory).expect("read source tree") {
            let path = entry.expect("source entry").path();
            if path.is_dir() {
                if !path.ends_with("target") && !path.ends_with("node_modules") {
                    directories.push(path);
                }
            } else if path.extension().is_some_and(|extension| extension == "rs")
                && !path.ends_with("plugins/api/setup_link.rs")
                && !path.ends_with("plugins/api/setup_link/tests.rs")
                && std::fs::read_to_string(&path).is_ok_and(|source| {
                    [
                        "fn valid_fragment(",
                        "fn valid_setup_fragment(",
                        "fn valid_secret_hex(",
                    ]
                    .iter()
                    .any(|rule| source.contains(rule))
                })
            {
                copies.push(path);
            }
        }
    }
    assert!(copies.is_empty(), "setup link rules copied into {copies:?}");
}
