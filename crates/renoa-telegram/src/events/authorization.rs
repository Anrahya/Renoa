use renoa_agent::{ContentBlock, ToolOutput};
use renoa_local::{PluginCredentialKind, PluginProgress};
use url::Url;

use crate::actions::ActionLink;

const MAX_AUTHORIZATION_URL_BYTES: usize = 16 * 1024;

pub(super) fn extension_progress(tool: &str, update: &ToolOutput) -> Option<String> {
    if let Some(parsed) = parse_authorization(tool, update) {
        return Some(format!(
            "Authorization needed for {}. Open the message I sent.",
            parsed.label
        ));
    }
    let parsed = parse_credential(tool, update)?;
    Some(format!(
        "A credential is needed for {}. Open the secure message I sent.",
        parsed.credential
    ))
}

pub(super) fn extension_action(
    action_prefix: &str,
    tool: &str,
    update: &ToolOutput,
) -> Option<ActionLink> {
    if let Some(parsed) = parse_authorization(tool, update) {
        return Some(ActionLink::new(
            format!("{action_prefix}/authorization"),
            format!("Authorize {}", parsed.label),
            "Open the provider page to finish connecting this MCP.".to_owned(),
            "Authorize".to_owned(),
            parsed.authorization_url,
            parsed.expires_at_ms,
        ));
    }
    let parsed = parse_credential(tool, update)?;
    Some(ActionLink::sensitive(
        format!("{action_prefix}/credential"),
        format!("Connect {}", parsed.credential),
        "Enter it on Renoa's encrypted setup page. Do not paste the secret into chat.".to_owned(),
        "Open secure setup".to_owned(),
        parsed.setup_url,
        Some(parsed.expires_at_ms),
    ))
}

struct ParsedAuthorization {
    label: String,
    authorization_url: Url,
    expires_at_ms: Option<i64>,
}

struct ParsedCredential {
    credential: String,
    setup_url: Url,
    expires_at_ms: i64,
}

fn parse_authorization(tool: &str, update: &ToolOutput) -> Option<ParsedAuthorization> {
    let text = extension_text(tool, update)?;
    let PluginProgress::AuthorizationRequired(parsed) = serde_json::from_str(text).ok()? else {
        return None;
    };
    if parsed.connection.is_empty()
        || parsed.connection.len() > 128
        || parsed
            .connection
            .bytes()
            .any(|byte| byte.is_ascii_control())
        || parsed.message.is_empty()
        || parsed.expires_at_ms.is_some_and(|expiry| expiry <= 0)
    {
        return None;
    }
    let url = Url::parse(&parsed.authorization_url).ok()?;
    if parsed.authorization_url.len() > MAX_AUTHORIZATION_URL_BYTES
        || url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let label = parsed
        .display_name
        .as_deref()
        .filter(|name| valid_display_name(name))
        .map(humanize_name)
        .unwrap_or(parsed.connection);
    Some(ParsedAuthorization {
        label,
        authorization_url: url,
        expires_at_ms: parsed.expires_at_ms,
    })
}

fn valid_display_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
        })
}

fn humanize_name(name: &str) -> String {
    name.split(['.', '-'])
        .filter(|part| !part.is_empty())
        .map(|part| match part {
            "api" => "API".to_owned(),
            "mcp" => "MCP".to_owned(),
            "oauth" => "OAuth".to_owned(),
            _ => {
                let mut characters = part.chars();
                characters.next().map_or_else(String::new, |first| {
                    first.to_uppercase().chain(characters).collect()
                })
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_credential(tool: &str, update: &ToolOutput) -> Option<ParsedCredential> {
    let text = extension_text(tool, update)?;
    let PluginProgress::CredentialRequired(parsed) = serde_json::from_str(text).ok()? else {
        return None;
    };
    if parsed.credential.is_empty()
        || parsed.credential.len() > 128
        || parsed
            .credential
            .bytes()
            .any(|byte| byte.is_ascii_control())
        || parsed.message.is_empty()
        || parsed.expires_at_ms <= 0
    {
        return None;
    }
    let url = Url::parse(&parsed.setup_url).ok()?;
    if parsed.setup_url.len() > MAX_AUTHORIZATION_URL_BYTES
        || url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || !url
            .fragment()
            .is_some_and(|fragment| valid_setup_fragment(fragment, parsed.credential_kind))
    {
        return None;
    }
    Some(ParsedCredential {
        credential: parsed.credential,
        setup_url: url,
        expires_at_ms: parsed.expires_at_ms,
    })
}

fn extension_text<'a>(tool: &str, update: &'a ToolOutput) -> Option<&'a str> {
    if tool != "plugin_manage" || update.is_error || update.content.len() != 1 {
        return None;
    }
    let ContentBlock::Text { text } = &update.content[0] else {
        return None;
    };
    Some(text)
}

fn valid_setup_fragment(fragment: &str, kind: PluginCredentialKind) -> bool {
    let mut version = None;
    let mut key = None;
    let mut token = None;
    let mut issuer = None;
    for (name, value) in url::form_urlencoded::parse(fragment.as_bytes()) {
        let slot = match name.as_ref() {
            "v" => &mut version,
            "key" => &mut key,
            "token" => &mut token,
            "issuer" => &mut issuer,
            _ => return false,
        };
        if slot.replace(value.into_owned()).is_some() {
            return false;
        }
    }
    version.as_deref() == Some("1")
        && key.as_deref().is_some_and(valid_secret_hex)
        && token.as_deref().is_some_and(valid_secret_hex)
        && match kind {
            PluginCredentialKind::ApiToken => issuer.is_none(),
            PluginCredentialKind::OAuthClient => issuer.as_deref().is_some_and(|issuer| {
                Url::parse(issuer).is_ok_and(|issuer| {
                    issuer.scheme() == "https"
                        && issuer.host_str().is_some()
                        && issuer.username().is_empty()
                        && issuer.password().is_none()
                        && issuer.query().is_none()
                        && issuer.fragment().is_none()
                })
            }),
        }
}

fn valid_secret_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
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
        }
    }
}
