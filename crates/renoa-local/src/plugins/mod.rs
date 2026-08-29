mod discovery;
mod error;
mod generated;
mod inspect;
mod json;
mod manager;
mod store;
mod tool;

#[cfg(test)]
mod conformance_tests;
#[cfg(test)]
mod recovery_tests;
#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod tests;

use std::{collections::BTreeMap, path::PathBuf};

use serde::Serialize;

pub(crate) use discovery::OfficialRegistry;
pub use error::PluginError;
pub(crate) use manager::PluginManager;
pub(crate) use tool::alpha_plugin_binding;

pub(crate) const PLUGIN_STORE_DIRECTORY: &str = "plugins";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ExtensionSource {
    Mcp(RemoteMcpSource),
    Package {
        path: PathBuf,
        expected_digest: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteMcpSource {
    name: String,
    description: String,
    server: String,
    endpoint: String,
    documentation: String,
    public_headers: Vec<(String, String)>,
}

impl RemoteMcpSource {
    pub(crate) fn new(
        name: String,
        description: String,
        server: String,
        endpoint: String,
        documentation: String,
        public_headers: Vec<(String, String)>,
    ) -> Self {
        Self {
            name,
            description,
            server,
            endpoint,
            documentation,
            public_headers,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ExtensionConnectionRequest {
    id: Option<String>,
    server: Option<String>,
    credential: PluginCredential,
    replace: bool,
}

impl ExtensionConnectionRequest {
    pub(crate) const fn new(
        id: Option<String>,
        server: Option<String>,
        credential: PluginCredential,
        replace: bool,
    ) -> Self {
        Self {
            id,
            server,
            credential,
            replace,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ExtensionAddRequest {
    source: ExtensionSource,
    connection: Option<ExtensionConnectionRequest>,
}

impl ExtensionAddRequest {
    pub(crate) const fn new(
        source: ExtensionSource,
        connection: Option<ExtensionConnectionRequest>,
    ) -> Self {
        Self { source, connection }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PluginMetadata {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    homepage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    repository: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    license: Option<String>,
}

impl PluginMetadata {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    #[must_use]
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    #[must_use]
    pub fn homepage(&self) -> Option<&str> {
        self.homepage.as_deref()
    }

    #[must_use]
    pub fn repository(&self) -> Option<&str> {
        self.repository.as_deref()
    }

    #[must_use]
    pub fn license(&self) -> Option<&str> {
        self.license.as_deref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PluginMcpServer {
    id: String,
    endpoint: String,
    request_headers: BTreeMap<String, String>,
}

impl PluginMcpServer {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    #[must_use]
    pub fn request_headers(&self) -> &BTreeMap<String, String> {
        &self.request_headers
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PluginNotice {
    component: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    entry: Option<String>,
    reason: String,
}

impl PluginNotice {
    fn new(component: &str, entry: Option<String>, reason: impl Into<String>) -> Self {
        Self {
            component: component.to_owned(),
            entry,
            reason: reason.into(),
        }
    }

    #[must_use]
    pub fn component(&self) -> &str {
        &self.component
    }

    #[must_use]
    pub fn entry(&self) -> Option<&str> {
        self.entry.as_deref()
    }

    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PluginInspection {
    digest: String,
    metadata: PluginMetadata,
    mcp_servers: Vec<PluginMcpServer>,
    notices: Vec<PluginNotice>,
}

impl PluginInspection {
    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    #[must_use]
    pub const fn metadata(&self) -> &PluginMetadata {
        &self.metadata
    }

    #[must_use]
    pub fn mcp_servers(&self) -> &[PluginMcpServer] {
        &self.mcp_servers
    }

    #[must_use]
    pub fn notices(&self) -> &[PluginNotice] {
        &self.notices
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InstalledPlugin {
    digest: String,
    metadata: PluginMetadata,
    mcp_servers: Vec<PluginMcpServer>,
    notices: Vec<PluginNotice>,
}

#[derive(Debug, Serialize)]
pub struct PluginListReport {
    installed: Vec<InstalledPlugin>,
    rejected: Vec<PluginListRejection>,
}

impl PluginListReport {
    fn new(installed: Vec<InstalledPlugin>, rejected: Vec<PluginListRejection>) -> Self {
        Self {
            installed,
            rejected,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct PluginListRejection {
    package_digest: String,
    reason: String,
}

impl InstalledPlugin {
    fn from_inspection(inspection: PluginInspection) -> Self {
        Self {
            digest: inspection.digest,
            metadata: inspection.metadata,
            mcp_servers: inspection.mcp_servers,
            notices: inspection.notices,
        }
    }

    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    #[must_use]
    pub const fn metadata(&self) -> &PluginMetadata {
        &self.metadata
    }

    #[must_use]
    pub fn mcp_servers(&self) -> &[PluginMcpServer] {
        &self.mcp_servers
    }

    #[must_use]
    pub fn notices(&self) -> &[PluginNotice] {
        &self.notices
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginCredential {
    None,
    SecretServiceBearer {
        credential_id: String,
    },
    OAuth {
        registration: PluginOAuthRegistration,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginOAuthRegistration {
    Dynamic,
    ClientMetadata { url: String },
    PreRegistered { credential_id: String },
}

#[derive(Debug)]
struct CapturedPlugin {
    tree: crate::package_tree::CapturedTree,
    inspection: PluginInspection,
}
