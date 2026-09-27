use std::path::{Path, PathBuf};

use super::{CapturedSkill, SKILL_DIGEST_DOMAIN, SKILL_TREE_LIMITS, load_owned, tree_error};
use crate::{
    package_tree::{self, CapturedTree},
    skills::SkillError,
};

pub(in crate::skills) fn initialize_store(path: &Path) -> Result<(), SkillError> {
    package_tree::initialize_store(path).map_err(tree_error)
}

pub(in crate::skills) fn publish(
    store: &Path,
    skill: &CapturedSkill,
) -> Result<PathBuf, SkillError> {
    let tree = CapturedTree {
        digest: skill.digest.clone(),
        files: skill.files.clone(),
        directories: Vec::new(),
        skipped_entries: Vec::new(),
    };
    let target = package_tree::publish(store, &tree, SKILL_DIGEST_DOMAIN, SKILL_TREE_LIMITS)
        .map_err(tree_error)?;
    load_owned(store, &skill.digest)?;
    Ok(target)
}

/// Skill writers hold the catalog's immediate transaction until this batch is
/// committed or dropped, so another writer cannot adopt a rolled-back target.
#[must_use]
pub(crate) struct PublicationBatch {
    store: PathBuf,
    created: Vec<PathBuf>,
}

impl PublicationBatch {
    pub(crate) fn new(store: &Path) -> Self {
        Self {
            store: store.to_owned(),
            created: Vec::new(),
        }
    }

    pub(crate) fn publish(&mut self, skills: &[&CapturedSkill]) -> Result<(), SkillError> {
        for skill in skills {
            let target = self.store.join(&skill.digest);
            match std::fs::symlink_metadata(&target) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(SkillError::Conflict(
                        "installed skill target is a symlink".to_owned(),
                    ));
                }
                Ok(_) => {
                    load_owned(&self.store, &skill.digest)?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(SkillError::io("inspect skill target", target, error)),
            }
        }
        for skill in skills {
            self.publish_one(skill)?;
        }
        Ok(())
    }

    fn publish_one(&mut self, skill: &CapturedSkill) -> Result<(), SkillError> {
        let target = self.store.join(&skill.digest);
        let existed = target
            .try_exists()
            .map_err(|error| SkillError::io("inspect skill target", &target, error))?;
        // Register ownership before publication, including a rename followed by
        // a failed freeze/fsync. Existing targets never belong to this batch.
        if !existed {
            self.created.push(target);
        }
        publish(&self.store, skill)?;
        Ok(())
    }

    pub(crate) fn commit(mut self, transaction: rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
        let result = transaction.execute_batch("COMMIT");
        if result.is_ok() {
            self.created.clear();
        }
        // On a failed commit, clean up before releasing the writer lock.
        drop(self);
        drop(transaction);
        result
    }
}

impl Drop for PublicationBatch {
    fn drop(&mut self) {
        for target in self.created.iter().rev() {
            if target.exists() {
                let _ = package_tree::remove_owned(target);
            }
        }
    }
}
