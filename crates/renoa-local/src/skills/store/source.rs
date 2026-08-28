use std::path::PathBuf;

use rusqlite::{OptionalExtension as _, Transaction, params};

use super::{SkillComponentRejection, SkillComponentReport};
use crate::skills::{
    SkillError,
    package::{CapturedSkill, SourceSnapshot},
    registry::SkillScope,
};

pub(super) struct SourceSpec {
    pub(super) scope: SkillScope,
    pub(super) workspace: Option<String>,
    pub(super) root: PathBuf,
    pub(super) id: String,
}

pub(super) struct PreparedSource {
    pub(super) spec: SourceSpec,
    pub(super) snapshot: SourceSnapshot,
}

pub(super) fn replace_source(
    transaction: &Transaction<'_>,
    profile_id: &str,
    source: &PreparedSource,
) -> Result<SkillComponentReport, SkillError> {
    transaction.execute(
        "DELETE FROM profile_skill_bindings WHERE profile_id = ?1 AND source_id = ?2",
        params![profile_id, source.spec.id],
    )?;
    transaction.execute(
        "DELETE FROM skill_source_rejections WHERE profile_id = ?1 AND source_id = ?2",
        params![profile_id, source.spec.id],
    )?;

    let mut accepted = Vec::new();
    let mut rejected = Vec::new();
    for skill in &source.snapshot.skills {
        if let Some(owner) = conflicting_plugin_source(transaction, profile_id, source, skill)? {
            let rejection = SkillComponentRejection::new(
                skill.metadata.name.clone(),
                format!(
                    "skill name is already provided by {owner}; Renoa does not choose silently between plugins"
                ),
            );
            record_rejection(transaction, profile_id, source, &rejection)?;
            rejected.push(rejection);
            continue;
        }
        ensure_revision(transaction, skill)?;
        transaction.execute(
            "INSERT INTO profile_skill_bindings(
                profile_id, scope_kind, workspace, source_id, skill_name, skill_digest
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                profile_id,
                source.spec.scope.as_str(),
                source.spec.workspace.as_deref(),
                source.spec.id,
                skill.metadata.name,
                skill.digest,
            ],
        )?;
        accepted.push(skill.metadata.name.clone());
    }
    for source_rejection in &source.snapshot.rejections {
        let rejection = SkillComponentRejection::new(
            source_rejection.entry_name.clone(),
            bounded_reason(&source_rejection.reason),
        );
        record_rejection(transaction, profile_id, source, &rejection)?;
        rejected.push(rejection);
    }
    rejected
        .sort_by(|left, right| (left.entry(), left.reason()).cmp(&(right.entry(), right.reason())));
    Ok(SkillComponentReport::new(accepted, rejected))
}

fn conflicting_plugin_source(
    transaction: &Transaction<'_>,
    profile_id: &str,
    source: &PreparedSource,
    skill: &CapturedSkill,
) -> Result<Option<String>, SkillError> {
    if source.spec.scope != SkillScope::Plugin {
        return Ok(None);
    }
    transaction
        .query_row(
            "SELECT source_id FROM profile_skill_bindings
             WHERE profile_id = ?1
               AND scope_kind = 'plugin'
               AND skill_name = ?2
               AND source_id != ?3
             ORDER BY source_id
             LIMIT 1",
            params![profile_id, skill.metadata.name, source.spec.id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(Into::into)
}

fn record_rejection(
    transaction: &Transaction<'_>,
    profile_id: &str,
    source: &PreparedSource,
    rejection: &SkillComponentRejection,
) -> Result<(), SkillError> {
    transaction.execute(
        "INSERT INTO skill_source_rejections(
            profile_id, scope_kind, workspace, source_id, entry_name, reason
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            profile_id,
            source.spec.scope.as_str(),
            source.spec.workspace.as_deref(),
            source.spec.id,
            rejection.entry(),
            rejection.reason(),
        ],
    )?;
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

fn bounded_reason(reason: &str) -> String {
    const LIMIT: usize = 1_024;
    if reason.chars().count() <= LIMIT {
        return reason.to_owned();
    }
    let mut bounded = reason.chars().take(LIMIT - 1).collect::<String>();
    bounded.push('…');
    bounded
}
