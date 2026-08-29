use serde::Deserialize;
use serde_json::Value;
use url::Url;

use super::{
    MAX_OAUTH_STATE_BYTES, MAX_OAUTH_VALUE_BYTES, OAuthResult, WIRE_VERSION, capture::Captured,
};
use crate::mcp::{McpAdapterError, McpAuthorization, McpRemoteFailure};

#[derive(Deserialize)]
#[serde(tag = "event", deny_unknown_fields)]
enum OAuthRecord {
    #[serde(rename = "oauth_redirect")]
    Redirect {
        wire_version: u32,
        authorization_url: String,
        oauth_state: Value,
    },
    #[serde(rename = "oauth_authorized")]
    Authorized {
        wire_version: u32,
        authorization: WireAuthorization,
        oauth_state: Value,
    },
    #[serde(rename = "oauth_refresh_required")]
    RefreshRequired {
        wire_version: u32,
        oauth_state: Value,
    },
    #[serde(rename = "oauth_failed")]
    OAuthFailed {
        wire_version: u32,
        failure: McpRemoteFailure,
        oauth_state: Value,
    },
    #[serde(rename = "failed")]
    Failed {
        wire_version: u32,
        failure: McpRemoteFailure,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireAuthorization {
    scheme: String,
    token: String,
}

pub(super) fn parse_record(
    mut stdout: Captured,
    mut stderr: Captured,
    expected_endpoint: &str,
    exact_secrets: &[&str],
) -> Result<OAuthResult, McpAdapterError> {
    if stdout.truncated {
        stdout.bytes.fill(0);
        stderr.bytes.fill(0);
        return Err(McpAdapterError::OutputLimit);
    }
    let result = parse_single_record(&stdout.bytes, expected_endpoint, exact_secrets);
    stdout.bytes.fill(0);
    stderr.bytes.fill(0);
    result
}

fn parse_single_record(
    encoded: &[u8],
    expected_endpoint: &str,
    exact_secrets: &[&str],
) -> Result<OAuthResult, McpAdapterError> {
    let mut records = encoded
        .split(|byte| *byte == b'\n')
        .filter(|record| !record.is_empty());
    let record = records
        .next()
        .ok_or_else(|| McpAdapterError::Protocol("adapter returned no OAuth record".to_owned()))?;
    if records.next().is_some() {
        return Err(McpAdapterError::Protocol(
            "adapter returned more than one OAuth record".to_owned(),
        ));
    }
    let record: OAuthRecord = serde_json::from_slice(record)
        .map_err(|error| McpAdapterError::Protocol(format!("decode OAuth record: {error}")))?;
    match record {
        OAuthRecord::Redirect {
            wire_version,
            authorization_url,
            oauth_state,
        } => {
            validate_wire(wire_version)?;
            validate_state(&oauth_state, expected_endpoint)?;
            validate_authorization_url(&authorization_url, &oauth_state)?;
            Ok(OAuthResult::Redirect {
                authorization_url,
                state: oauth_state,
            })
        }
        OAuthRecord::Authorized {
            wire_version,
            authorization,
            oauth_state,
        } => {
            validate_wire(wire_version)?;
            let scheme = authorization.scheme;
            let authorization = McpAuthorization::from_token(authorization.token)
                .map_err(McpAdapterError::Credential)?;
            if scheme != "bearer" {
                return Err(McpAdapterError::Protocol(
                    "OAuth adapter returned a non-Bearer token".to_owned(),
                ));
            }
            validate_state(&oauth_state, expected_endpoint)?;
            Ok(OAuthResult::Authorized {
                authorization,
                state: oauth_state,
            })
        }
        OAuthRecord::RefreshRequired {
            wire_version,
            oauth_state,
        } => {
            validate_wire(wire_version)?;
            validate_state(&oauth_state, expected_endpoint)?;
            Ok(OAuthResult::RefreshRequired { state: oauth_state })
        }
        OAuthRecord::OAuthFailed {
            wire_version,
            mut failure,
            oauth_state,
        } => {
            validate_wire(wire_version)?;
            validate_state(&oauth_state, expected_endpoint)?;
            failure.redact_exact_secrets(exact_secrets.iter().copied());
            failure.validate_wire().map_err(McpAdapterError::Protocol)?;
            Ok(OAuthResult::Failed {
                failure,
                state: oauth_state,
            })
        }
        OAuthRecord::Failed {
            wire_version,
            mut failure,
        } => {
            validate_wire(wire_version)?;
            failure.redact_exact_secrets(exact_secrets.iter().copied());
            failure.validate_wire().map_err(McpAdapterError::Protocol)?;
            Err(McpAdapterError::Remote(failure))
        }
    }
}

fn validate_wire(wire_version: u32) -> Result<(), McpAdapterError> {
    if wire_version == WIRE_VERSION {
        Ok(())
    } else {
        Err(McpAdapterError::Protocol(format!(
            "adapter wire version {wire_version} is unsupported; expected {WIRE_VERSION}"
        )))
    }
}

fn validate_state(state: &Value, expected_endpoint: &str) -> Result<(), McpAdapterError> {
    if !state.is_object() || state.get("schema_version").and_then(Value::as_u64) != Some(1) {
        return Err(McpAdapterError::Protocol(
            "OAuth adapter returned malformed durable state".to_owned(),
        ));
    }
    let bytes = serde_json::to_vec(state)
        .map_err(McpAdapterError::Encode)?
        .len();
    if bytes > MAX_OAUTH_STATE_BYTES {
        return Err(McpAdapterError::OutputLimit);
    }
    for name in ["mcp_endpoint", "csrf_state", "redirect_uri"] {
        let value = required_state_string(state, name)?;
        if value.is_empty() || value.len() > MAX_OAUTH_VALUE_BYTES {
            return Err(McpAdapterError::Protocol(format!(
                "OAuth state field '{name}' is malformed"
            )));
        }
    }
    let stored_endpoint = required_state_string(state, "mcp_endpoint")?;
    let stored_endpoint = Url::parse(stored_endpoint).map_err(|_| {
        McpAdapterError::Protocol("OAuth state has a malformed MCP endpoint".to_owned())
    })?;
    let expected_endpoint = Url::parse(expected_endpoint).map_err(|_| {
        McpAdapterError::Protocol("OAuth request has a malformed MCP endpoint".to_owned())
    })?;
    if stored_endpoint != expected_endpoint {
        return Err(McpAdapterError::Protocol(
            "OAuth credential state belongs to a different MCP endpoint".to_owned(),
        ));
    }
    Ok(())
}

fn validate_authorization_url(value: &str, state: &Value) -> Result<(), McpAdapterError> {
    if value.len() > MAX_OAUTH_VALUE_BYTES {
        return Err(McpAdapterError::OutputLimit);
    }
    let url = Url::parse(value)
        .map_err(|_| McpAdapterError::Protocol("OAuth authorization URL is invalid".to_owned()))?;
    let loopback = url.scheme() == "http" && url.host_str().is_some_and(is_loopback);
    if (url.scheme() != "https" && !loopback)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(McpAdapterError::Protocol(
            "OAuth authorization URL is insecure or malformed".to_owned(),
        ));
    }
    let csrf_state = required_state_string(state, "csrf_state")?;
    let redirect_uri = required_state_string(state, "redirect_uri")?;
    if exact_query_value(&url, "state").as_deref() != Some(csrf_state)
        || exact_query_value(&url, "redirect_uri").as_deref() != Some(redirect_uri)
    {
        return Err(McpAdapterError::Protocol(
            "OAuth authorization URL changed the Host callback identity".to_owned(),
        ));
    }
    Ok(())
}

fn required_state_string<'a>(state: &'a Value, name: &str) -> Result<&'a str, McpAdapterError> {
    state
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| McpAdapterError::Protocol(format!("OAuth state is missing '{name}'")))
}

fn exact_query_value(url: &Url, name: &str) -> Option<String> {
    let mut values = url
        .query_pairs()
        .filter_map(|(candidate, value)| (candidate == name).then_some(value));
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    Some(value.into_owned())
}

fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || matches!(host, "[::1]" | "::1")
        || host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|address| address.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::parse_single_record;
    use crate::mcp::McpAdapterError;

    #[test]
    fn redirect_must_preserve_the_host_state_and_callback() {
        let record = br#"{"wire_version":6,"event":"oauth_redirect","authorization_url":"https://auth.example/authorize?state=changed&redirect_uri=http%3A%2F%2F127.0.0.1%3A3210%2Foauth%2Fcallback","oauth_state":{"schema_version":1,"mcp_endpoint":"https://mcp.example/mcp","csrf_state":"expected","redirect_uri":"http://127.0.0.1:3210/oauth/callback"}}
"#;
        let Err(error) = parse_single_record(record, "https://mcp.example/mcp", &[]) else {
            panic!("changed OAuth state must be rejected")
        };
        assert!(matches!(error, McpAdapterError::Protocol(_)));
        assert!(error.to_string().contains("callback identity"));
    }

    #[test]
    fn credential_state_is_bound_to_the_requested_mcp_endpoint() {
        let record = br#"{"wire_version":6,"event":"oauth_authorized","authorization":{"scheme":"bearer","token":"must-not-escape"},"oauth_state":{"schema_version":1,"mcp_endpoint":"https://first.example/mcp","csrf_state":"expected","redirect_uri":"http://127.0.0.1:3210/oauth/callback"}}
"#;
        let Err(error) = parse_single_record(record, "https://second.example/mcp", &[]) else {
            panic!("credential state for another endpoint must be rejected")
        };
        assert!(matches!(error, McpAdapterError::Protocol(_)));
        assert!(error.to_string().contains("different MCP endpoint"));
    }

    #[test]
    fn oauth_failures_cannot_echo_request_secrets_across_the_host_boundary() {
        let record = br#"{"wire_version":6,"event":"oauth_failed","failure":{"kind":"protocol","certainty":"definite","message":"access-one and code-one were rejected","partial_changes_possible":true,"diagnostic":{"code":"access-one","detail":"code-one failed"}},"oauth_state":{"schema_version":1,"mcp_endpoint":"https://mcp.example/mcp","csrf_state":"state-one","redirect_uri":"http://127.0.0.1:3210/oauth/callback","access_token":"access-one"}}
"#;
        let parsed = parse_single_record(
            record,
            "https://mcp.example/mcp",
            &["access-one", "code-one", "state-one"],
        )
        .expect("redacted OAuth failure remains valid");
        let super::OAuthResult::Failed { failure, .. } = parsed else {
            panic!("fixture must produce an OAuth failure");
        };
        let encoded = format!(
            "{} {} {:?}",
            failure.message(),
            failure.diagnostic_detail(),
            failure.diagnostic_code()
        );
        for secret in ["access-one", "code-one", "state-one"] {
            assert!(!encoded.contains(secret));
        }
    }
}
