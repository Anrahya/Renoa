use std::{collections::BTreeSet, path::Path};

use serde_json::{Map, Value};
use url::{Host, Url};

use super::{
    CapturedPlugin, PluginError, PluginInspection, PluginMcpServer, PluginMetadata, PluginNotice,
    json,
};
use crate::{
    mcp::{McpRequestHeaders, validate_identity},
    package_tree::{self, TreeLimits, UnsupportedEntryPolicy},
};

pub(super) const PLUGIN_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";
pub(super) const MCP_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json";
const DIGEST_DOMAIN: &[u8] = b"renoa.agent-plugin.package.v1\0";
const TREE_LIMITS: TreeLimits = TreeLimits {
    max_files: 4_096,
    max_depth: 32,
    max_file_bytes: 32 * 1_024 * 1_024,
    max_total_bytes: 128 * 1_024 * 1_024,
    ignored_root_entries: &[".git"],
    unsupported_entry_policy: UnsupportedEntryPolicy::Skip,
};
const MAX_MCP_SERVERS: usize = 1_024;

pub(super) fn inspect(root: &Path) -> Result<CapturedPlugin, PluginError> {
    let tree =
        package_tree::capture(root, DIGEST_DOMAIN, TREE_LIMITS).map_err(PluginError::from_tree)?;
    let manifest = required_file(&tree.files, "plugin.json")?;
    let (metadata, mut notices) = parse_manifest(manifest)?;
    for entry in &tree.skipped_entries {
        notices.push(PluginNotice::new(
            "package",
            Some(entry.clone()),
            "entry is a symlink or special file and was denied",
        ));
    }
    let mcp = tree
        .files
        .iter()
        .find(|file| file.relative == "mcp.json")
        .map(|file| file.bytes.as_slice());
    let mcp_symlink = tree.skipped_entries.iter().any(|entry| entry == "mcp.json");
    let mcp_wrong_kind = tree.directories.iter().any(|entry| entry == "mcp.json");
    let mcp_servers = if mcp_symlink || mcp_wrong_kind {
        notices.push(PluginNotice::new(
            "mcp",
            None,
            "mcp.json is not a real file; MCP is disabled for this package",
        ));
        Vec::new()
    } else {
        parse_mcp(mcp, &mut notices)
    };
    notices.sort_by(|left, right| {
        (&left.component, &left.entry, &left.reason).cmp(&(
            &right.component,
            &right.entry,
            &right.reason,
        ))
    });
    Ok(CapturedPlugin {
        inspection: PluginInspection {
            digest: tree.digest.clone(),
            metadata,
            mcp_servers,
            notices,
        },
        tree,
    })
}

pub(crate) const fn digest_domain() -> &'static [u8] {
    DIGEST_DOMAIN
}

pub(crate) const fn tree_limits() -> TreeLimits {
    TREE_LIMITS
}

fn required_file<'a>(
    files: &'a [crate::package_tree::CapturedFile],
    name: &str,
) -> Result<&'a [u8], PluginError> {
    files
        .iter()
        .find(|file| file.relative == name)
        .map(|file| file.bytes.as_slice())
        .ok_or_else(|| PluginError::Invalid(format!("package has no real root {name}")))
}

fn parse_manifest(bytes: &[u8]) -> Result<(PluginMetadata, Vec<PluginNotice>), PluginError> {
    let value = json::parse(bytes, "plugin.json").map_err(PluginError::Invalid)?;
    let object = value
        .as_object()
        .ok_or_else(|| PluginError::Invalid("plugin.json must be a JSON object".to_owned()))?;
    let known = BTreeSet::from([
        "$schema",
        "name",
        "version",
        "description",
        "author",
        "homepage",
        "repository",
        "license",
        "keywords",
        "extensions",
    ]);
    let mut notices = object
        .keys()
        .filter(|key| !known.contains(key.as_str()))
        .map(|key| {
            PluginNotice::new(
                "manifest",
                Some(key.clone()),
                "unknown top-level field was ignored as required by Agent Plugins 1.0",
            )
        })
        .collect::<Vec<_>>();
    require_exact_schema(object, "plugin.json", PLUGIN_SCHEMA)?;
    let name = required_string(object, "name", "plugin.json", 64)?;
    validate_plugin_name(&name)?;
    let version = optional_string(object, "version", "plugin.json", 256)?;
    let description = optional_string(object, "description", "plugin.json", 8 * 1_024)?;
    let homepage = optional_string(object, "homepage", "plugin.json", 8 * 1_024)?;
    let repository = optional_string(object, "repository", "plugin.json", 8 * 1_024)?;
    let license = optional_string(object, "license", "plugin.json", 1_024)?;
    validate_author(object.get("author"))?;
    validate_keywords(object.get("keywords"))?;
    if object
        .get("extensions")
        .is_some_and(|extensions| !extensions.is_object())
    {
        notices.push(PluginNotice::new(
            "manifest",
            Some("extensions".to_owned()),
            "non-object extensions value was ignored as required by Agent Plugins 1.0",
        ));
    }
    Ok((
        PluginMetadata {
            name,
            version,
            description,
            homepage,
            repository,
            license,
        },
        notices,
    ))
}

fn parse_mcp(bytes: Option<&[u8]>, notices: &mut Vec<PluginNotice>) -> Vec<PluginMcpServer> {
    let Some(bytes) = bytes else {
        return Vec::new();
    };
    match parse_mcp_inner(bytes) {
        Ok((servers, rejected)) => {
            notices.extend(rejected);
            servers
        }
        Err(reason) => {
            notices.push(PluginNotice::new(
                "mcp",
                None,
                format!("MCP is disabled for this package: {reason}"),
            ));
            Vec::new()
        }
    }
}

fn parse_mcp_inner(bytes: &[u8]) -> Result<(Vec<PluginMcpServer>, Vec<PluginNotice>), String> {
    let value = json::parse(bytes, "mcp.json")?;
    let object = value
        .as_object()
        .ok_or_else(|| "mcp.json must be a JSON object".to_owned())?;
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "$schema" | "mcpServers"))
    {
        return Err("mcp.json contains an unknown top-level field".to_owned());
    }
    require_exact_schema_string(object, "mcp.json", MCP_SCHEMA)?;
    let values = object
        .get("mcpServers")
        .and_then(Value::as_object)
        .ok_or_else(|| "mcp.json.mcpServers must be an object".to_owned())?;
    if values.len() > MAX_MCP_SERVERS {
        return Err(format!("mcp.json exceeds {MAX_MCP_SERVERS} server entries"));
    }
    let mut entries = values.iter().collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(right.0));
    let mut servers = Vec::new();
    let mut notices = Vec::new();
    for (id, value) in entries {
        match parse_server(id, value) {
            Ok(ServerDisposition::Supported(server)) => servers.push(server),
            Ok(ServerDisposition::Unsupported(transport)) => notices.push(PluginNotice::new(
                "mcp",
                Some(id.clone()),
                format!("transport '{transport}' is not supported by Renoa v0"),
            )),
            Err(reason) => notices.push(PluginNotice::new("mcp", Some(id.clone()), reason)),
        }
    }
    Ok((servers, notices))
}

enum ServerDisposition {
    Supported(PluginMcpServer),
    Unsupported(String),
}

fn parse_server(id: &str, value: &Value) -> Result<ServerDisposition, String> {
    validate_identity("Agent Plugin MCP server", id).map_err(|error| error.to_string())?;
    let object = value
        .as_object()
        .ok_or_else(|| "server entry must be an object".to_owned())?;
    let transport = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| "server type must be a string".to_owned())?;
    match transport {
        "stdio" => {
            validate_stdio(object)?;
            Ok(ServerDisposition::Unsupported(transport.to_owned()))
        }
        "sse" => {
            parse_http_server(object, "sse")?;
            Ok(ServerDisposition::Unsupported(transport.to_owned()))
        }
        "streamable-http" => {
            let (endpoint, request_headers) = parse_http_server(object, "streamable-http")?;
            Ok(ServerDisposition::Supported(PluginMcpServer {
                id: id.to_owned(),
                endpoint,
                request_headers: request_headers.values().clone(),
            }))
        }
        _ => Err(format!("server type '{transport}' is unknown")),
    }
}

fn parse_http_server(
    object: &Map<String, Value>,
    transport: &str,
) -> Result<(String, McpRequestHeaders), String> {
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "type" | "url" | "headers"))
    {
        return Err(format!("{transport} server contains an unknown field"));
    }
    let endpoint = object
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{transport} server url must be a string"))?;
    let endpoint = validate_url(endpoint)?;
    let header_entries = match object.get("headers") {
        None => Vec::new(),
        Some(Value::Object(headers)) => headers
            .iter()
            .map(|(name, value)| {
                value
                    .as_str()
                    .map(|value| (name.clone(), value.to_owned()))
                    .ok_or_else(|| format!("header '{name}' must be a string"))
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err(format!("{transport} server headers must be an object")),
    };
    let request_headers =
        McpRequestHeaders::new(header_entries).map_err(|error| error.to_string())?;
    Ok((endpoint, request_headers))
}

fn validate_stdio(object: &Map<String, Value>) -> Result<(), String> {
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "type" | "command" | "args" | "env" | "cwd"))
    {
        return Err("stdio server contains an unknown field".to_owned());
    }
    if object
        .get("command")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Err("stdio server command must be a non-empty string".to_owned());
    }
    if object.get("args").is_some_and(|args| {
        args.as_array()
            .is_none_or(|args| args.iter().any(|argument| !argument.is_string()))
    }) {
        return Err("stdio server args must be an array of strings".to_owned());
    }
    if object.get("env").is_some_and(|env| {
        env.as_object().is_none_or(|env| {
            env.iter().any(|(name, value)| {
                matches!(name.as_str(), "PLUGIN_ROOT" | "PLUGIN_DATA") || !value.is_string()
            })
        })
    }) {
        return Err(
            "stdio server env must contain string values and cannot replace PLUGIN_ROOT or PLUGIN_DATA"
                .to_owned(),
        );
    }
    if object
        .get("cwd")
        .is_some_and(|cwd| cwd.as_str().is_none_or(|cwd| !valid_stdio_cwd(cwd)))
    {
        return Err("stdio server cwd has an invalid Agent Plugins path form".to_owned());
    }
    Ok(())
}

fn valid_stdio_cwd(value: &str) -> bool {
    value.starts_with("./")
        || value == "${PLUGIN_ROOT}"
        || value.starts_with("${PLUGIN_ROOT}/")
        || value == "${PLUGIN_DATA}"
        || value.starts_with("${PLUGIN_DATA}/")
}

fn validate_url(value: &str) -> Result<String, String> {
    let url = Url::parse(value).map_err(|error| format!("MCP URL is invalid: {error}"))?;
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err("MCP URL must not contain user information or a fragment".to_owned());
    }
    let host = url
        .host()
        .ok_or_else(|| "MCP URL must contain a host".to_owned())?;
    let loopback = match host {
        Host::Domain(name) => name.eq_ignore_ascii_case("localhost"),
        Host::Ipv4(address) => address.is_loopback(),
        Host::Ipv6(address) => address.is_loopback(),
    };
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err("MCP URL must use HTTPS; HTTP is allowed only for loopback".to_owned());
    }
    Ok(url.to_string())
}

fn require_exact_schema(
    object: &Map<String, Value>,
    source: &str,
    expected: &str,
) -> Result<(), PluginError> {
    require_exact_schema_string(object, source, expected).map_err(PluginError::Invalid)
}

fn require_exact_schema_string(
    object: &Map<String, Value>,
    source: &str,
    expected: &str,
) -> Result<(), String> {
    match object.get("$schema").and_then(Value::as_str) {
        Some(value) if value == expected => Ok(()),
        _ => Err(format!("{source} must target supported schema {expected}")),
    }
}

fn required_string(
    object: &Map<String, Value>,
    field: &str,
    source: &str,
    max_bytes: usize,
) -> Result<String, PluginError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= max_bytes)
        .map(str::to_owned)
        .ok_or_else(|| {
            PluginError::Invalid(format!(
                "{source}.{field} must contain 1-{max_bytes} UTF-8 bytes"
            ))
        })
}

fn optional_string(
    object: &Map<String, Value>,
    field: &str,
    source: &str,
    max_bytes: usize,
) -> Result<Option<String>, PluginError> {
    match object.get(field) {
        None => Ok(None),
        Some(Value::String(value)) if value.len() <= max_bytes => Ok(Some(value.clone())),
        Some(_) => Err(PluginError::Invalid(format!(
            "{source}.{field} must be a string of at most {max_bytes} UTF-8 bytes"
        ))),
    }
}

fn validate_plugin_name(name: &str) -> Result<(), PluginError> {
    let bytes = name.as_bytes();
    let valid = !name.contains("--")
        && !name.contains("..")
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
        });
    if valid {
        Ok(())
    } else {
        Err(PluginError::Invalid(
            "plugin.json.name must start and end with a lowercase ASCII letter or digit, contain only lowercase ASCII letters, digits, '.', or '-', and cannot contain '..' or '--' (Agent Plugins 1.0)".to_owned(),
        ))
    }
}

fn validate_author(value: Option<&Value>) -> Result<(), PluginError> {
    let Some(value) = value else {
        return Ok(());
    };
    let object = value
        .as_object()
        .ok_or_else(|| PluginError::Invalid("plugin.json.author must be an object".to_owned()))?;
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "name" | "email" | "url"))
        || object
            .values()
            .any(|value| value.as_str().is_none_or(|value| value.len() > 8 * 1_024))
    {
        return Err(PluginError::Invalid(
            "plugin.json.author does not satisfy Agent Plugins 1.0".to_owned(),
        ));
    }
    Ok(())
}

fn validate_keywords(value: Option<&Value>) -> Result<(), PluginError> {
    let Some(value) = value else {
        return Ok(());
    };
    let valid = value.as_array().is_some_and(|values| {
        values.len() <= 256
            && values
                .iter()
                .all(|value| value.as_str().is_some_and(|value| value.len() <= 256))
    });
    if valid {
        Ok(())
    } else {
        Err(PluginError::Invalid(
            "plugin.json.keywords must be a bounded array of strings".to_owned(),
        ))
    }
}
