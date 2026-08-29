use std::{collections::BTreeMap, fs, path::Path};

use url::Url;

use super::{PluginError, RemoteMcpSource, inspect};
use crate::mcp::McpRequestHeaders;

pub(super) struct GeneratedMcpPlugin {
    name: String,
    description: String,
    homepage: String,
    server: String,
    endpoint: String,
    public_headers: BTreeMap<String, String>,
}

impl GeneratedMcpPlugin {
    pub(super) fn from_researched(source: RemoteMcpSource) -> Result<Self, PluginError> {
        let documentation = Url::parse(&source.documentation).map_err(|error| {
            PluginError::Invalid(format!("MCP documentation URL is invalid: {error}"))
        })?;
        if documentation.scheme() != "https"
            || documentation.host().is_none()
            || !documentation.username().is_empty()
            || documentation.password().is_some()
            || documentation.fragment().is_some()
        {
            return Err(PluginError::Invalid(
                "MCP documentation must be HTTPS without credentials or a fragment".to_owned(),
            ));
        }
        let headers = McpRequestHeaders::new(source.public_headers)?;
        Ok(Self {
            name: source.name,
            description: source.description,
            homepage: documentation.to_string(),
            server: source.server,
            endpoint: source.endpoint,
            public_headers: headers.values().clone(),
        })
    }

    pub(super) fn server(&self) -> &str {
        &self.server
    }

    pub(super) fn write(&self, root: &Path) -> Result<(), PluginError> {
        let manifest = serde_json::json!({
            "$schema": inspect::PLUGIN_SCHEMA,
            "name": self.name,
            "description": self.description,
            "homepage": self.homepage,
        });
        let mcp = serde_json::json!({
            "$schema": inspect::MCP_SCHEMA,
            "mcpServers": {
                (&self.server): {
                    "type": "streamable-http",
                    "url": self.endpoint,
                    "headers": self.public_headers,
                }
            }
        });
        write_json(&root.join("plugin.json"), &manifest)?;
        write_json(&root.join("mcp.json"), &mcp)
    }
}

fn write_json(path: &Path, value: &serde_json::Value) -> Result<(), PluginError> {
    let bytes = serde_json::to_vec_pretty(value)?;
    fs::write(path, bytes).map_err(|source| PluginError::Io {
        action: "write generated package file",
        path: path.to_path_buf(),
        source,
    })
}
