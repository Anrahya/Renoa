use std::{collections::BTreeMap, fs, path::Path};

use url::Url;

use super::{PluginError, RemoteMcpSource, StdioMcpSource, inspect};
use crate::mcp::{McpRequestHeaders, McpStdioConfig, valid_package_stdio_cwd};

enum GeneratedServer {
    StreamableHttp {
        endpoint: String,
        public_headers: BTreeMap<String, String>,
    },
    Stdio(McpStdioConfig),
}

pub(super) struct GeneratedMcpPlugin {
    name: String,
    description: String,
    homepage: Option<String>,
    server: String,
    transport: GeneratedServer,
}

impl GeneratedMcpPlugin {
    pub(super) fn from_researched(source: RemoteMcpSource) -> Result<Self, PluginError> {
        let documentation = validate_documentation_url(&source.documentation)?;
        let headers = McpRequestHeaders::new(source.public_headers)?;
        Ok(Self {
            name: source.name,
            description: source.description,
            homepage: Some(documentation),
            server: source.server,
            transport: GeneratedServer::StreamableHttp {
                endpoint: source.endpoint,
                public_headers: headers.values().clone(),
            },
        })
    }

    pub(super) fn from_stdio(source: StdioMcpSource) -> Result<Self, PluginError> {
        let homepage = source
            .documentation
            .as_deref()
            .map(validate_documentation_url)
            .transpose()?;
        if let Some(cwd) = &source.cwd
            && !valid_package_stdio_cwd(cwd)
        {
            return Err(PluginError::Invalid(
                "stdio server cwd must start with './', '${PLUGIN_ROOT}', or '${PLUGIN_DATA}'"
                    .to_owned(),
            ));
        }
        let stdio = McpStdioConfig::new(source.command, source.args, source.env, source.cwd)?;
        stdio.validate_package_env_keys()?;
        Ok(Self {
            name: source.name,
            description: source.description,
            homepage,
            server: source.server,
            transport: GeneratedServer::Stdio(stdio),
        })
    }

    pub(super) fn from_host(name: &str, endpoint: &str) -> Self {
        Self {
            name: name.to_owned(),
            description: format!("Host-configured {name} MCP."),
            homepage: None,
            server: "default".to_owned(),
            transport: GeneratedServer::StreamableHttp {
                endpoint: endpoint.to_owned(),
                public_headers: BTreeMap::new(),
            },
        }
    }

    pub(super) fn write(&self, root: &Path) -> Result<(), PluginError> {
        let mut manifest = serde_json::json!({
            "$schema": inspect::PLUGIN_SCHEMA,
            "name": self.name,
            "description": self.description,
        });
        if let Some(homepage) = &self.homepage {
            manifest["homepage"] = homepage.clone().into();
        }
        let server_entry = match &self.transport {
            GeneratedServer::StreamableHttp {
                endpoint,
                public_headers,
            } => serde_json::json!({
                "type": "streamable-http",
                "url": endpoint,
                "headers": public_headers,
            }),
            GeneratedServer::Stdio(stdio) => {
                let mut entry = serde_json::json!({
                    "type": "stdio",
                    "command": stdio.command,
                });
                if !stdio.args.is_empty() {
                    entry["args"] = serde_json::json!(stdio.args);
                }
                if !stdio.env.is_empty() {
                    entry["env"] = serde_json::json!(stdio.env);
                }
                if let Some(cwd) = &stdio.cwd {
                    entry["cwd"] = serde_json::json!(cwd);
                }
                entry
            }
        };
        let mcp = serde_json::json!({
            "$schema": inspect::MCP_SCHEMA,
            "mcpServers": {
                (&self.server): server_entry
            }
        });
        write_json(&root.join("plugin.json"), &manifest)?;
        write_json(&root.join("mcp.json"), &mcp)
    }
}

fn validate_documentation_url(raw: &str) -> Result<String, PluginError> {
    let documentation = Url::parse(raw).map_err(|error| {
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
    Ok(documentation.to_string())
}

fn write_json(path: &Path, value: &serde_json::Value) -> Result<(), PluginError> {
    let bytes = serde_json::to_vec_pretty(value)?;
    fs::write(path, bytes).map_err(|source| PluginError::Io {
        action: "write generated package file",
        path: path.to_path_buf(),
        source,
    })
}
