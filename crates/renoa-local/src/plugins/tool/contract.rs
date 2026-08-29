use std::path::{Path, PathBuf};

use renoa_agent::{ToolError, ToolSpec};
use serde::Deserialize;
use serde_json::json;

use crate::plugins::{ExtensionSource, PluginCredential, PluginOAuthRegistration, RemoteMcpSource};

pub(super) fn manage_tool_spec(name: &str) -> ToolSpec {
    ToolSpec {
        name: name.to_owned(),
        description: "Search and exactly inspect publisher metadata from the official MCP Registry, or add, inspect, install, list, connect, authorize, and disconnect extensions through Renoa Host. List reports package integrity separately from each durable connection's catalog and Alpha availability. Disconnect removes a connection from Alpha without deleting its durable registration or catalog. Registry search is discovery only: publication verifies control of the publisher namespace, not provider endorsement, metadata accuracy, server safety, or endpoint behavior. Never install directly from a search result. Call lookup for one exact version, then verify its endpoint and authentication against the provider's official HTTPS documentation before add. Add accepts that independently researched MCP definition or a local Agent Plugins 1.0 package. Credential arguments are references only: this tool never accepts or displays API keys, client secrets, tokens, or authorization codes. It has no credential-entry UI and cannot create a referenced API key or pre-registered client. Never ask the user to paste credential material into chat or tool arguments. If a required reference is missing, report the Host's setup instructions and stop. Renoa hot-loads supported skills and successful MCP connections; for OAuth, the Host owns the browser flow and stores resulting credentials outside model context.".to_owned(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["search", "lookup", "add", "inspect", "install", "list", "connect", "authorize", "disconnect"],
                    "description": "Operation to perform. Search and lookup are read-only discovery; research official provider documentation before adding an mcp source."
                },
                "query": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 256,
                    "description": "Required for search: a short human query. Renoa normalizes common multi-word names and returns as many relevant latest-version summaries as fit its explicit result boundary, at most 100; coverage reports filtering and truncation."
                },
                "registry_name": {
                    "type": "string",
                    "minLength": 3,
                    "maxLength": 200,
                    "pattern": "^[a-zA-Z0-9.-]+/[a-zA-Z0-9._-]+$",
                    "description": "Required for lookup: the exact publisher/server name returned by search."
                },
                "registry_version": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 255,
                    "not": {"const": "latest"},
                    "description": "Required for lookup: the exact published version returned by search; 'latest' is rejected."
                },
                "source": {
                    "type": "object",
                    "description": "Required for add. kind=mcp needs name, description, server, endpoint, and the provider's official documentation URL. kind=package needs source_path and the exact expected_digest returned by inspect.",
                    "properties": {
                        "kind": {
                            "type": "string",
                            "enum": ["mcp", "package"]
                        },
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
                    "description": "Required for connect, authorize, or disconnect. Optional for add; Renoa otherwise derives one stable default connection id from the installed package and server."
                },
                "credential": credential_schema(),
                "restart": {
                    "type": "boolean",
                    "description": "Only for authorize. Set true only after Renoa reports an expired, unknown, or unusable prior OAuth flow; this explicitly abandons that flow and starts again."
                },
                "replace": {
                    "type": "boolean",
                    "description": "Only for add/connect. Set true to atomically replace an existing connection whose endpoint or credential configuration is wrong. Existing tools are detached until the replacement is successfully discovered. Repeating the same replacement is harmless."
                }
            },
            "required": ["action"],
            "additionalProperties": false
        }),
    }
}

fn credential_schema() -> serde_json::Value {
    json!({
        "oneOf": [
            {
                "type": "object",
                "properties": {
                    "kind": {"const": "secret_service_bearer"},
                    "credential_id": {"type": "string"}
                },
                "required": ["kind", "credential_id"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "kind": {"const": "oauth"},
                    "registration": {
                        "oneOf": [
                            {
                                "type": "object",
                                "properties": {"mode": {"const": "dynamic"}},
                                "required": ["mode"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "mode": {"const": "client_metadata"},
                                    "url": {"type": "string"}
                                },
                                "required": ["mode", "url"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "mode": {"const": "pre_registered"},
                                    "credential_id": {"type": "string"}
                                },
                                "required": ["mode", "credential_id"],
                                "additionalProperties": false
                            }
                        ]
                    }
                },
                "required": ["kind", "registration"],
                "additionalProperties": false
            }
        ],
        "description": "Optional for add/connect. Supply only an existing Host credential reference, never raw credential material. This tool can bind the reference to a connection but cannot create the referenced API key or pre-registered client. OAuth requires the server's documented registration mode: client_metadata uses an official CIMD URL and may fall back to advertised Dynamic Client Registration; pre_registered names an existing Secret Service item containing JSON {schema_version:1,issuer,client_id,client_secret?}; dynamic requires advertised Dynamic Client Registration. Renoa derives a separate secure Host reference for tokens and runs browser authorization."
    })
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum ManageInput {
    Search {
        query: String,
    },
    Lookup {
        registry_name: String,
        registry_version: String,
    },
    Add {
        source: AddSourceInput,
        #[serde(default)]
        server: Option<String>,
        #[serde(default)]
        connection: Option<String>,
        #[serde(default)]
        credential: Option<CredentialInput>,
        #[serde(default)]
        replace: bool,
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
        #[serde(default)]
        replace: bool,
    },
    Authorize {
        connection: String,
        #[serde(default)]
        restart: bool,
    },
    Disconnect {
        connection: String,
    },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum AddSourceInput {
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
    SecretServiceBearer {
        credential_id: String,
    },
    #[serde(rename = "oauth")]
    OAuth {
        registration: OAuthRegistrationInput,
    },
}

#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum OAuthRegistrationInput {
    Dynamic,
    ClientMetadata { url: String },
    PreRegistered { credential_id: String },
}

impl From<CredentialInput> for PluginCredential {
    fn from(value: CredentialInput) -> Self {
        match value {
            CredentialInput::SecretServiceBearer { credential_id } => {
                Self::SecretServiceBearer { credential_id }
            }
            CredentialInput::OAuth { registration } => Self::OAuth {
                registration: match registration {
                    OAuthRegistrationInput::Dynamic => PluginOAuthRegistration::Dynamic,
                    OAuthRegistrationInput::ClientMetadata { url } => {
                        PluginOAuthRegistration::ClientMetadata { url }
                    }
                    OAuthRegistrationInput::PreRegistered { credential_id } => {
                        PluginOAuthRegistration::PreRegistered { credential_id }
                    }
                },
            },
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
