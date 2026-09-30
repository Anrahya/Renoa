//! Canonical requests for the Host plugin lifecycle. Tool schemas are projections
//! of these types; callers cannot supply fields belonging to another operation.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod dispatch;
pub(crate) mod inventory;
mod progress;
mod schema;
pub use inventory::{PluginInventoryItem, PluginInventoryPage};
pub use progress::{
    PluginAuthorizationRequired, PluginCredentialKind, PluginCredentialRequired, PluginProgress,
};

pub use super::host::state::HostPluginActivation;
pub use dispatch::{PluginInvocation, PluginOutcome};
pub(crate) use schema::{manage_tool_spec, model_schema};

pub const PLUGIN_API_REVISION: &str = "renoa-plugin-api-v4";
pub const MAX_PLUGIN_PAGE: usize = 200;

/// One operation on the shared library or on the caller's agent bindings.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum PluginRequest {
    /// Add and activate a plugin for this agent, restoring its retained account selections. Mutable sources require
    /// `expected_digest` from inspect. Include connection and credential to connect it now.
    Add {
        source: PluginSource,
        /// 64 lowercase hexadecimal characters. For add/install, copy the source digest
        /// returned by inspect. For `replace_plugin`, copy the CURRENT selected `package_digest`.
        #[serde(default)]
        #[schemars(length(min = 64, max = 64), regex(pattern = "^[a-f0-9]{64}$"))]
        expected_digest: Option<String>,
        /// Exact packaged MCP server id; required when selecting among multiple servers.
        #[serde(default)]
        server: Option<String>,
        /// Stable Host connection name. Reuse it for authorize, disconnect, and enable.
        #[serde(default)]
        connection: Option<String>,
        #[serde(default)]
        credential: Option<PluginAuthentication>,
        /// Explicitly replace a conflicting connection configuration.
        #[serde(default)]
        replace: bool,
    },
    /// Inspect a source without installing, enabling, or executing it.
    Inspect { source: PluginSource },
    /// Install exactly the inspected revision into the shared library without enabling it.
    Install {
        source: PluginSource,
        /// Copy the exact 64-character lowercase hexadecimal digest returned by inspect.
        #[schemars(length(min = 64, max = 64), regex(pattern = "^[a-f0-9]{64}$"))]
        expected_digest: String,
    },
    /// List bounded package, connection, and skill facts. Pass `next_cursor` unchanged.
    List {
        #[serde(default)]
        #[schemars(length(min = 1, max = 256))]
        cursor: Option<String>,
        #[serde(default = "default_page_limit")]
        #[schemars(range(min = 1, max = MAX_PLUGIN_PAGE))]
        limit: usize,
    },
    /// Connect one installed server and enable that connection for this agent.
    Connect {
        /// Installed package revision: 64 lowercase hexadecimal characters.
        #[schemars(length(min = 64, max = 64), regex(pattern = "^[a-f0-9]{64}$"))]
        package_digest: String,
        /// Exact supported server id returned by inspection. Never guess a server name.
        server: String,
        /// Stable Host connection name, reused by authorize, disconnect, and enable.
        connection: String,
        #[serde(default)]
        credential: Option<PluginAuthentication>,
        #[serde(default)]
        replace: bool,
        /// Abandon an expired or unusable prior OAuth flow and start again.
        #[serde(default)]
        restart: bool,
        /// After `oauth_insufficient_scope`, copy the exact `required_scope` returned by Renoa.
        /// Do not translate, widen, or invent scopes. Omit for initial authorization.
        #[serde(default, deserialize_with = "deserialize_optional_oauth_scope")]
        required_scope: Option<String>,
    },
    /// Activate one installed revision for this agent. Names never replace another plugin.
    Activate {
        /// Exact installed revision from `plugin_search` or inventory.
        #[schemars(length(min = 64, max = 64), regex(pattern = "^[a-f0-9]{64}$"))]
        package_digest: String,
    },
    /// Stop future discovery and MCP resolution for this plugin. Loaded session skills remain pinned.
    Deactivate {
        /// Exact `plugin_id`, a 64-character external identity or a renoa.* Host plugin identity from activation or `plugin_search`; not the display name.
        #[schemars(
            length(min = 1, max = 64),
            regex(pattern = "^(?:[a-f0-9]{64}|renoa\\.[a-z]+)$")
        )]
        plugin_id: String,
    },
    /// Restore a previously selected plugin, including its retained account selections.
    EnablePlugin {
        /// Exact `plugin_id`, a 64-character external identity or a renoa.* Host plugin identity from activation or `plugin_search`; not the display name.
        #[schemars(
            length(min = 1, max = 64),
            regex(pattern = "^(?:[a-f0-9]{64}|renoa\\.[a-z]+)$")
        )]
        plugin_id: String,
    },
    /// Replace exactly this agent's selected revision. Retains plugin identity, requires the
    /// current digest, and does not move credentials or connections to new endpoints.
    ReplacePlugin {
        /// Stable `plugin_id` from activation or `plugin_search`. Preserved across revisions.
        #[schemars(length(min = 64, max = 64), regex(pattern = "^[a-f0-9]{64}$"))]
        plugin_id: String,
        /// New installed revision; install it first without activating another identity.
        #[schemars(length(min = 64, max = 64), regex(pattern = "^[a-f0-9]{64}$"))]
        package_digest: String,
        /// Current selected `package_digest`, not the new revision's inspected digest.
        #[schemars(length(min = 64, max = 64), regex(pattern = "^[a-f0-9]{64}$"))]
        expected_digest: String,
    },
    /// Complete or renew OAuth for an existing connection, then refresh its catalog.
    Authorize {
        connection: String,
        #[serde(default)]
        restart: bool,
        /// Copy the exact `required_scope` returned by Renoa after `oauth_insufficient_scope`.
        #[serde(default, deserialize_with = "deserialize_optional_oauth_scope")]
        required_scope: Option<String>,
    },
    /// Replace this agent's settings for a compiled Host plugin. The change applies to
    /// messages sent after it. Pass {} to return to the plugin's defaults.
    ConfigurePlugin {
        /// Exact renoa.* `plugin_id` of a compiled Host plugin that takes settings.
        #[schemars(length(min = 1, max = 64), regex(pattern = "^renoa\\.[a-z]+$"))]
        plugin_id: String,
        /// The plugin's settings object, as its description states.
        settings: serde_json::Map<String, Value>,
    },
    /// Remove this agent's access to a connection while retaining its shared catalog.
    Disconnect { connection: String },
    /// Restore this agent's access to an existing connection without repeating installation.
    Enable { connection: String },
}

/// Sources all converge on a verified immutable Agent Plugin revision.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PluginSource {
    /// Reuse a verified revision already installed in the Host library.
    Installed {
        /// Exact verified package revision from local plugin search or inventory.
        #[schemars(length(min = 64, max = 64), regex(pattern = "^[a-f0-9]{64}$"))]
        package_digest: String,
    },
    /// A researched remote MCP. Verify the endpoint and authentication in official documentation.
    Mcp {
        /// Plugin name: 1-64 lowercase ASCII letters, digits, dots, or hyphens.
        /// Start and end with a letter or digit; no consecutive dots or hyphens.
        #[schemars(length(min = 1, max = 64))]
        name: String,
        /// Compact capability description from verified provider documentation.
        description: String,
        /// One stable server id for this endpoint; preserve it when connecting later.
        server: String,
        /// Exact Streamable HTTP MCP endpoint from official documentation.
        /// HTTPS is required except for local loopback endpoints.
        endpoint: String,
        /// Official HTTPS page used to verify this endpoint and its authentication.
        documentation: String,
        /// Fixed public headers only. Never pass tokens, keys, cookies, or other secrets.
        #[serde(default)]
        #[schemars(length(max = 64))]
        headers: Vec<PluginHeader>,
    },
    /// An Agent Plugins 1.0 directory. Paths may be absolute or relative to the workspace.
    Package { source_path: PathBuf },
    /// One standard Agent Skill directory containing SKILL.md and optional resources.
    Skill { source_path: PathBuf },
    /// A public GitHub repository pinned to a full immutable commit. Select a plugin or
    /// skill directory with path; omit path for the repository root. No repository code runs.
    Github {
        /// Canonical HTTPS repository URL, such as <https://github.com/owner/repo>.
        repository: String,
        /// Full 40-character lowercase hexadecimal commit SHA. Branches and tags are rejected.
        #[schemars(length(min = 40, max = 40), regex(pattern = "^[a-f0-9]{40}$"))]
        commit: String,
        /// Relative directory inside the repository. No absolute paths, parent traversal, or symlinks.
        #[serde(default)]
        path: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PluginHeader {
    /// Public header name. Authorization, cookies, and credential-bearing headers are rejected.
    pub name: String,
    /// Fixed public value only; never a credential or environment variable reference.
    pub value: String,
}

/// Authentication references, never credential material. For browser sign-in pass
/// exactly {"kind":"oauth"}; Renoa discovers and validates the OAuth setup.
/// Static modes `secret_service_bearer` and `secret_service_header` require a stable Host `credential_id` reference. A missing reference
/// can emit a secure setup link on a configured headless Host.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PluginAuthentication {
    /// Static Bearer token; pass only a stable Host `credential_id` reference.
    SecretServiceBearer {
        /// Stable Host credential reference, never the token itself. Missing secrets can prompt secure setup.
        #[schemars(length(min = 1, max = 128))]
        credential_id: String,
    },
    /// Static API key through `secret_service_header`; pass its saved reference and header name.
    SecretServiceHeader {
        /// Stable Host credential reference, never the API key itself.
        #[schemars(length(min = 1, max = 128))]
        credential_id: String,
        /// Exact HTTP header required by the service, such as x-api-key.
        header: String,
        /// Exact public prefix before the secret, including any trailing space. Omit for no prefix.
        #[serde(default)]
        prefix: String,
    },
    /// Browser sign-in. Renoa discovers and validates the OAuth configuration.
    #[serde(rename = "oauth")]
    OAuth {},
}

impl From<PluginAuthentication> for super::PluginCredential {
    fn from(value: PluginAuthentication) -> Self {
        match value {
            PluginAuthentication::SecretServiceBearer { credential_id } => {
                Self::SecretServiceBearer { credential_id }
            }
            PluginAuthentication::SecretServiceHeader {
                credential_id,
                header,
                prefix,
            } => Self::SecretServiceHeader {
                credential_id,
                header,
                prefix,
            },
            PluginAuthentication::OAuth {} => Self::OAuth {
                registration: super::PluginOAuthRegistration::Auto,
            },
        }
    }
}

/// Complete discriminated JSON Schema for API clients; unlike the model projection,
/// this schema expresses the fields permitted and required by each operation.
#[must_use]
pub fn plugin_api_schema() -> Value {
    let settings = schemars::generate::SchemaSettings::draft2020_12().with(|settings| {
        settings.inline_subschemas = true;
    });
    settings
        .into_generator()
        .into_root_schema_for::<PluginRequest>()
        .to_value()
}

const fn default_page_limit() -> usize {
    MAX_PLUGIN_PAGE
}

fn deserialize_optional_oauth_scope<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    let value = Option::<String>::deserialize(deserializer)?;
    if let Some(scope) = value.as_deref() {
        crate::mcp::validate_oauth_scope(scope).map_err(serde::de::Error::custom)?;
    }
    Ok(value)
}

#[cfg(test)]
mod tests;
