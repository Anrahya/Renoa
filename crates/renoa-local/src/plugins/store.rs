use std::{
    fs,
    path::{Path, PathBuf},
};

use super::{
    CapturedPlugin, InstalledPlugin, PluginError, PluginListRejection, PluginListReport,
    PluginMcpServer, PluginMetadata, generated::GeneratedMcpPlugin, inspect,
};
use crate::{
    mcp::McpRequestHeaders,
    package_tree::{self, publish},
};
use rusqlite::{Connection, OptionalExtension as _, Transaction, TransactionBehavior, params};

#[derive(Clone)]
pub(super) struct PluginStore {
    database: PathBuf,
    packages: PathBuf,
}

impl PluginStore {
    pub(super) fn initialize(database: PathBuf, packages: PathBuf) -> Result<Self, PluginError> {
        crate::host::catalog::open_verified(&database)?;
        package_tree::initialize_store(&packages).map_err(PluginError::from_tree)?;
        Ok(Self { database, packages })
    }

    pub(super) fn install(
        &self,
        source: &Path,
        expected_digest: &str,
    ) -> Result<InstalledPlugin, PluginError> {
        validate_digest(expected_digest)?;
        if load_stored(&self.connection()?, expected_digest)?.is_some() {
            return self.load(expected_digest);
        }
        if let Some(installed) = self.recover_published(expected_digest)? {
            return Ok(installed);
        }
        let captured = inspect::inspect(source)?;
        if captured.inspection.digest != expected_digest {
            return Err(PluginError::Conflict(format!(
                "package source changed after inspection: expected {expected_digest}, found {}",
                captured.inspection.digest
            )));
        }
        self.install_captured(&captured)
    }

    pub(super) fn install_current(&self, source: &Path) -> Result<InstalledPlugin, PluginError> {
        let captured = inspect::inspect(source)?;
        self.install_captured(&captured)
    }

    fn install_captured(&self, captured: &CapturedPlugin) -> Result<InstalledPlugin, PluginError> {
        let expected_digest = captured.inspection.digest.clone();
        if load_stored(&self.connection()?, &expected_digest)?.is_some() {
            return self.load(&expected_digest);
        }
        if let Some(installed) = self.recover_published(&expected_digest)? {
            return Ok(installed);
        }
        let published = publish(
            &self.packages,
            &captured.tree,
            inspect::digest_domain(),
            inspect::tree_limits(),
        )
        .map_err(PluginError::from_tree)?;
        let installed = verify_published(&published, &expected_digest, captured)?;
        self.record(&installed)?;
        Ok(installed)
    }

    pub(super) fn install_generated(
        &self,
        generated: &GeneratedMcpPlugin,
    ) -> Result<InstalledPlugin, PluginError> {
        let staging = tempfile::Builder::new()
            .prefix(".generated-source-")
            .tempdir_in(&self.packages)
            .map_err(|source| PluginError::Io {
                action: "create generated package staging directory",
                path: self.packages.clone(),
                source,
            })?;
        let staging_path = staging.path().to_path_buf();
        let result = generated
            .write(&staging_path)
            .and_then(|()| self.install_current(&staging_path));
        let cleanup = staging.close().map_err(|source| PluginError::Io {
            action: "remove generated package staging directory",
            path: staging_path,
            source,
        });
        match (result, cleanup) {
            (Ok(installed), Ok(())) => Ok(installed),
            (Err(error), Ok(())) => Err(error),
            (Ok(_), Err(cleanup)) => Err(cleanup),
            (Err(error), Err(cleanup)) => Err(PluginError::Conflict(format!(
                "{error}; generated package staging cleanup also failed: {cleanup}"
            ))),
        }
    }

    fn recover_published(
        &self,
        expected_digest: &str,
    ) -> Result<Option<InstalledPlugin>, PluginError> {
        let target = self.packages.join(expected_digest);
        match fs::symlink_metadata(&target) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(PluginError::Io {
                    action: "inspect published package",
                    path: target,
                    source,
                });
            }
            Ok(_) => {}
        }
        let captured = inspect::inspect(&target)?;
        require_no_denied_installed_entries(&captured, expected_digest)?;
        if captured.inspection.digest != expected_digest {
            return Err(PluginError::Conflict(format!(
                "published package '{expected_digest}' differs from its content digest"
            )));
        }
        let installed = InstalledPlugin::from_inspection(captured.inspection);
        self.record(&installed)?;
        Ok(Some(installed))
    }

    fn record(&self, installed: &InstalledPlugin) -> Result<(), PluginError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_installed(&transaction, installed)?;
        transaction.commit()?;
        Ok(())
    }

    pub(super) fn load(&self, digest: &str) -> Result<InstalledPlugin, PluginError> {
        validate_digest(digest)?;
        let mut connection = self.connection()?;
        let stored = load_stored(&connection, digest)?.ok_or_else(|| {
            PluginError::NotFound(format!("package digest '{digest}' is not installed"))
        })?;
        let captured = inspect::inspect(&self.packages.join(digest))?;
        require_no_denied_installed_entries(&captured, digest)?;
        let observed = InstalledPlugin::from_inspection(captured.inspection);
        if same_durable_plugin(&observed, &stored) {
            return Ok(observed);
        }
        if differs_only_by_missing_legacy_homepage(&observed, &stored) {
            repair_legacy_homepage(&mut connection, &observed)?;
            return Ok(observed);
        }
        Err(PluginError::Conflict(format!(
            "installed package '{digest}' differs from its durable record"
        )))
    }

    pub(super) fn list(&self) -> Result<Vec<InstalledPlugin>, PluginError> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT plugin_digest FROM installed_plugins ORDER BY name, plugin_digest")?;
        let digests = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        drop(connection);
        digests
            .into_iter()
            .map(|digest| self.load(&digest))
            .collect()
    }

    pub(super) fn list_report(&self) -> Result<PluginListReport, PluginError> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT plugin_digest FROM installed_plugins ORDER BY name, plugin_digest")?;
        let digests = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        drop(connection);
        let mut installed = Vec::new();
        let mut rejected = Vec::new();
        for digest in digests {
            match self.load(&digest) {
                Ok(plugin) => installed.push(plugin),
                Err(error) => rejected.push(PluginListRejection {
                    package_digest: digest,
                    reason: error.to_string(),
                }),
            }
        }
        Ok(PluginListReport::new(installed, rejected))
    }

    pub(super) fn package_root(&self, digest: &str) -> Result<PathBuf, PluginError> {
        self.load(digest)?;
        Ok(self.packages.join(digest))
    }

    fn connection(&self) -> Result<Connection, PluginError> {
        Ok(crate::host::catalog::open_verified(&self.database)?)
    }
}

fn verify_published(
    path: &Path,
    expected_digest: &str,
    captured: &CapturedPlugin,
) -> Result<InstalledPlugin, PluginError> {
    let published = inspect::inspect(path)?;
    require_no_denied_installed_entries(&published, expected_digest)?;
    if published.inspection.digest != expected_digest
        || published.inspection.metadata != captured.inspection.metadata
        || published.inspection.mcp_servers != captured.inspection.mcp_servers
        || published.inspection.notices != captured.inspection.notices
    {
        return Err(PluginError::Conflict(format!(
            "published package '{expected_digest}' differs from the inspected source"
        )));
    }
    Ok(InstalledPlugin::from_inspection(published.inspection))
}

fn require_no_denied_installed_entries(
    captured: &CapturedPlugin,
    digest: &str,
) -> Result<(), PluginError> {
    if captured.tree.skipped_entries.is_empty() {
        Ok(())
    } else {
        Err(PluginError::Conflict(format!(
            "installed package '{digest}' contains a denied symlink or special file"
        )))
    }
}

fn ensure_installed(
    transaction: &Transaction<'_>,
    plugin: &InstalledPlugin,
) -> Result<(), PluginError> {
    let inserted = transaction.execute(
        "INSERT OR IGNORE INTO installed_plugins(
            plugin_digest, name, version, description, homepage, repository, license
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            plugin.digest,
            plugin.metadata.name,
            plugin.metadata.version,
            plugin.metadata.description,
            plugin.metadata.homepage,
            plugin.metadata.repository,
            plugin.metadata.license,
        ],
    )?;
    if inserted == 1 {
        for server in &plugin.mcp_servers {
            transaction.execute(
                "INSERT INTO plugin_mcp_servers(
                    plugin_digest, server_id, transport, endpoint, request_headers_json
                 ) VALUES (?1, ?2, 'streamable_http', ?3, ?4)",
                params![
                    plugin.digest,
                    server.id,
                    server.endpoint,
                    serde_json::to_string(&server.request_headers)?,
                ],
            )?;
        }
        return Ok(());
    }
    let stored = load_stored(transaction, &plugin.digest)?.ok_or_else(|| {
        PluginError::Conflict(format!(
            "package '{}' disappeared during installation",
            plugin.digest
        ))
    })?;
    if same_durable_plugin(&stored, plugin) {
        Ok(())
    } else {
        Err(PluginError::Conflict(format!(
            "package digest '{}' already has different durable metadata",
            plugin.digest
        )))
    }
}

fn load_stored(
    connection: &Connection,
    digest: &str,
) -> Result<Option<InstalledPlugin>, PluginError> {
    let metadata = connection
        .query_row(
            "SELECT name, version, description, homepage, repository, license
             FROM installed_plugins WHERE plugin_digest = ?1",
            [digest],
            |row| {
                Ok(PluginMetadata {
                    name: row.get(0)?,
                    version: row.get(1)?,
                    description: row.get(2)?,
                    homepage: row.get(3)?,
                    repository: row.get(4)?,
                    license: row.get(5)?,
                })
            },
        )
        .optional()?;
    let Some(metadata) = metadata else {
        return Ok(None);
    };
    let mut statement = connection.prepare(
        "SELECT server_id, transport, endpoint, request_headers_json
         FROM plugin_mcp_servers WHERE plugin_digest = ?1 ORDER BY server_id",
    )?;
    let servers = statement
        .query_map([digest], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .map(|row| {
            let (id, transport, endpoint, headers) = row?;
            if transport != "streamable_http" {
                return Err(PluginError::Conflict(format!(
                    "installed package '{digest}' has unknown transport '{transport}'"
                )));
            }
            let headers = McpRequestHeaders::from_stored(&headers)?;
            Ok(PluginMcpServer {
                id,
                endpoint,
                request_headers: headers.values().clone(),
            })
        })
        .collect::<Result<Vec<_>, PluginError>>()?;
    Ok(Some(InstalledPlugin {
        digest: digest.to_owned(),
        metadata,
        mcp_servers: servers,
        notices: Vec::new(),
    }))
}

fn same_durable_plugin(left: &InstalledPlugin, right: &InstalledPlugin) -> bool {
    left.digest == right.digest
        && left.metadata == right.metadata
        && left.mcp_servers == right.mcp_servers
}

fn differs_only_by_missing_legacy_homepage(
    observed: &InstalledPlugin,
    stored: &InstalledPlugin,
) -> bool {
    stored.metadata.homepage.is_none()
        && observed.metadata.homepage.is_some()
        && observed.digest == stored.digest
        && observed.metadata.name == stored.metadata.name
        && observed.metadata.version == stored.metadata.version
        && observed.metadata.description == stored.metadata.description
        && observed.metadata.repository == stored.metadata.repository
        && observed.metadata.license == stored.metadata.license
        && observed.mcp_servers == stored.mcp_servers
}

fn repair_legacy_homepage(
    connection: &mut Connection,
    observed: &InstalledPlugin,
) -> Result<(), PluginError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let stored = load_stored(&transaction, &observed.digest)?.ok_or_else(|| {
        PluginError::Conflict(format!(
            "package '{}' disappeared while recovering legacy metadata",
            observed.digest
        ))
    })?;
    if same_durable_plugin(observed, &stored) {
        transaction.commit()?;
        return Ok(());
    }
    if !differs_only_by_missing_legacy_homepage(observed, &stored) {
        return Err(PluginError::Conflict(format!(
            "installed package '{}' changed while recovering legacy metadata",
            observed.digest
        )));
    }
    let homepage = observed.metadata.homepage.as_deref().ok_or_else(|| {
        PluginError::Conflict(format!(
            "installed package '{}' has no homepage to recover",
            observed.digest
        ))
    })?;
    transaction.execute(
        "UPDATE installed_plugins SET homepage = ?1 WHERE plugin_digest = ?2",
        params![homepage, observed.digest],
    )?;
    transaction.commit()?;
    Ok(())
}

fn validate_digest(digest: &str) -> Result<(), PluginError> {
    if digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(PluginError::Invalid(
            "package digest must be 64 lowercase hexadecimal characters".to_owned(),
        ))
    }
}
