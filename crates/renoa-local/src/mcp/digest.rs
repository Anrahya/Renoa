use serde::Serialize;
use sha2::{Digest as _, Sha256};

use super::{AdapterCatalog, McpCatalogTool, McpRejectedTool, McpRequestHeaders};

pub(super) fn catalog_digest(
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

pub(super) fn headerless_catalog_digest(
    catalog: &AdapterCatalog,
) -> Result<String, serde_json::Error> {
    #[derive(Serialize)]
    struct DigestCatalog<'a> {
        endpoint: &'a str,
        protocol_version: &'a str,
        adapter_revision: &'a str,
        tools: &'a [McpCatalogTool],
        rejected_tools: &'a [McpRejectedTool],
    }

    let encoded = serde_json::to_vec(&DigestCatalog {
        endpoint: &catalog.endpoint,
        protocol_version: &catalog.protocol_version,
        adapter_revision: &catalog.adapter_revision,
        tools: &catalog.tools,
        rejected_tools: &catalog.rejected_tools,
    })?;
    Ok(hex_sha256(&encoded))
}

pub(crate) fn hex_sha256(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            output.push(char::from(HEX[usize::from(byte >> 4)]));
            output.push(char::from(HEX[usize::from(byte & 0x0f)]));
            output
        })
}
