mod manifest;
mod publish;

use std::{
    fs::{self, File},
    io::Read as _,
    path::{Component, Path, PathBuf},
};

use sha2::{Digest as _, Sha256};

use super::{SkillError, registry::validate_digest};

pub(super) use publish::{initialize_store, publish};

const MAX_SOURCE_SKILLS: usize = 2_000;
const MAX_SOURCE_FILES: usize = 16_384;
const MAX_SOURCE_BYTES: u64 = 256 * 1_024 * 1_024;
const MAX_FILES: usize = 2_048;
const MAX_DEPTH: usize = 16;
const MAX_FILE_BYTES: u64 = 32 * 1_024 * 1_024;
const MAX_PACKAGE_BYTES: u64 = 64 * 1_024 * 1_024;

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
pub(super) struct CapturedFile {
    pub(super) relative: String,
    pub(super) bytes: Vec<u8>,
    pub(super) executable: bool,
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
    let files = capture_files(root)?;
    let (metadata, body) = manifest::parse(&files, expected_name)?;
    Ok(CapturedSkill {
        digest: digest(&files),
        metadata,
        body,
        files,
    })
}

fn capture_files(root: &Path) -> Result<Vec<CapturedFile>, SkillError> {
    let metadata = fs::symlink_metadata(root)
        .map_err(|error| SkillError::io("inspect skill directory", root, error))?;
    if !metadata.file_type().is_dir() {
        return Err(SkillError::Invalid(format!(
            "skill path `{}` is not a real directory",
            root.display()
        )));
    }
    let skill_md_path = root.join("SKILL.md");
    let skill_md_metadata = fs::symlink_metadata(&skill_md_path)
        .map_err(|error| SkillError::io("inspect root SKILL.md", &skill_md_path, error))?;
    if !skill_md_metadata.file_type().is_file() {
        return Err(SkillError::Invalid(format!(
            "skill `{}` root SKILL.md is not a real file",
            root.display()
        )));
    }
    let mut pending = vec![(root.to_path_buf(), 0_usize)];
    let mut files = Vec::new();
    let mut total_bytes = 0_u64;
    while let Some((directory, depth)) = pending.pop() {
        if depth > MAX_DEPTH {
            return Err(SkillError::Invalid(format!(
                "skill `{}` exceeds directory depth {MAX_DEPTH}",
                root.display()
            )));
        }
        let mut entries = read_dir_sorted(&directory)?;
        entries.reverse();
        for entry in entries {
            let path = entry.path();
            let file_type = entry
                .file_type()
                .map_err(|error| SkillError::io("inspect skill entry", &path, error))?;
            if file_type.is_symlink() {
                return Err(SkillError::Invalid(format!(
                    "skill `{}` contains symlink `{}`",
                    root.display(),
                    relative_path(root, &path)?
                )));
            }
            if file_type.is_dir() {
                pending.push((path, depth + 1));
                continue;
            }
            if !file_type.is_file() {
                return Err(SkillError::Invalid(format!(
                    "skill `{}` contains a non-file entry",
                    root.display()
                )));
            }
            if files.len() >= MAX_FILES {
                return Err(SkillError::Invalid(format!(
                    "skill `{}` exceeds {MAX_FILES} files",
                    root.display()
                )));
            }
            let (bytes, metadata) = read_bounded_file(&path)?;
            let bytes_len = u64::try_from(bytes.len()).map_err(|error| {
                SkillError::Invalid(format!("skill file size is invalid: {error}"))
            })?;
            total_bytes = total_bytes.checked_add(bytes_len).ok_or_else(|| {
                SkillError::Invalid("skill package byte count overflowed".to_owned())
            })?;
            if total_bytes > MAX_PACKAGE_BYTES {
                return Err(SkillError::Invalid(format!(
                    "skill `{}` exceeds {MAX_PACKAGE_BYTES} bytes",
                    root.display()
                )));
            }
            let relative = relative_path(root, &path)?;
            files.push(CapturedFile {
                relative,
                bytes,
                executable: executable(&metadata),
            });
        }
    }
    files.sort_by(|left, right| left.relative.cmp(&right.relative));
    Ok(files)
}

fn read_bounded_file(path: &Path) -> Result<(Vec<u8>, fs::Metadata), SkillError> {
    let file = File::open(path).map_err(|error| SkillError::io("open skill file", path, error))?;
    let metadata = file
        .metadata()
        .map_err(|error| SkillError::io("inspect opened skill file", path, error))?;
    if !metadata.is_file() {
        return Err(SkillError::Invalid(format!(
            "skill entry `{}` did not open as a regular file",
            path.display()
        )));
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| SkillError::io("read skill file", path, error))?;
    let length = u64::try_from(bytes.len())
        .map_err(|error| SkillError::Invalid(format!("skill file size is invalid: {error}")))?;
    if length > MAX_FILE_BYTES {
        return Err(SkillError::Invalid(format!(
            "skill file `{}` exceeds {MAX_FILE_BYTES} bytes",
            path.display()
        )));
    }
    Ok((bytes, metadata))
}

fn digest(files: &[CapturedFile]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"renoa.skill.package.v1\0");
    for file in files {
        hasher.update((file.relative.len() as u64).to_be_bytes());
        hasher.update(file.relative.as_bytes());
        hasher.update([u8::from(file.executable)]);
        hasher.update((file.bytes.len() as u64).to_be_bytes());
        hasher.update(&file.bytes);
    }
    hex(hasher.finalize().as_slice())
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn read_dir_sorted(path: &Path) -> Result<Vec<fs::DirEntry>, SkillError> {
    let mut entries = fs::read_dir(path)
        .map_err(|error| SkillError::io("read skill directory", path, error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| SkillError::io("read skill directory entry", path, error))?;
    entries.sort_by_key(fs::DirEntry::file_name);
    Ok(entries)
}

fn relative_path(root: &Path, path: &Path) -> Result<String, SkillError> {
    let relative = path.strip_prefix(root).map_err(|_| {
        SkillError::Invalid(format!(
            "skill path `{}` escaped `{}`",
            path.display(),
            root.display()
        ))
    })?;
    relative
        .components()
        .map(|component| match component {
            Component::Normal(value) => value.to_str().map(str::to_owned).ok_or_else(|| {
                SkillError::Invalid(format!("skill path `{}` is not UTF-8", path.display()))
            }),
            _ => Err(SkillError::Invalid(format!(
                "skill path `{}` is not contained",
                path.display()
            ))),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|parts| parts.join("/"))
}

fn utf8_name(path: &Path) -> Result<String, SkillError> {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(str::to_owned)
        .ok_or_else(|| {
            SkillError::Invalid(format!("skill entry `{}` is not UTF-8", path.display()))
        })
}

#[cfg(unix)]
fn executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn executable(_metadata: &fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests;
