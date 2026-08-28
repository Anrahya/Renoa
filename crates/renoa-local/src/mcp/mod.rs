mod auth;
mod call;
mod error;
mod headers;
mod process;
mod registry;
mod store;
mod tool;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

pub(crate) use auth::{McpAuthorization, McpConnectionAuth, McpCredentialResolver};
pub(crate) use headers::McpRequestHeaders;

pub use error::{
    McpAdapterError, McpCredentialError, McpFailureKind, McpHostError, McpOutcomeCertainty,
    McpRemoteFailure,
};

pub(crate) use process::{discover, discover_cancellable};
pub(crate) use registry::{
    LOAD_OUTPUT_BYTES, LOAD_REFERENCE_LIMIT, McpToolReference, SEARCH_RESULT_LIMIT, rank_tools,
};
pub(crate) use store::McpCatalogStore;
pub(crate) use tool::{adapter_tool_error, alpha_registry_bindings};

const MCP_PROTOCOL_VERSION: &str = "2026-07-28";
const MCP_ADAPTER_REVISION: &str = "mcp-client-node-v0.4.0";
const MCP_LEGACY_PROTOCOL_VERSIONS: &[&str] = &[
    "2025-11-25",
    "2025-06-18",
    "2025-03-26",
    "2024-11-05",
    "2024-10-07",
];
const MAX_ENDPOINT_BYTES: usize = 8 * 1_024;
const MAX_CATALOG_TOOLS: usize = 1_024;
const MAX_TOOL_NAME_BYTES: usize = 128;
const MAX_DESCRIPTION_BYTES: usize = 32 * 1_024;
const MAX_SCHEMA_BYTES: usize = 1_024 * 1_024;
const MAX_REJECTION_BYTES: usize = 512;

/// One validated tool from a complete MCP catalog snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpCatalogTool {
    name: String,
    description: String,
    input_schema: Value,
    model_input_schema: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    output_schema: Option<Value>,
}

impl McpCatalogTool {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    #[must_use]
    pub fn input_schema(&self) -> &Value {
        &self.input_schema
    }

    #[must_use]
    pub fn model_input_schema(&self) -> &Value {
        &self.model_input_schema
    }

    #[must_use]
    pub fn output_schema(&self) -> Option<&Value> {
        self.output_schema.as_ref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpRejectedTool {
    index: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    reason: String,
}

impl McpRejectedTool {
    #[must_use]
    pub const fn index(&self) -> usize {
        self.index
    }

    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// One complete, validated catalog published for a Host connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpCatalogSnapshot {
    connection_id: String,
    endpoint: String,
    request_headers: McpRequestHeaders,
    protocol_version: String,
    adapter_revision: String,
    digest: String,
    tools: Vec<McpCatalogTool>,
    rejected_tools: Vec<McpRejectedTool>,
}

impl McpCatalogSnapshot {
    #[must_use]
    pub fn connection_id(&self) -> &str {
        &self.connection_id
    }

    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    #[must_use]
    pub fn request_headers(&self) -> &std::collections::BTreeMap<String, String> {
        self.request_headers.values()
    }

    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    #[must_use]
    pub fn tools(&self) -> &[McpCatalogTool] {
        &self.tools
    }

    #[must_use]
    pub fn rejected_tools(&self) -> &[McpRejectedTool] {
        &self.rejected_tools
    }

    #[must_use]
    pub fn protocol_version(&self) -> &str {
        &self.protocol_version
    }

    #[must_use]
    pub fn adapter_revision(&self) -> &str {
        &self.adapter_revision
    }
}

/// One exact catalog-bound MCP target resolved for Alpha execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlphaMcpTool {
    integration_id: String,
    connection_id: String,
    endpoint: String,
    request_headers: McpRequestHeaders,
    protocol_version: String,
    adapter_revision: String,
    auth: McpConnectionAuth,
    tool: McpCatalogTool,
}

impl AlphaMcpTool {
    #[must_use]
    pub fn integration_id(&self) -> &str {
        &self.integration_id
    }

    #[must_use]
    pub fn connection_id(&self) -> &str {
        &self.connection_id
    }

    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub(crate) fn request_headers(&self) -> &McpRequestHeaders {
        &self.request_headers
    }

    #[must_use]
    pub fn protocol_version(&self) -> &str {
        &self.protocol_version
    }

    #[must_use]
    pub fn adapter_revision(&self) -> &str {
        &self.adapter_revision
    }

    #[must_use]
    pub const fn tool(&self) -> &McpCatalogTool {
        &self.tool
    }

    pub(crate) const fn auth(&self) -> &McpConnectionAuth {
        &self.auth
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdapterCatalog {
    endpoint: String,
    protocol_version: String,
    adapter_revision: String,
    tools: Vec<McpCatalogTool>,
    rejected_tools: Vec<McpRejectedTool>,
}

impl AdapterCatalog {
    fn redact_authorization(&mut self, authorization: Option<&McpAuthorization>) {
        let Some(authorization) = authorization else {
            return;
        };
        authorization.redact_text(&mut self.endpoint);
        for tool in &mut self.tools {
            authorization.redact_text(&mut tool.description);
            authorization.redact_json(&mut tool.input_schema);
            authorization.redact_json(&mut tool.model_input_schema);
            if let Some(output_schema) = &mut tool.output_schema {
                authorization.redact_json(output_schema);
            }
        }
        for rejected in &mut self.rejected_tools {
            if let Some(name) = &mut rejected.name {
                authorization.redact_text(name);
            }
            authorization.redact_text(&mut rejected.reason);
        }
    }
}

impl McpCatalogSnapshot {
    #[cfg(test)]
    fn from_adapter(connection_id: &str, catalog: AdapterCatalog) -> Result<Self, McpHostError> {
        Self::from_adapter_with_headers(connection_id, McpRequestHeaders::default(), catalog)
    }

    fn from_adapter_with_headers(
        connection_id: &str,
        request_headers: McpRequestHeaders,
        catalog: AdapterCatalog,
    ) -> Result<Self, McpHostError> {
        validate_identity("connection", connection_id)?;
        validate_endpoint(&catalog.endpoint)?;
        if catalog.protocol_version != MCP_PROTOCOL_VERSION
            && !MCP_LEGACY_PROTOCOL_VERSIONS.contains(&catalog.protocol_version.as_str())
        {
            return Err(McpHostError::Invalid(format!(
                "adapter returned unsupported protocol {}",
                catalog.protocol_version
            )));
        }
        if catalog.adapter_revision != MCP_ADAPTER_REVISION {
            return Err(McpHostError::Invalid(format!(
                "adapter returned revision {}, expected {MCP_ADAPTER_REVISION}",
                catalog.adapter_revision
            )));
        }
        if catalog
            .tools
            .len()
            .saturating_add(catalog.rejected_tools.len())
            > MAX_CATALOG_TOOLS
        {
            return Err(McpHostError::Invalid(format!(
                "catalog exceeds {MAX_CATALOG_TOOLS} total tool entries"
            )));
        }
        let mut previous: Option<&str> = None;
        for tool in &catalog.tools {
            validate_tool(tool)?;
            if previous.is_some_and(|name| name >= tool.name()) {
                return Err(McpHostError::Invalid(
                    "catalog tool names are not strictly ordered and unique".to_owned(),
                ));
            }
            previous = Some(tool.name());
        }
        let mut previous_rejection = None;
        for rejected in &catalog.rejected_tools {
            if rejected.index >= MAX_CATALOG_TOOLS {
                return Err(McpHostError::Invalid(
                    "rejected tool index exceeds the catalog bound".to_owned(),
                ));
            }
            if previous_rejection.is_some_and(|index| index >= rejected.index) {
                return Err(McpHostError::Invalid(
                    "rejected tool indexes are not strictly ordered and unique".to_owned(),
                ));
            }
            if rejected
                .name
                .as_ref()
                .is_some_and(|name| name.len() > MAX_TOOL_NAME_BYTES)
                || rejected.reason.len() > MAX_REJECTION_BYTES
            {
                return Err(McpHostError::Invalid(
                    "rejected tool diagnostic exceeds its bound".to_owned(),
                ));
            }
            previous_rejection = Some(rejected.index);
        }
        let digest = catalog_digest(&request_headers, &catalog)?;
        Ok(Self {
            connection_id: connection_id.to_owned(),
            endpoint: catalog.endpoint,
            request_headers,
            protocol_version: catalog.protocol_version,
            adapter_revision: catalog.adapter_revision,
            digest,
            tools: catalog.tools,
            rejected_tools: catalog.rejected_tools,
        })
    }
}

pub(crate) fn resolve_adapter(path: &Path) -> Result<PathBuf, McpAdapterError> {
    let resolved = std::fs::canonicalize(path).map_err(McpAdapterError::Resolve)?;
    if !resolved.is_file() {
        return Err(McpAdapterError::NotFile(resolved));
    }
    Ok(resolved)
}

pub(crate) fn validate_identity(kind: &str, value: &str) -> Result<(), McpHostError> {
    if value.is_empty()
        || value.len() > MAX_TOOL_NAME_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(McpHostError::Invalid(format!(
            "{kind} id must be 1-{MAX_TOOL_NAME_BYTES} ASCII letters, digits, '_', '-', or '.'"
        )));
    }
    Ok(())
}

pub(crate) fn validate_endpoint(endpoint: &str) -> Result<(), McpHostError> {
    if endpoint.is_empty() || endpoint.len() > MAX_ENDPOINT_BYTES {
        return Err(McpHostError::Invalid(format!(
            "endpoint must be 1-{MAX_ENDPOINT_BYTES} UTF-8 bytes"
        )));
    }
    Ok(())
}

fn validate_tool(tool: &McpCatalogTool) -> Result<(), McpHostError> {
    validate_identity("tool", tool.name())?;
    if tool.description.len() > MAX_DESCRIPTION_BYTES {
        return Err(McpHostError::Invalid(
            "tool description exceeds its bound".to_owned(),
        ));
    }
    validate_schema("input", &tool.input_schema)?;
    validate_schema("model input", &tool.model_input_schema)?;
    if let Some(output) = &tool.output_schema {
        validate_schema("output", output)?;
    }
    Ok(())
}

fn validate_schema(kind: &str, schema: &Value) -> Result<(), McpHostError> {
    if !schema.is_object() {
        return Err(McpHostError::Invalid(format!(
            "{kind} tool schema must be a JSON object"
        )));
    }
    if serde_json::to_vec(schema)?.len() > MAX_SCHEMA_BYTES {
        return Err(McpHostError::Invalid(format!(
            "{kind} tool schema exceeds {MAX_SCHEMA_BYTES} bytes"
        )));
    }
    Ok(())
}

fn catalog_digest(
    request_headers: &McpRequestHeaders,
    catalog: &AdapterCatalog,
) -> Result<String, serde_json::Error> {
    #[derive(Serialize)]
    struct DigestCatalog<'a> {
        endpoint: &'a str,
        request_headers: &'a McpRequestHeaders,
        protocol_version: &'a str,
        adapter_revision: &'a str,
        tools: &'a [McpCatalogTool],
        rejected_tools: &'a [McpRejectedTool],
    }

    let encoded = serde_json::to_vec(&DigestCatalog {
        endpoint: &catalog.endpoint,
        request_headers,
        protocol_version: &catalog.protocol_version,
        adapter_revision: &catalog.adapter_revision,
        tools: &catalog.tools,
        rejected_tools: &catalog.rejected_tools,
    })?;
    Ok(hex_sha256(&encoded))
}

fn hex_sha256(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            output.push(char::from(HEX[usize::from(byte >> 4)]));
            output.push(char::from(HEX[usize::from(byte & 0x0f)]));
            output
        })
}
