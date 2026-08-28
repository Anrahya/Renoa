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
