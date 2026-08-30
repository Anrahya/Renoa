use std::collections::BTreeMap;

use rusqlite::params;

use super::{SkillComponentRejection, SkillComponentReport, SkillSourceReport, SkillStore};
use crate::skills::SkillError;

impl SkillStore {
    pub(crate) fn plugin_source_reports(
        &self,
        profile_id: &str,
    ) -> Result<Vec<SkillSourceReport>, SkillError> {
        let connection = self.connection()?;
        let mut reports = BTreeMap::<String, SkillComponentReport>::new();

        let mut accepted = connection.prepare(
            "SELECT source_id, skill_name
             FROM profile_skill_bindings
             WHERE profile_id = ?1 AND scope_kind = 'plugin'
             ORDER BY source_id, skill_name",
        )?;
        let rows = accepted.query_map([profile_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (source, skill) = row?;
            reports
                .entry(source)
                .or_insert_with(|| SkillComponentReport::new(Vec::new(), Vec::new()))
                .accepted
                .push(skill);
        }
        drop(accepted);

        let mut rejected = connection.prepare(
            "SELECT source_id, entry_name, reason
             FROM skill_source_rejections
             WHERE profile_id = ?1 AND scope_kind = 'plugin'
             ORDER BY source_id, entry_name, reason",
        )?;
        let rows = rejected.query_map(params![profile_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (source, entry, reason) = row?;
            reports
                .entry(source)
                .or_insert_with(|| SkillComponentReport::new(Vec::new(), Vec::new()))
                .rejected
                .push(SkillComponentRejection::new(entry, reason));
        }

        Ok(reports
            .into_iter()
            .map(|(source, components)| SkillSourceReport::new(source, components))
            .collect())
    }
}
