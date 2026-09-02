use renoa_agent::{ContentBlock, ToolOutput};
use serde::Deserialize;
use url::Url;

use crate::actions::ActionLink;

const MAX_AUTHORIZATION_URL_BYTES: usize = 16 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorizationUpdate {
    status: String,
    connection: String,
    #[serde(default)]
    display_name: Option<String>,
    authorization_url: String,
    #[serde(default)]
    expires_at_ms: Option<i64>,
    message: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CredentialUpdate {
    status: String,
    credential: String,
    credential_kind: String,
    setup_url: String,
    expires_at_ms: i64,
    message: String,
}

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
    let parsed = serde_json::from_str::<AuthorizationUpdate>(text).ok()?;
    if parsed.status != "authorization_required"
        || parsed.connection.is_empty()
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
    let parsed = serde_json::from_str::<CredentialUpdate>(text).ok()?;
    if parsed.status != "credential_required"
        || parsed.credential.is_empty()
        || parsed.credential.len() > 128
        || parsed
            .credential
            .bytes()
            .any(|byte| byte.is_ascii_control())
        || !matches!(
            parsed.credential_kind.as_str(),
            "api_token" | "oauth_client"
        )
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
        || !url.fragment().is_some_and(valid_setup_fragment)
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
    if tool != "extension_manage" || update.is_error || update.content.len() != 1 {
        return None;
    }
    let ContentBlock::Text { text } = &update.content[0] else {
        return None;
    };
    Some(text)
}

fn valid_setup_fragment(fragment: &str) -> bool {
    let mut version = None;
    let mut key = None;
    let mut token = None;
    for (name, value) in url::form_urlencoded::parse(fragment.as_bytes()) {
        let slot = match name.as_ref() {
            "v" => &mut version,
            "key" => &mut key,
            "token" => &mut token,
            _ => return false,
        };
        if slot.replace(value.into_owned()).is_some() {
            return false;
        }
    }
    version.as_deref() == Some("1")
        && key.as_deref().is_some_and(valid_secret_hex)
        && token.as_deref().is_some_and(valid_secret_hex)
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
            extension_progress("extension_manage", &update).as_deref(),
            Some("Authorization needed for Notion MCP. Open the message I sent.")
        );
        let action = extension_action("request/call", "extension_manage", &update)
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
            extension_progress("extension_manage", &update).as_deref(),
            Some("A credential is needed for x.default. Open the secure message I sent.")
        );
        let action = extension_action("request/call", "extension_manage", &update)
            .expect("valid credential update becomes a secure action");
        assert_eq!(action.id, "request/call/credential");
        assert!(action.sensitive_fragment);
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
            assert!(extension_progress("extension_manage", &update).is_none());
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
