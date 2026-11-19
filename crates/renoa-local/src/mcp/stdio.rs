use std::{
    collections::BTreeMap,
    path::{Component, Path},
};

use serde::{Deserialize, Serialize};

use super::{McpConnectionAuth, McpHostError, McpRequestHeaders, hex_sha256};

const MAX_COMMAND_BYTES: usize = 1_024;
const MAX_ARGS: usize = 256;
const MAX_ARG_BYTES: usize = 8 * 1_024;
const MAX_USER_ENV_VARS: usize = 64;
const MAX_ENV_VARS: usize = MAX_USER_ENV_VARS + 2;
const MAX_ENV_KEY_BYTES: usize = 256;
const MAX_ENV_VALUE_BYTES: usize = 16 * 1_024;
const MAX_CWD_BYTES: usize = 1_024;

/// Canonical configuration for one local stdio MCP server process.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpStdioConfig {
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

impl McpStdioConfig {
    pub(crate) fn new(
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        cwd: Option<String>,
    ) -> Result<Self, McpHostError> {
        let config = Self {
            command,
            args,
            env,
            cwd,
        };
        config.validate()?;
        Ok(config)
    }

    pub(crate) fn validate(&self) -> Result<(), McpHostError> {
        validate_stdio_string(
            "stdio server command",
            &self.command,
            MAX_COMMAND_BYTES,
            true,
        )?;
        if self.args.len() > MAX_ARGS {
            return Err(McpHostError::Invalid(format!(
                "stdio server args exceeds {MAX_ARGS} entries"
            )));
        }
        for (index, arg) in self.args.iter().enumerate() {
            validate_stdio_string(
                &format!("stdio server args[{index}]"),
                arg,
                MAX_ARG_BYTES,
                false,
            )?;
        }
        if self.env.len() > MAX_ENV_VARS {
            return Err(McpHostError::Invalid(format!(
                "stdio server env exceeds {MAX_ENV_VARS} entries"
            )));
        }
        for (key, value) in &self.env {
            if key.is_empty()
                || key.len() > MAX_ENV_KEY_BYTES
                || key.bytes().any(|b| matches!(b, b'=' | 0 | b'\r' | b'\n'))
            {
                return Err(McpHostError::Invalid(format!(
                    "stdio server env key '{key}' is invalid"
                )));
            }
            validate_stdio_string(
                &format!("stdio server env.{key}"),
                value,
                MAX_ENV_VALUE_BYTES,
                false,
            )?;
        }
        if let Some(cwd) = &self.cwd {
            validate_stdio_string("stdio server cwd", cwd, MAX_CWD_BYTES, true)?;
        }
        Ok(())
    }

    pub(crate) fn validate_package_env_keys(&self) -> Result<(), McpHostError> {
        if self.env.len() > MAX_USER_ENV_VARS {
            return Err(McpHostError::Invalid(format!(
                "stdio server env exceeds {MAX_USER_ENV_VARS} entries"
            )));
        }
        if self
            .env
            .keys()
            .any(|name| matches!(name.as_str(), "PLUGIN_ROOT" | "PLUGIN_DATA"))
        {
            return Err(McpHostError::Invalid(
                "stdio server env cannot replace PLUGIN_ROOT or PLUGIN_DATA".to_owned(),
            ));
        }
        Ok(())
    }

    pub(crate) fn uses_plugin_data(&self) -> bool {
        let has_data = |value: &str| value.contains("${PLUGIN_DATA}");
        has_data(&self.command)
            || self.args.iter().any(|arg| has_data(arg))
            || self.env.values().any(|value| has_data(value))
            || self.cwd.as_deref().is_some_and(has_data)
    }

    pub(crate) fn encoded(&self) -> Result<String, McpHostError> {
        self.validate()?;
        Ok(serde_json::to_string(self)?)
    }

    pub(crate) fn from_stored(encoded: &str) -> Result<Self, McpHostError> {
        let config: Self = serde_json::from_str(encoded)?;
        config.validate()?;
        Ok(config)
    }

    /// Deterministic descriptor stored in `endpoint` columns and catalog
    /// digests so changing any stdio launch field invalidates stale catalogs.
    pub(crate) fn canonical_endpoint(&self) -> Result<String, McpHostError> {
        let encoded = self.encoded()?;
        Ok(format!("stdio:{}", hex_sha256(encoded.as_bytes())))
    }

    /// Resolves `${PLUGIN_ROOT}`, `${PLUGIN_DATA}`, and relative `./` `cwd`
    /// references against the installed package root and data directory.
    pub(crate) fn resolve_for_package(
        &self,
        plugin_root: &Path,
        plugin_data: &Path,
    ) -> Result<Self, McpHostError> {
        let root_str = plugin_root.to_str().ok_or_else(|| {
            McpHostError::Invalid("plugin root path is not valid UTF-8".to_owned())
        })?;
        let data_str = plugin_data.to_str().ok_or_else(|| {
            McpHostError::Invalid("plugin data path is not valid UTF-8".to_owned())
        })?;
        let expand = |value: &str| {
            value
                .replace("${PLUGIN_ROOT}", root_str)
                .replace("${PLUGIN_DATA}", data_str)
        };
        let references_vars =
            |value: &str| value.contains("${PLUGIN_ROOT}") || value.contains("${PLUGIN_DATA}");

        let command = expand(&self.command);
        let args = self.args.iter().map(|arg| expand(arg)).collect::<Vec<_>>();
        let mut env = self
            .env
            .iter()
            .map(|(k, v)| (k.clone(), expand(v)))
            .collect::<BTreeMap<_, _>>();
        let cwd = match self.cwd.as_deref() {
            None => None,
            Some(raw) => {
                let (base, segments) = parse_package_stdio_cwd(raw).ok_or_else(|| {
                    McpHostError::Invalid(
                        "stdio server cwd has an invalid Agent Plugins path form".to_owned(),
                    )
                })?;
                let mut resolved = match base {
                    PackageCwdBase::PluginRoot => plugin_root.to_path_buf(),
                    PackageCwdBase::PluginData => plugin_data.to_path_buf(),
                };
                for segment in segments {
                    resolved.push(segment);
                }
                let text = resolved.to_str().ok_or_else(|| {
                    McpHostError::Invalid("resolved stdio server cwd is not valid UTF-8".to_owned())
                })?;
                Some(text.to_owned())
            }
        };
        let uses_plugin_vars = references_vars(&self.command)
            || self.args.iter().any(|arg| references_vars(arg))
            || self.env.values().any(|value| references_vars(value))
            || self.cwd.is_some();
        if uses_plugin_vars {
            env.insert("PLUGIN_ROOT".to_owned(), root_str.to_owned());
            env.insert("PLUGIN_DATA".to_owned(), data_str.to_owned());
        }
        Self::new(command, args, env, cwd)
    }
}

pub(crate) fn validate_connection_binding(
    endpoint: &str,
    request_headers: &McpRequestHeaders,
    stdio: Option<&McpStdioConfig>,
    auth: &McpConnectionAuth,
) -> Result<(), McpHostError> {
    if let Some(stdio_config) = stdio {
        stdio_config.validate()?;
        if endpoint != stdio_config.canonical_endpoint()? {
            return Err(McpHostError::Invalid(
                "stdio MCP connection endpoint does not match its stdio configuration".to_owned(),
            ));
        }
        if !request_headers.is_empty() {
            return Err(McpHostError::Invalid(
                "stdio MCP connections cannot carry HTTP request headers".to_owned(),
            ));
        }
        if !matches!(auth, McpConnectionAuth::None) {
            return Err(McpHostError::Invalid(
                "stdio MCP connections do not accept HTTP credentials or OAuth".to_owned(),
            ));
        }
    } else if is_stdio_endpoint(endpoint) {
        return Err(McpHostError::Invalid(
            "streamable HTTP MCP connection cannot use a stdio endpoint".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn is_stdio_endpoint(endpoint: &str) -> bool {
    endpoint.strip_prefix("stdio:").is_some_and(|hex| {
        hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

pub(crate) fn valid_package_stdio_cwd(value: &str) -> bool {
    parse_package_stdio_cwd(value).is_some()
}

enum PackageCwdBase {
    PluginRoot,
    PluginData,
}

fn parse_package_stdio_cwd(value: &str) -> Option<(PackageCwdBase, Vec<&str>)> {
    if value == "${PLUGIN_ROOT}" {
        return Some((PackageCwdBase::PluginRoot, Vec::new()));
    }
    if value == "${PLUGIN_DATA}" {
        return Some((PackageCwdBase::PluginData, Vec::new()));
    }
    let (base, suffix) = if let Some(rest) = value.strip_prefix("./") {
        (PackageCwdBase::PluginRoot, rest)
    } else if let Some(rest) = value.strip_prefix("${PLUGIN_ROOT}/") {
        (PackageCwdBase::PluginRoot, rest)
    } else {
        let rest = value.strip_prefix("${PLUGIN_DATA}/")?;
        (PackageCwdBase::PluginData, rest)
    };
    if suffix.is_empty()
        || suffix.starts_with('/')
        || suffix.ends_with('/')
        || suffix.contains('\\')
        || suffix.contains("//")
        || suffix.contains("${")
        || suffix.bytes().any(|b| matches!(b, 0 | b'\r' | b'\n'))
    {
        return None;
    }
    let mut segments = Vec::new();
    for component in Path::new(suffix).components() {
        match component {
            Component::Normal(part) => segments.push(part.to_str()?),
            _ => return None,
        }
    }
    if segments.is_empty() {
        return None;
    }
    Some((base, segments))
}

fn validate_stdio_string(
    label: &str,
    value: &str,
    max_bytes: usize,
    non_empty_no_newlines: bool,
) -> Result<(), McpHostError> {
    if non_empty_no_newlines && value.is_empty() {
        return Err(McpHostError::Invalid(format!(
            "{label} must be a non-empty string"
        )));
    }
    if value.len() > max_bytes {
        return Err(McpHostError::Invalid(format!(
            "{label} exceeds {max_bytes} UTF-8 bytes"
        )));
    }
    if value.bytes().any(|b| b == 0)
        || (non_empty_no_newlines && value.bytes().any(|b| matches!(b, b'\r' | b'\n')))
    {
        return Err(McpHostError::Invalid(format!(
            "{label} contains forbidden control characters"
        )));
    }
    Ok(())
}
