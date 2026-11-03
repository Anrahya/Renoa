use renoa_agent::ToolOutput;
use renoa_local::{SetupLink, SetupLinkKind};

use crate::actions::ActionLink;

/// A `plugin_manage` update's setup link, once it passed the shared check.
/// An unsafe link is never shown.
fn setup_link(tool: &str, update: &ToolOutput) -> Option<SetupLink> {
    SetupLink::from_update(tool, update)?.ok()
}

pub(super) fn extension_progress(tool: &str, update: &ToolOutput) -> Option<String> {
    let link = setup_link(tool, update)?;
    Some(match link.kind {
        SetupLinkKind::Authorization => format!(
            "Authorization needed for {}. Open the message I sent.",
            link.label
        ),
        SetupLinkKind::Credentials(_) => format!(
            "A credential is needed for {}. Open the secure message I sent.",
            link.label
        ),
    })
}

pub(super) fn extension_action(
    action_prefix: &str,
    tool: &str,
    update: &ToolOutput,
) -> Option<ActionLink> {
    let link = setup_link(tool, update)?;
    Some(match link.kind {
        SetupLinkKind::Authorization => ActionLink::new(
            format!("{action_prefix}/authorization"),
            format!("Authorize {}", link.label),
            "Open the provider page to finish connecting this MCP.".to_owned(),
            "Authorize".to_owned(),
            link.url,
            link.expires_at_ms,
        ),
        SetupLinkKind::Credentials(_) => ActionLink::sensitive(
            format!("{action_prefix}/credential"),
            format!("Connect {}", link.label),
            "Enter it on Renoa's encrypted setup page. Do not paste the secret into chat."
                .to_owned(),
            "Open secure setup".to_owned(),
            link.url,
            link.expires_at_ms,
        ),
    })
}

#[cfg(test)]
mod tests {
    use renoa_agent::{ContentBlock, ToolOutput};

    use super::{extension_action, extension_progress};

    #[test]
    fn only_a_structured_https_extension_authorization_is_shown() {
        let update = output(
            r#"{"status":"authorization_required","connection":"plugin.digest.default","display_name":"notion-mcp","authorization_url":"https://provider.example/authorize?state=one","expires_at_ms":123,"message":"Open it"}"#,
            false,
        );
        assert_eq!(
            extension_progress("plugin_manage", &update).as_deref(),
            Some("Authorization needed for Notion MCP. Open the message I sent.")
        );
        let action = extension_action("request/call", "plugin_manage", &update)
            .expect("valid authorization becomes a permanent action");
        assert_eq!(action.title, "Authorize Notion MCP");
        assert_eq!(action.button, "Authorize");
        assert_eq!(
            action.url.as_str(),
            "https://provider.example/authorize?state=one"
        );
        assert!(extension_progress("read_file", &update).is_none());
    }

    #[test]
    fn a_secure_credential_update_becomes_a_fragment_bearing_action() {
        let secret = "a".repeat(64);
        let update = output(
            &format!(
                "{{\"status\":\"credential_required\",\"credential\":\"x.default\",\"credential_kind\":\"api_token\",\"setup_url\":\"https://renoa.live/v1/credential-relays/00000000-0000-0000-0000-000000000001/setup#v=1&key={secret}&token={secret}\",\"expires_at_ms\":9999999999999,\"message\":\"Open it\"}}"
            ),
            false,
        );
        assert_eq!(
            extension_progress("plugin_manage", &update).as_deref(),
            Some("A credential is needed for x.default. Open the secure message I sent.")
        );
        let action = extension_action("request/call", "plugin_manage", &update)
            .expect("valid credential update becomes a secure action");
        assert_eq!(action.id, "request/call/credential");
        assert!(action.sensitive_fragment);
    }

    #[test]
    fn oauth_client_setup_preserves_the_discovered_issuer_in_the_private_link() {
        let secret = "a".repeat(64);
        let event = renoa_local::PluginProgress::CredentialRequired(
            renoa_local::PluginCredentialRequired {
                credential: "provider.client".to_owned(),
                credential_kind: renoa_local::PluginCredentialKind::OAuthClient,
                setup_url: format!(
                    "https://renoa.example/setup#v=1&key={secret}&token={secret}&issuer=https%3A%2F%2Faccounts.example"
                ),
                expires_at_ms: i64::MAX,
                message: "Open secure setup".to_owned(),
            },
        );
        let update = output(
            &serde_json::to_string(&event).expect("canonical event"),
            false,
        );
        let action = extension_action("request/call", "plugin_manage", &update)
            .expect("OAuth app setup action");
        assert!(action.sensitive_fragment);
        assert!(
            action
                .url
                .fragment()
                .expect("private fragment")
                .contains("issuer=")
        );
    }

    #[test]
    fn malformed_insecure_and_failed_updates_stay_hidden() {
        for update in [
            output("arbitrary tool output", false),
            output(
                r#"{"status":"authorization_required","connection":"exa","authorization_url":"http://provider.example/authorize","message":"Open it"}"#,
                false,
            ),
            output(
                r#"{"status":"authorization_required","connection":"exa","authorization_url":"https://provider.example/authorize","message":"Open it"}"#,
                true,
            ),
        ] {
            assert!(extension_progress("plugin_manage", &update).is_none());
        }
    }

    fn output(text: &str, is_error: bool) -> ToolOutput {
        ToolOutput {
            content: vec![ContentBlock::text(text)],
            details: None,
            is_error,
            closing: None,
        }
    }
}
