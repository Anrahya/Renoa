use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use renoa_kernel::{CommandId, SessionId};
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};
use serde::Serialize;

use super::{
    SkillError,
    package::{self, OwnedSkill, RejectedSkill, SourceSnapshot},
    registry::{SkillScope, SkillSummary, validate_name},
};

mod source;
mod status;

use source::{PreparedSource, SourceSpec, replace_source};

#[derive(Clone)]
pub(crate) struct SkillStore {
    database: PathBuf,
    packages: PathBuf,
    global_source: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct SkillComponentReport {
    accepted: Vec<String>,
    rejected: Vec<SkillComponentRejection>,
}

impl SkillComponentReport {
    fn new(accepted: Vec<String>, rejected: Vec<SkillComponentRejection>) -> Self {
        Self { accepted, rejected }
    }

    pub(crate) fn accepted(&self) -> &[String] {
        &self.accepted
    }

    pub(crate) fn rejected(&self) -> &[SkillComponentRejection] {
        &self.rejected
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct SkillComponentRejection {
    entry: String,
    reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct SkillSourceReport {
    source: String,
    #[serde(flatten)]
    components: SkillComponentReport,
}

impl SkillSourceReport {
    fn new(source: String, components: SkillComponentReport) -> Self {
        Self { source, components }
    }

    pub(crate) fn source(&self) -> &str {
        &self.source
    }

    pub(crate) const fn components(&self) -> &SkillComponentReport {
        &self.components
    }
}

impl SkillComponentRejection {
    fn new(entry: String, reason: String) -> Self {
        Self { entry, reason }
    }

    pub(crate) fn entry(&self) -> &str {
        &self.entry
    }

    pub(crate) fn reason(&self) -> &str {
        &self.reason
    }
}

impl SkillStore {
    pub(crate) fn initialize(
        database: PathBuf,
        packages: PathBuf,
        global_source: Option<PathBuf>,
    ) -> Result<Self, SkillError> {
        crate::host::catalog::open_verified(&database)?;
        package::initialize_store(&packages)?;
        Ok(Self {
            database,
            packages,
            global_source,
        })
    }

    pub(super) fn sync(&self, profile_id: &str, workspace: &Path) -> Result<(), SkillError> {
        let workspace = path_text(workspace, "workspace")?;
        let mut specs = Vec::new();
        if let Some(root) = &self.global_source {
            specs.push(SourceSpec {
                scope: SkillScope::Global,
                workspace: None,
                root: root.clone(),
                id: path_text(root, "global skill source")?,
            });
        }
        let root = Path::new(&workspace).join(".agents/skills");
        specs.push(SourceSpec {
            scope: SkillScope::Workspace,
            workspace: Some(workspace.clone()),
            id: path_text(&root, "workspace skill source")?,
            root,
        });

        let mut prepared = Vec::with_capacity(specs.len());
        for spec in specs {
            let snapshot = package::inspect_source(&spec.root)?;
            for skill in &snapshot.skills {
                package::publish(&self.packages, skill)?;
            }
            prepared.push(PreparedSource { spec, snapshot });
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for source in &prepared {
            replace_source(&transaction, profile_id, source)?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn sync_plugin(
        &self,
        profile_id: &str,
        plugin_name: &str,
        plugin_root: &Path,
    ) -> Result<SkillComponentReport, SkillError> {
        let root = plugin_root.join("skills");
        let snapshot = match package::inspect_source(&root) {
            Ok(snapshot) => snapshot,
            Err(SkillError::Invalid(reason) | SkillError::Conflict(reason)) => SourceSnapshot {
                skills: Vec::new(),
                rejections: vec![RejectedSkill {
                    entry_name: "skills".to_owned(),
                    reason,
                }],
            },
            Err(error) => return Err(error),
        };
        for skill in &snapshot.skills {
            package::publish(&self.packages, skill)?;
        }
        let prepared = PreparedSource {
            spec: SourceSpec {
                scope: SkillScope::Plugin,
                workspace: None,
                root,
                id: format!("agent-plugin:{plugin_name}"),
            },
            snapshot,
        };
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let report = replace_source(&transaction, profile_id, &prepared)?;
        transaction.commit()?;
        Ok(report)
    }

    pub(crate) fn summaries(
        &self,
        profile_id: &str,
        workspace: &Path,
    ) -> Result<Vec<SkillSummary>, SkillError> {
        let workspace = path_text(workspace, "workspace")?;
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT binding.scope_kind, revision.name, revision.description
             FROM profile_skill_bindings AS binding
             JOIN skill_revisions AS revision
               ON revision.skill_digest = binding.skill_digest
             WHERE binding.profile_id = ?1
               AND (
                    (binding.scope_kind = 'plugin' AND binding.workspace IS NULL)
                    OR (binding.scope_kind = 'global' AND binding.workspace IS NULL)
                    OR
                    (binding.scope_kind = 'workspace' AND binding.workspace = ?2)
               )
             ORDER BY revision.name,
                      CASE binding.scope_kind
                        WHEN 'workspace' THEN 0
                        WHEN 'global' THEN 1
                        ELSE 2
                      END,
                      revision.skill_digest",
        )?;
        let rows = statement.query_map(params![profile_id, workspace], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut observed = HashSet::new();
        let mut summaries = Vec::new();
        for row in rows {
            let (scope, name, description) = row?;
            SkillScope::from_stored(&scope)?;
            if observed.insert(name.clone()) {
                summaries.push(SkillSummary { name, description });
            }
        }
        Ok(summaries)
    }

    #[cfg(test)]
    pub(super) fn rejection_count(
        &self,
        profile_id: &str,
        workspace: &Path,
    ) -> Result<usize, SkillError> {
        let workspace = path_text(workspace, "workspace")?;
        let count = self.connection()?.query_row(
            "SELECT count(*)
             FROM skill_source_rejections
             WHERE profile_id = ?1
               AND (
                    (scope_kind = 'global' AND workspace IS NULL)
                    OR
                    (scope_kind = 'workspace' AND workspace = ?2)
               )",
            params![profile_id, workspace],
            |row| row.get::<_, i64>(0),
        )?;
        usize::try_from(count).map_err(|error| {
            SkillError::Invalid(format!("skill rejection count is invalid: {error}"))
        })
    }

    pub(super) fn activate(
        &self,
        profile_id: &str,
        workspace: &Path,
        session_id: SessionId,
        command_id: CommandId,
        name: &str,
    ) -> Result<OwnedSkill, SkillError> {
        validate_name(name)?;
        let workspace = path_text(workspace, "workspace")?;
        let session_id = session_id.to_string();
        let candidate = active_or_selected_digest(
            &self.connection()?,
            profile_id,
            &workspace,
            &session_id,
            name,
        )?
        .ok_or_else(|| {
            SkillError::NotFound(format!(
                "skill `{name}` is not available for this workspace"
            ))
        })?;
        let loaded = package::load_owned(&self.packages, &candidate)?;
        if loaded.metadata.name != name {
            return Err(SkillError::Conflict(format!(
                "installed skill name `{}` differs from requested name `{name}`",
                loaded.metadata.name
            )));
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let resolved =
            active_or_selected_digest(&transaction, profile_id, &workspace, &session_id, name)?
                .ok_or_else(|| {
                    SkillError::NotFound(format!(
                        "skill `{name}` is not available for this workspace"
                    ))
                })?;
        if resolved != candidate {
            return Err(SkillError::Conflict(format!(
                "skill `{name}` changed while it was being loaded; search and try again"
            )));
        }
        let active_digest = transaction
            .query_row(
                "SELECT skill_digest FROM session_skills
                 WHERE session_id = ?1 AND skill_name = ?2",
                params![session_id, name],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let already_active = active_digest.is_some();
        if !already_active {
            transaction.execute(
                "INSERT INTO session_skills(
                    session_id, activation_command_id, skill_name, skill_digest
                 ) VALUES (?1, ?2, ?3, ?4)",
                params![session_id, command_id.to_string(), name, candidate,],
            )?;
        }
        transaction.commit()?;
        Ok(loaded)
    }

    pub(super) fn active(
        &self,
        session_id: SessionId,
        current_command_id: Option<CommandId>,
    ) -> Result<Vec<OwnedSkill>, SkillError> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT skill_digest FROM session_skills
             WHERE session_id = ?1
               AND (?2 IS NULL OR activation_command_id != ?2)
             ORDER BY activation_order",
        )?;
        let current_command_id = current_command_id.map(|command_id| command_id.to_string());
        let digests = statement
            .query_map(params![session_id.to_string(), current_command_id], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        digests
            .into_iter()
            .map(|digest| package::load_owned(&self.packages, &digest))
            .collect()
    }

    pub(crate) fn remove_session(&self, session_id: SessionId) -> Result<(), SkillError> {
        self.connection()?.execute(
            "DELETE FROM session_skills WHERE session_id = ?1",
            [session_id.to_string()],
        )?;
        Ok(())
    }

    fn connection(&self) -> Result<Connection, SkillError> {
        Ok(crate::host::catalog::open_verified(&self.database)?)
    }
}

fn active_or_selected_digest(
    connection: &Connection,
    profile_id: &str,
    workspace: &str,
    session_id: &str,
    name: &str,
) -> Result<Option<String>, SkillError> {
    let active = connection
        .query_row(
            "SELECT skill_digest FROM session_skills
             WHERE session_id = ?1 AND skill_name = ?2",
            params![session_id, name],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if active.is_some() {
        return Ok(active);
    }
    connection
        .query_row(
            "SELECT skill_digest FROM profile_skill_bindings
             WHERE profile_id = ?1
               AND skill_name = ?2
               AND (
                    (scope_kind = 'plugin' AND workspace IS NULL)
                    OR (scope_kind = 'global' AND workspace IS NULL)
                    OR
                    (scope_kind = 'workspace' AND workspace = ?3)
               )
             ORDER BY CASE scope_kind
                        WHEN 'workspace' THEN 0
                        WHEN 'global' THEN 1
                        ELSE 2
                      END,
                      skill_digest
             LIMIT 1",
            params![profile_id, name, workspace],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(Into::into)
}

fn path_text(path: &Path, kind: &str) -> Result<String, SkillError> {
    path.to_str().map(str::to_owned).ok_or_else(|| {
        SkillError::Invalid(format!("{kind} path `{}` is not UTF-8", path.display()))
    })
}

#[cfg(test)]
mod tests;
