use std::path::{Path, PathBuf};

use renoa_agent::{ToolError, ToolSpec};
use serde::Deserialize;
use serde_json::json;

use crate::plugins::{ExtensionSource, PluginCredential, RemoteMcpSource};

pub(super) fn manage_tool_spec(name: &str) -> ToolSpec {
    ToolSpec {
        name: name.to_owned(),
        description: "Find and add extensions through one Host-owned path. Add accepts an exact catalog result, an MCP definition verified from official documentation, or a local Agent Plugins 1.0 package. Renoa installs the immutable package before attempting a connection, keeps credentials outside package data, and hot-loads supported skills and a successful MCP connection.".to_owned(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["search", "add", "inspect", "install", "list", "connect"],
                    "description": "Operation to perform. Use search, then add the exact catalog result. If no reliable catalog result exists, research official MCP documentation and add an mcp source."
                },
                "query": {
                    "type": "string",
                    "description": "Required for search: capability or provider to find, in plain language."
                },
                "source": {
                    "type": "object",
                    "description": "Required for add. kind=catalog needs candidate. kind=mcp needs name, description, server, endpoint, and documentation. kind=package needs source_path and the exact expected_digest returned by inspect.",
                    "properties": {
                        "kind": {
                            "type": "string",
                            "enum": ["catalog", "mcp", "package"]
                        },
                        "candidate": {"type": "string"},
                        "source_path": {"type": "string"},
                        "expected_digest": {"type": "string"},
                        "name": {"type": "string"},
                        "description": {"type": "string"},
                        "server": {"type": "string"},
                        "endpoint": {"type": "string"},
                        "documentation": {"type": "string"},
                        "headers": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "name": {"type": "string"},
                                    "value": {"type": "string"}
                                },
                                "required": ["name", "value"],
                                "additionalProperties": false
                            }
                        }
                    },
                    "required": ["kind"],
                    "additionalProperties": false
                },
                "source_path": {
                    "type": "string",
                    "description": "Required for inspect or install: absolute Agent Plugin directory or a path relative to the workspace."
                },
                "expected_digest": {
                    "type": "string",
                    "description": "Required for install: exact digest returned by inspect."
                },
                "package_digest": {
                    "type": "string",
                    "description": "Required for connect: installed package digest."
                },
                "server": {
                    "type": "string",
                    "description": "For connect, the MCP server id. For add, optional when the source has exactly one server."
                },
                "connection": {
                    "type": "string",
                    "description": "Required for connect. Optional for add; Renoa otherwise derives one stable default connection id from the installed package and server."
                },
                "credential": {
                    "type": "object",
                    "properties": {
                        "kind": {"const": "secret_service_bearer"},
                        "credential_id": {"type": "string"}
                    },
                    "required": ["kind", "credential_id"],
                    "additionalProperties": false,
                    "description": "Optional for connect: reference to an existing Host credential. Never include a raw secret."
                }
            },
            "required": ["action"],
            "additionalProperties": false
        }),
    }
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum ManageInput {
    Search {
        query: String,
    },
    Add {
        source: AddSourceInput,
        #[serde(default)]
        server: Option<String>,
        #[serde(default)]
        connection: Option<String>,
        #[serde(default)]
        credential: Option<CredentialInput>,
    },
    Inspect {
        source_path: PathBuf,
    },
    Install {
        source_path: PathBuf,
        expected_digest: String,
    },
    List,
    Connect {
        package_digest: String,
        server: String,
        connection: String,
        #[serde(default)]
        credential: Option<CredentialInput>,
    },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum AddSourceInput {
    Catalog {
        candidate: String,
    },
    Mcp {
        name: String,
        description: String,
        server: String,
        endpoint: String,
        documentation: String,
        #[serde(default)]
        headers: Vec<HeaderInput>,
    },
    Package {
        source_path: PathBuf,
        expected_digest: String,
    },
}

impl AddSourceInput {
    pub(super) fn into_source(self, workspace: &Path) -> Result<ExtensionSource, ToolError> {
        match self {
            Self::Catalog { candidate } => Ok(ExtensionSource::Catalog {
                reference: candidate,
            }),
            Self::Mcp {
                name,
                description,
                server,
                endpoint,
                documentation,
                headers,
            } => Ok(ExtensionSource::Mcp(RemoteMcpSource::new(
                name,
                description,
                server,
                endpoint,
                documentation,
                headers
                    .into_iter()
                    .map(|header| (header.name, header.value))
                    .collect(),
            ))),
            Self::Package {
                source_path,
                expected_digest,
            } => Ok(ExtensionSource::Package {
                path: resolve_source(workspace, source_path)?,
                expected_digest,
            }),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HeaderInput {
    name: String,
    value: String,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum CredentialInput {
    SecretServiceBearer { credential_id: String },
}

impl From<CredentialInput> for PluginCredential {
    fn from(value: CredentialInput) -> Self {
        match value {
            CredentialInput::SecretServiceBearer { credential_id } => {
                Self::SecretServiceBearer { credential_id }
            }
        }
    }
}

pub(super) fn resolve_source(workspace: &Path, source: PathBuf) -> Result<PathBuf, ToolError> {
    if source.as_os_str().is_empty() {
        return Err(ToolError::invalid_input("source_path must not be empty"));
    }
    if source.is_absolute() {
        Ok(source)
    } else {
        Ok(workspace.join(source))
    }
}
