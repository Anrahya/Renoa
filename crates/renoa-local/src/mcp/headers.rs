use std::collections::{BTreeMap, HashSet};

use serde::Serialize;

use super::McpHostError;

const MAX_HEADERS: usize = 64;
const MAX_HEADER_BYTES: usize = 32 * 1_024;

const CLIENT_OWNED_HEADERS: &[&str] = &[
    "accept",
    "authorization",
    "connection",
    "content-length",
    "content-type",
    "cookie",
    "host",
    "mcp-method",
    "mcp-protocol-version",
    "mcp-session-id",
    "proxy-authorization",
    "set-cookie",
    "transfer-encoding",
    "x-api-key",
];

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub(crate) struct McpRequestHeaders(BTreeMap<String, String>);

impl McpRequestHeaders {
    pub(crate) fn new(
        entries: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, McpHostError> {
        let mut headers = BTreeMap::new();
        let mut seen = HashSet::new();
        let mut bytes = 0_usize;
        for (name, value) in entries {
            if headers.len() >= MAX_HEADERS {
                return Err(McpHostError::Invalid(format!(
                    "MCP request headers exceed {MAX_HEADERS} entries"
                )));
            }
            let normalized = name.to_ascii_lowercase();
            if !valid_name(&name) || CLIENT_OWNED_HEADERS.contains(&normalized.as_str()) {
                return Err(McpHostError::Invalid(format!(
                    "MCP request header name '{name}' is invalid or Host-owned"
                )));
            }
            if !seen.insert(normalized.clone()) {
                return Err(McpHostError::Invalid(format!(
                    "MCP request header '{name}' is repeated case-insensitively"
                )));
            }
            if !valid_value(&value) {
                return Err(McpHostError::Invalid(format!(
                    "MCP request header '{name}' has an invalid value"
                )));
            }
            bytes = bytes
                .checked_add(name.len())
                .and_then(|total| total.checked_add(value.len()))
                .ok_or_else(|| {
                    McpHostError::Invalid("MCP request header size overflowed".to_owned())
                })?;
            if bytes > MAX_HEADER_BYTES {
                return Err(McpHostError::Invalid(format!(
                    "MCP request headers exceed {MAX_HEADER_BYTES} bytes"
                )));
            }
            headers.insert(normalized, value);
        }
        Ok(Self(headers))
    }

    pub(crate) fn from_stored(encoded: &str) -> Result<Self, McpHostError> {
        let values = serde_json::from_str::<BTreeMap<String, String>>(encoded)?;
        Self::new(values)
    }

    pub(crate) fn encoded(&self) -> Result<String, McpHostError> {
        Ok(serde_json::to_string(self)?)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn values(&self) -> &BTreeMap<String, String> {
        &self.0
    }
}

fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value.is_ascii()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn valid_value(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte == b'\t' || matches!(byte, 0x20..=0x7e))
}

#[cfg(test)]
mod tests {
    use super::McpRequestHeaders;

    #[test]
    fn public_headers_are_normalized_and_sensitive_names_fail_closed() {
        let headers =
            McpRequestHeaders::new([("X-Exa-Source".to_owned(), "agent-plugin".to_owned())])
                .expect("valid public header");
        assert_eq!(
            headers.values().get("x-exa-source").map(String::as_str),
            Some("agent-plugin")
        );
        assert!(
            McpRequestHeaders::new([(
                "Authorization".to_owned(),
                "Bearer package-secret".to_owned()
            )])
            .is_err()
        );
        assert!(
            McpRequestHeaders::new([
                ("Tenant".to_owned(), "one".to_owned()),
                ("tenant".to_owned(), "two".to_owned()),
            ])
            .is_err()
        );
    }
}
