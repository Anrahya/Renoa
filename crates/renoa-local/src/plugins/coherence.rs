use std::collections::BTreeSet;

use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use url::Url;

use super::{CapturedPlugin, PluginError, json};

/// A Host-reviewed provider family and its exact MCP origins. Origins may be
/// added to a family but never reassigned by this API. Portable package
/// metadata cannot create or modify these rules.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginProviderFamily {
    pub family: String,
    pub origins: Vec<String>,
}

pub(crate) fn initialize(transaction: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS host_plugin_provider_families (
            family TEXT PRIMARY KEY CHECK (length(family) BETWEEN 1 AND 128)
         ) STRICT;
         CREATE TABLE IF NOT EXISTS host_plugin_provider_origins (
            origin TEXT PRIMARY KEY,
            family TEXT NOT NULL REFERENCES host_plugin_provider_families(family)
         ) STRICT;",
    )
}

pub(crate) fn define(
    database: &std::path::Path,
    rule: &PluginProviderFamily,
) -> Result<(), PluginError> {
    if rule.family.is_empty()
        || rule.family.len() > 128
        || !rule
            .family
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_'))
        || rule.origins.is_empty()
        || rule.origins.len() > 1_024
    {
        return Err(PluginError::Invalid(
            "provider family requires a 1-128 byte ASCII identity and 1-1024 exact origins"
                .to_owned(),
        ));
    }
    let mut origins = BTreeSet::new();
    for input in &rule.origins {
        let origin = origin(input)?;
        if (input != &origin && input != &format!("{origin}/")) || !origins.insert(origin) {
            return Err(PluginError::Invalid("provider origins must be unique canonical origins without a path, query, fragment, credentials, or wildcard".to_owned()));
        }
    }
    let mut connection = crate::host::catalog::open_verified(database)?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for origin in &origins {
        if let Some(owner) = tx
            .query_row(
                "SELECT family FROM host_plugin_provider_origins WHERE origin=?1",
                [origin],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            && owner != rule.family
        {
            return Err(PluginError::Conflict(format!(
                "origin '{origin}' already belongs to provider family '{owner}'"
            )));
        }
    }
    tx.execute(
        "INSERT OR IGNORE INTO host_plugin_provider_families(family) VALUES (?1)",
        [&rule.family],
    )?;
    for origin in origins {
        tx.execute(
            "INSERT OR IGNORE INTO host_plugin_provider_origins(origin,family) VALUES (?1,?2)",
            params![origin, rule.family],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub(crate) fn require(
    connection: &Connection,
    captured: &CapturedPlugin,
) -> Result<(), PluginError> {
    let Some(file) = captured
        .tree
        .files
        .iter()
        .find(|file| file.relative == "mcp.json")
    else {
        return Ok(());
    };
    let Ok(value) = json::parse(&file.bytes, "mcp.json") else {
        return Ok(());
    };
    let Some(servers) = value
        .get("mcpServers")
        .and_then(serde_json::Value::as_object)
    else {
        return Ok(());
    };
    if servers.len() <= 1 {
        return Ok(());
    }
    let mut family = None;
    for server in servers.values() {
        let endpoint = server.get("url").and_then(serde_json::Value::as_str).ok_or_else(|| PluginError::Invalid("multi-server plugins require reviewable HTTP endpoints for every MCP entry; separate unsupported or unidentifiable entries".to_owned()))?;
        let origin = origin(endpoint)?;
        let selected = connection.query_row("SELECT family FROM host_plugin_provider_origins WHERE origin=?1", [&origin], |r| r.get::<_, String>(0)).optional()?.ok_or_else(|| PluginError::Invalid(format!("multi-server plugin needs a Host-reviewed provider family for origin '{origin}'; package names and author metadata are not proof")))?;
        if family.as_ref().is_some_and(|family| *family != selected) {
            return Err(PluginError::Invalid("plugin combines unrelated Host provider families; install them as separate plugins".to_owned()));
        }
        family = Some(selected);
    }
    Ok(())
}

fn origin(endpoint: &str) -> Result<String, PluginError> {
    crate::mcp::validate_endpoint(endpoint)?;
    let validated = super::inspect::validate_url(endpoint).map_err(PluginError::Invalid)?;
    let parsed = Url::parse(&validated).map_err(|error| PluginError::Invalid(error.to_string()))?;
    if parsed.host_str().is_some_and(|host| host.contains('*')) {
        return Err(PluginError::Invalid(
            "provider origins cannot contain wildcards".to_owned(),
        ));
    }
    Ok(parsed.origin().ascii_serialization())
}
