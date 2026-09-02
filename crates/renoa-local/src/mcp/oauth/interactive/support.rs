use renoa_agent::{ContentBlock, ToolOutput, ToolUpdates};
use serde::Serialize;
use serde_json::Value;
use url::Url;

use crate::mcp::{McpHostError, McpOAuthError, hex_sha256};

#[derive(Serialize)]
struct RedirectUpdate<'a> {
    status: &'static str,
    connection: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<&'a str>,
    authorization_url: &'a str,
    expires_at_ms: Option<i64>,
    message: &'static str,
}

pub(super) async fn emit_redirect(
    updates: Option<&ToolUpdates>,
    connection_id: &str,
    display_name: Option<&str>,
    authorization_url: &str,
    expires_at_ms: Option<i64>,
) {
    let Some(updates) = updates else {
        return;
    };
    let update = RedirectUpdate {
        status: "authorization_required",
        connection: connection_id,
        display_name,
        authorization_url,
        expires_at_ms,
        message: "Open the authorization link in a browser. Renoa is waiting for the callback.",
    };
    if let Ok(encoded) = serde_json::to_string(&update) {
        updates
            .emit(ToolOutput {
                content: vec![ContentBlock::text(encoded)],
                details: None,
                is_error: false,
            })
            .await;
    }
}

pub(super) fn random_state() -> Result<String, McpHostError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| {
        McpOAuthError::Invalid(format!("secure random state generation failed: {error}"))
    })?;
    Ok(hex_sha256(&bytes))
}

pub(super) fn state_string<'a>(state: &'a Value, name: &str) -> Result<&'a str, McpHostError> {
    state.get(name).and_then(Value::as_str).ok_or_else(|| {
        McpOAuthError::Invalid(format!("durable OAuth state is missing '{name}'")).into()
    })
}

pub(super) fn authorization_url(state: &Value) -> Result<&str, McpHostError> {
    let value = state_string(state, "authorization_url")?;
    let url = Url::parse(value).map_err(|_| {
        McpOAuthError::Invalid("durable OAuth authorization URL is malformed".to_owned())
    })?;
    let loopback = url.scheme() == "http"
        && url.host_str().is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost")
                || matches!(host, "::1" | "[::1]")
                || host
                    .parse::<std::net::Ipv4Addr>()
                    .is_ok_and(|address| address.is_loopback())
        });
    if (url.scheme() != "https" && !loopback)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || exact_query_value(&url, "state").as_deref() != Some(state_string(state, "csrf_state")?)
        || exact_query_value(&url, "redirect_uri").as_deref()
            != Some(state_string(state, "redirect_uri")?)
    {
        return Err(McpOAuthError::Invalid(
            "durable OAuth authorization URL changed the Host callback identity".to_owned(),
        )
        .into());
    }
    Ok(value)
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

pub(super) fn validate_state_identity(
    state: &Value,
    expected_state: &str,
    expected_redirect: &str,
) -> Result<(), McpHostError> {
    if state_string(state, "csrf_state")? != expected_state
        || state_string(state, "redirect_uri")? != expected_redirect
    {
        return Err(McpOAuthError::Invalid(
            "OAuth adapter changed the Host callback identity".to_owned(),
        )
        .into());
    }
    Ok(())
}
