mod manifest;
mod publish;

use std::{
    fs,
    path::{Path, PathBuf},
};

use super::{SkillError, registry::validate_digest};
use crate::package_tree::{self, CapturedFile, TreeError, TreeLimits, UnsupportedEntryPolicy};

pub(super) use publish::{initialize_store, publish};

const MAX_SOURCE_SKILLS: usize = 2_000;
const MAX_SOURCE_FILES: usize = 16_384;
const MAX_SOURCE_BYTES: u64 = 256 * 1_024 * 1_024;
const MAX_FILES: usize = 2_048;
const MAX_DEPTH: usize = 16;
const MAX_FILE_BYTES: u64 = 32 * 1_024 * 1_024;
const MAX_PACKAGE_BYTES: u64 = 64 * 1_024 * 1_024;
const SKILL_DIGEST_DOMAIN: &[u8] = b"renoa.skill.package.v1\0";
const SKILL_TREE_LIMITS: TreeLimits = TreeLimits {
    max_files: MAX_FILES,
    max_depth: MAX_DEPTH,
    max_file_bytes: MAX_FILE_BYTES,
    max_total_bytes: MAX_PACKAGE_BYTES,
    ignored_root_entries: &[],
    unsupported_entry_policy: UnsupportedEntryPolicy::Reject,
};

#[derive(Debug)]
pub(super) struct SourceSnapshot {
    pub(super) skills: Vec<CapturedSkill>,
    pub(super) rejections: Vec<RejectedSkill>,
}

#[derive(Debug)]
pub(super) struct RejectedSkill {
    pub(super) entry_name: String,
    pub(super) reason: String,
}

#[derive(Clone, Debug)]
pub(super) struct SkillMetadata {
    pub(super) name: String,
    pub(super) description: String,
    pub(super) license: Option<String>,
    pub(super) compatibility: Option<String>,
}

#[derive(Debug)]
pub(super) struct CapturedSkill {
    pub(super) digest: String,
    pub(super) metadata: SkillMetadata,
    pub(super) body: String,
    pub(super) files: Vec<CapturedFile>,
}

#[derive(Debug)]
pub(super) struct OwnedSkill {
    pub(super) root: PathBuf,
    pub(super) digest: String,
    pub(super) metadata: SkillMetadata,
    pub(super) body: String,
    pub(super) files: Vec<String>,
}

pub(super) fn inspect_source(root: &Path) -> Result<SourceSnapshot, SkillError> {
    let metadata = match fs::metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SourceSnapshot {
                skills: Vec::new(),
                rejections: Vec::new(),
            });
        }
        Err(error) => return Err(SkillError::io("inspect source", root, error)),
    };
    if !metadata.is_dir() {
        return Err(SkillError::Invalid(format!(
            "skill source `{}` is not a directory",
            root.display()
        )));
    }
    let resolved_root =
        fs::canonicalize(root).map_err(|error| SkillError::io("resolve source", root, error))?;
    let mut entries = read_dir_sorted(&resolved_root)?;
    if entries.len() > MAX_SOURCE_SKILLS {
        return Err(SkillError::Invalid(format!(
            "skill source `{}` exceeds {MAX_SOURCE_SKILLS} top-level entries",
            root.display()
        )));
    }

    let mut skills = Vec::new();
    let mut rejections = Vec::new();
    let mut source_files = 0_usize;
    let mut source_bytes = 0_u64;
    for entry in entries.drain(..) {
        let entry_name = utf8_name(&entry.path())?;
        let file_type = entry
            .file_type()
            .map_err(|error| SkillError::io("inspect source entry", entry.path(), error))?;
        if file_type.is_symlink() {
            rejections.push(RejectedSkill {
                entry_name,
                reason: "top-level skill entry is a symlink".to_owned(),
            });
            continue;
        }
        if !file_type.is_dir() {
            continue;
        }
        let skill_md = entry.path().join("SKILL.md");
        match fs::symlink_metadata(&skill_md) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                rejections.push(RejectedSkill {
                    entry_name,
                    reason: SkillError::io("inspect source SKILL.md", skill_md, error).to_string(),
                });
                continue;
            }
        }
        match capture(&entry.path(), Some(&entry_name)) {
            Ok(skill) => {
                let skill_bytes = skill.files.iter().try_fold(0_u64, |total, file| {
                    let length = u64::try_from(file.bytes.len()).map_err(|error| {
                        SkillError::Invalid(format!("skill file size is invalid: {error}"))
                    })?;
                    total.checked_add(length).ok_or_else(|| {
                        SkillError::Invalid("skill source byte count overflowed".to_owned())
                    })
                })?;
                (source_files, source_bytes) = admit_source_budget(
                    root,
                    source_files,
                    source_bytes,
                    skill.files.len(),
                    skill_bytes,
                )?;
                skills.push(skill);
            }
            Err(error) => rejections.push(RejectedSkill {
                entry_name,
                reason: error.to_string(),
            }),
        }
    }
    skills.sort_by(|left, right| left.metadata.name.cmp(&right.metadata.name));
    rejections.sort_by(|left, right| left.entry_name.cmp(&right.entry_name));
    Ok(SourceSnapshot { skills, rejections })
}

fn admit_source_budget(
    root: &Path,
    current_files: usize,
    current_bytes: u64,
    added_files: usize,
    added_bytes: u64,
) -> Result<(usize, u64), SkillError> {
    let files = current_files
        .checked_add(added_files)
        .ok_or_else(|| SkillError::Invalid("skill source file count overflowed".to_owned()))?;
    let bytes = current_bytes
        .checked_add(added_bytes)
        .ok_or_else(|| SkillError::Invalid("skill source byte count overflowed".to_owned()))?;
    if files > MAX_SOURCE_FILES || bytes > MAX_SOURCE_BYTES {
        return Err(SkillError::Invalid(format!(
            "skill source `{}` exceeds {MAX_SOURCE_FILES} files or {MAX_SOURCE_BYTES} bytes",
            root.display()
        )));
    }
    Ok((files, bytes))
}

pub(super) fn load_owned(store: &Path, expected_digest: &str) -> Result<OwnedSkill, SkillError> {
    validate_digest(expected_digest)?;
    let root = store.join(expected_digest);
    let captured = capture(&root, None)?;
    if captured.digest != expected_digest {
        return Err(SkillError::Conflict(format!(
            "installed skill `{expected_digest}` no longer matches its content digest"
        )));
    }
    let files = captured
        .files
        .iter()
        .map(|file| file.relative.clone())
        .collect();
    Ok(OwnedSkill {
        root,
        digest: captured.digest,
        metadata: captured.metadata,
        body: captured.body,
        files,
    })
}

fn capture(root: &Path, expected_name: Option<&str>) -> Result<CapturedSkill, SkillError> {
    let tree =
        package_tree::capture(root, SKILL_DIGEST_DOMAIN, SKILL_TREE_LIMITS).map_err(tree_error)?;
    if !tree.files.iter().any(|file| file.relative == "SKILL.md") {
        return Err(SkillError::Invalid(format!(
            "skill `{}` has no root SKILL.md",
            root.display()
        )));
    }
    let (metadata, body) = manifest::parse(&tree.files, expected_name)?;
    Ok(CapturedSkill {
        digest: tree.digest,
        metadata,
        body,
        files: tree.files,
    })
}

fn read_dir_sorted(path: &Path) -> Result<Vec<fs::DirEntry>, SkillError> {
    let mut entries = fs::read_dir(path)
        .map_err(|error| SkillError::io("read skill directory", path, error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| SkillError::io("read skill directory entry", path, error))?;
    entries.sort_by_key(fs::DirEntry::file_name);
    Ok(entries)
}

fn utf8_name(path: &Path) -> Result<String, SkillError> {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(str::to_owned)
        .ok_or_else(|| {
            SkillError::Invalid(format!("skill entry `{}` is not UTF-8", path.display()))
        })
}

pub(super) fn tree_error(error: TreeError) -> SkillError {
    match error {
        TreeError::Invalid(message) => SkillError::Invalid(message),
        TreeError::Conflict(message) => SkillError::Conflict(message),
        TreeError::Io {
            action,
            path,
            source,
        } => SkillError::io(action, path, source),
    }
}

#[cfg(test)]
mod tests;
