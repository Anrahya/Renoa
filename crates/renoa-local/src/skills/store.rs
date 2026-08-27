use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use renoa_kernel::{CommandId, SessionId};
use rusqlite::{Connection, OptionalExtension as _, Transaction, TransactionBehavior, params};

use super::{
    MAX_ACTIVE_SKILL_INSTRUCTION_BYTES, MAX_ACTIVE_SKILLS, SkillError,
    package::{self, CapturedSkill, OwnedSkill, SourceSnapshot},
    registry::{SkillReference, SkillScope, SkillSummary},
    render,
};

#[derive(Clone)]
pub(crate) struct SkillStore {
    database: PathBuf,
    packages: PathBuf,
    global_source: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SkillSyncReport {
    pub(crate) available: usize,
    pub(crate) rejected: usize,
}

struct SourceSpec {
    scope: SkillScope,
    workspace: Option<String>,
    root: PathBuf,
}

struct PreparedSource {
    spec: SourceSpec,
    snapshot: SourceSnapshot,
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

    pub(super) fn sync(
        &self,
        profile_id: &str,
        workspace: &Path,
    ) -> Result<SkillSyncReport, SkillError> {
        let workspace = path_text(workspace, "workspace")?;
        let mut specs = Vec::new();
        if let Some(root) = &self.global_source {
            specs.push(SourceSpec {
                scope: SkillScope::Global,
                workspace: None,
                root: root.clone(),
            });
        }
        specs.push(SourceSpec {
            scope: SkillScope::Workspace,
            workspace: Some(workspace.clone()),
            root: Path::new(&workspace).join(".agents/skills"),
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
        let available = self.summaries(profile_id, Path::new(&workspace))?.len();
        let rejected = self.rejection_count(profile_id, Path::new(&workspace))?;
        Ok(SkillSyncReport {
            available,
            rejected,
        })
    }

    pub(super) fn summaries(
        &self,
        profile_id: &str,
        workspace: &Path,
    ) -> Result<Vec<SkillSummary>, SkillError> {
        let workspace = path_text(workspace, "workspace")?;
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT binding.scope_kind, revision.name, revision.description,
                    revision.skill_digest
             FROM profile_skill_bindings AS binding
             JOIN skill_revisions AS revision
               ON revision.skill_digest = binding.skill_digest
             WHERE binding.profile_id = ?1
               AND (
                    (binding.scope_kind = 'global' AND binding.workspace IS NULL)
                    OR
                    (binding.scope_kind = 'workspace' AND binding.workspace = ?2)
               )
             ORDER BY binding.scope_kind DESC, revision.name, revision.skill_digest",
        )?;
        let rows = statement.query_map(params![profile_id, workspace], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        let mut observed = HashSet::new();
        let mut summaries = Vec::new();
        for row in rows {
            let (scope, name, description, digest) = row?;
            if observed.insert(digest.clone()) {
                summaries.push(SkillSummary {
                    scope: SkillScope::from_stored(&scope)?,
                    name,
                    description,
                    digest,
                });
            }
        }
        Ok(summaries)
    }

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
        reference: &SkillReference,
    ) -> Result<OwnedSkill, SkillError> {
        let workspace = path_text(workspace, "workspace")?;
        let session_id = session_id.to_string();
        let loaded = package::load_owned(&self.packages, reference.digest())?;
        if loaded.metadata.name != reference.name() {
            return Err(SkillError::Conflict(format!(
                "installed skill name `{}` differs from reference `{}`",
                loaded.metadata.name,
                reference.name()
            )));
        }
        let instruction_bytes = render::one(&loaded)?.len();
        let stored_instruction_bytes = i64::try_from(instruction_bytes).map_err(|error| {
            SkillError::Invalid(format!("skill instruction size is invalid: {error}"))
        })?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let eligible = reference_is_eligible_or_active(
            &transaction,
            profile_id,
            &workspace,
            &session_id,
            reference,
        )?;
        if !eligible {
            return Err(SkillError::NotFound(format!(
                "{reference} is stale or is not selected for this workspace"
            )));
        }
        let active_digest = transaction
            .query_row(
                "SELECT skill_digest FROM session_skills
                 WHERE session_id = ?1 AND skill_name = ?2",
                params![session_id, reference.name()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if active_digest
            .as_deref()
            .is_some_and(|digest| digest != reference.digest())
        {
            return Err(SkillError::Conflict(format!(
                "session already pins a different revision of skill `{}`",
                reference.name()
            )));
        }
        let already_active = active_digest.is_some();
        if !already_active {
            enforce_activation_limits(&transaction, &session_id, instruction_bytes)?;
            transaction.execute(
                "INSERT INTO session_skills(
                    session_id, activation_command_id, skill_name, skill_digest,
                    instruction_bytes
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    session_id,
                    command_id.to_string(),
                    reference.name(),
                    reference.digest(),
                    stored_instruction_bytes,
                ],
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

fn replace_source(
    transaction: &Transaction<'_>,
    profile_id: &str,
    source: &PreparedSource,
) -> Result<(), SkillError> {
    let root = path_text(&source.spec.root, "skill source")?;
    transaction.execute(
        "DELETE FROM profile_skill_bindings WHERE profile_id = ?1 AND source_root = ?2",
        params![profile_id, root],
    )?;
    transaction.execute(
        "DELETE FROM skill_source_rejections WHERE profile_id = ?1 AND source_root = ?2",
        params![profile_id, root],
    )?;
    for skill in &source.snapshot.skills {
        ensure_revision(transaction, skill)?;
        transaction.execute(
            "INSERT INTO profile_skill_bindings(
                profile_id, scope_kind, workspace, source_root, skill_name, skill_digest
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                profile_id,
                source.spec.scope.as_str(),
                source.spec.workspace.as_deref(),
                root,
                skill.metadata.name,
                skill.digest,
            ],
        )?;
    }
    for rejection in &source.snapshot.rejections {
        transaction.execute(
            "INSERT INTO skill_source_rejections(
                profile_id, scope_kind, workspace, source_root, entry_name, reason
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                profile_id,
                source.spec.scope.as_str(),
                source.spec.workspace.as_deref(),
                root,
                rejection.entry_name,
                bounded_reason(&rejection.reason),
            ],
        )?;
    }
    Ok(())
}

fn ensure_revision(transaction: &Transaction<'_>, skill: &CapturedSkill) -> Result<(), SkillError> {
    transaction.execute(
        "INSERT OR IGNORE INTO skill_revisions(
            skill_digest, name, description, license, compatibility
         ) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            skill.digest,
            skill.metadata.name,
            skill.metadata.description,
            skill.metadata.license,
            skill.metadata.compatibility,
        ],
    )?;
    let stored = transaction.query_row(
        "SELECT name, description, license, compatibility
         FROM skill_revisions WHERE skill_digest = ?1",
        [&skill.digest],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        },
    )?;
    let expected = (
        skill.metadata.name.clone(),
        skill.metadata.description.clone(),
        skill.metadata.license.clone(),
        skill.metadata.compatibility.clone(),
    );
    if stored != expected {
        return Err(SkillError::Conflict(format!(
            "skill digest {} already has different metadata",
            skill.digest
        )));
    }
    Ok(())
}

fn reference_is_eligible_or_active(
    transaction: &Transaction<'_>,
    profile_id: &str,
    workspace: &str,
    session_id: &str,
    reference: &SkillReference,
) -> Result<bool, SkillError> {
    Ok(transaction
        .query_row(
            "SELECT 1
             WHERE EXISTS (
                SELECT 1 FROM profile_skill_bindings
                WHERE profile_id = ?1
                  AND skill_name = ?2
                  AND skill_digest = ?3
                  AND (
                       (scope_kind = 'global' AND workspace IS NULL)
                       OR
                       (scope_kind = 'workspace' AND workspace = ?4)
                  )
             ) OR EXISTS (
                SELECT 1 FROM session_skills
                WHERE session_id = ?5 AND skill_name = ?2 AND skill_digest = ?3
             )",
            params![
                profile_id,
                reference.name(),
                reference.digest(),
                workspace,
                session_id,
            ],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn enforce_activation_limits(
    transaction: &Transaction<'_>,
    session_id: &str,
    added_instruction_bytes: usize,
) -> Result<(), SkillError> {
    let (count, bytes) = transaction.query_row(
        "SELECT count(*), coalesce(sum(instruction_bytes), 0)
         FROM session_skills WHERE session_id = ?1",
        [session_id],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    )?;
    let count = usize::try_from(count)
        .map_err(|error| SkillError::Invalid(format!("active skill count is invalid: {error}")))?;
    let bytes = usize::try_from(bytes)
        .map_err(|error| SkillError::Invalid(format!("active skill size is invalid: {error}")))?;
    if count >= MAX_ACTIVE_SKILLS {
        return Err(SkillError::Conflict(format!(
            "session already has the {MAX_ACTIVE_SKILLS}-skill activation limit"
        )));
    }
    if bytes.saturating_add(added_instruction_bytes) > MAX_ACTIVE_SKILL_INSTRUCTION_BYTES {
        return Err(SkillError::Conflict(format!(
            "active skill instructions would exceed {MAX_ACTIVE_SKILL_INSTRUCTION_BYTES} bytes"
        )));
    }
    Ok(())
}

fn bounded_reason(reason: &str) -> String {
    const LIMIT: usize = 1_024;
    if reason.chars().count() <= LIMIT {
        return reason.to_owned();
    }
    let mut bounded = reason.chars().take(LIMIT - 1).collect::<String>();
    bounded.push('…');
    bounded
}

fn path_text(path: &Path, kind: &str) -> Result<String, SkillError> {
    path.to_str().map(str::to_owned).ok_or_else(|| {
        SkillError::Invalid(format!("{kind} path `{}` is not UTF-8", path.display()))
    })
}

#[cfg(test)]
mod tests;
