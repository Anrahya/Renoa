use std::{
    fs::{self, File, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
};

use uuid::Uuid;

use super::{CapturedFile, CapturedSkill, load_owned};
use crate::skills::SkillError;

pub(in crate::skills) fn initialize_store(path: &Path) -> Result<(), SkillError> {
    fs::create_dir_all(path).map_err(|error| SkillError::io("create skill store", path, error))?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| SkillError::io("inspect skill store", path, error))?;
    if !metadata.file_type().is_dir() {
        return Err(SkillError::Invalid(format!(
            "skill store `{}` is not a real directory",
            path.display()
        )));
    }
    owner_only_directory(path)?;
    Ok(())
}

pub(in crate::skills) fn publish(
    store: &Path,
    skill: &CapturedSkill,
) -> Result<PathBuf, SkillError> {
    let target = store.join(&skill.digest);
    if target
        .try_exists()
        .map_err(|error| SkillError::io("inspect installed skill", &target, error))?
    {
        load_owned(store, &skill.digest)?;
        freeze_directory(&target)?;
        sync_directory(store, "sync repaired skill store")?;
        return Ok(target);
    }
    let staging = store.join(format!(".installing-{}", Uuid::new_v4()));
    fs::create_dir(&staging)
        .map_err(|error| SkillError::io("create skill staging directory", &staging, error))?;
    owner_only_directory(&staging)?;
    let result = write_tree(&staging, &skill.files).and_then(|()| {
        sync_tree(&staging)?;
        match fs::rename(&staging, &target) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::DirectoryNotEmpty
                ) =>
            {
                load_owned(store, &skill.digest)?;
                remove_staging(&staging)?;
                return Ok(target.clone());
            }
            Err(error) => return Err(SkillError::io("publish skill", &target, error)),
        }
        freeze_directory(&target)?;
        sync_directory(store, "sync skill store")?;
        Ok(target.clone())
    });
    match result {
        Ok(target) => Ok(target),
        Err(original) if staging.exists() => match remove_staging(&staging) {
            Ok(()) => Err(original),
            Err(cleanup) => Err(SkillError::Conflict(format!(
                "{original}; staging cleanup also failed: {cleanup}"
            ))),
        },
        Err(original) => Err(original),
    }
}

fn write_tree(root: &Path, files: &[CapturedFile]) -> Result<(), SkillError> {
    for captured in files {
        let path = destination(root, &captured.relative)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| SkillError::io("create skill directory", parent, error))?;
            owner_only_directory(parent)?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| SkillError::io("create installed skill file", &path, error))?;
        file.write_all(&captured.bytes)
            .map_err(|error| SkillError::io("write installed skill file", &path, error))?;
        file.sync_all()
            .map_err(|error| SkillError::io("sync installed skill file", &path, error))?;
        readonly_file(&path, captured.executable)?;
    }
    Ok(())
}

fn destination(root: &Path, relative: &str) -> Result<PathBuf, SkillError> {
    let mut path = root.to_path_buf();
    for component in relative.split('/') {
        if component.is_empty() || matches!(component, "." | "..") {
            return Err(SkillError::Invalid(format!(
                "captured skill path `{relative}` is invalid"
            )));
        }
        path.push(component);
    }
    Ok(path)
}

fn sync_tree(root: &Path) -> Result<(), SkillError> {
    let mut directories = vec![root.to_path_buf()];
    let mut index = 0;
    while index < directories.len() {
        let directory = directories[index].clone();
        index += 1;
        let entries = fs::read_dir(&directory)
            .map_err(|error| SkillError::io("read installed skill directory", &directory, error))?;
        for entry in entries {
            let entry = entry
                .map_err(|error| SkillError::io("read installed skill entry", &directory, error))?;
            if entry
                .file_type()
                .map_err(|error| {
                    SkillError::io("inspect installed skill entry", entry.path(), error)
                })?
                .is_dir()
            {
                directories.push(entry.path());
            }
        }
    }
    for directory in directories.into_iter().rev() {
        File::open(&directory)
            .and_then(|file| file.sync_all())
            .map_err(|error| SkillError::io("sync installed skill directory", directory, error))?;
    }
    Ok(())
}

fn remove_staging(path: &Path) -> Result<(), SkillError> {
    make_tree_writable(path)?;
    fs::remove_dir_all(path)
        .map_err(|error| SkillError::io("remove skill staging directory", path, error))
}

fn make_tree_writable(root: &Path) -> Result<(), SkillError> {
    let mut directories = vec![root.to_path_buf()];
    let mut index = 0;
    while index < directories.len() {
        let directory = directories[index].clone();
        index += 1;
        owner_only_directory(&directory)?;
        for entry in fs::read_dir(&directory)
            .map_err(|error| SkillError::io("read skill staging directory", &directory, error))?
        {
            let entry = entry
                .map_err(|error| SkillError::io("read skill staging entry", &directory, error))?;
            let path = entry.path();
            if entry
                .file_type()
                .map_err(|error| SkillError::io("inspect skill staging entry", &path, error))?
                .is_dir()
            {
                directories.push(path);
            } else {
                writable_file(&path)?;
            }
        }
    }
    Ok(())
}

fn sync_directory(path: &Path, action: &'static str) -> Result<(), SkillError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| SkillError::io(action, path, error))
}

fn freeze_directory(root: &Path) -> Result<(), SkillError> {
    let mut directories = vec![root.to_path_buf()];
    let mut index = 0;
    while index < directories.len() {
        let directory = directories[index].clone();
        index += 1;
        for entry in fs::read_dir(&directory)
            .map_err(|error| SkillError::io("read installed skill directory", &directory, error))?
        {
            let entry = entry
                .map_err(|error| SkillError::io("read installed skill entry", &directory, error))?;
            let path = entry.path();
            let file_type = entry
                .file_type()
                .map_err(|error| SkillError::io("inspect installed skill entry", &path, error))?;
            if file_type.is_dir() {
                directories.push(entry.path());
            } else if file_type.is_file() {
                let executable = executable_file(&path)?;
                readonly_file(&path, executable)?;
            } else {
                return Err(SkillError::Invalid(format!(
                    "installed skill contains a non-file entry `{}`",
                    path.display()
                )));
            }
        }
    }
    for directory in directories.into_iter().rev() {
        readonly_directory(&directory)?;
    }
    Ok(())
}

#[cfg(unix)]
fn executable_file(path: &Path) -> Result<bool, SkillError> {
    use std::os::unix::fs::PermissionsExt as _;

    Ok(fs::metadata(path)
        .map_err(|error| SkillError::io("inspect installed skill file", path, error))?
        .permissions()
        .mode()
        & 0o111
        != 0)
}

#[cfg(not(unix))]
fn executable_file(_path: &Path) -> Result<bool, SkillError> {
    Ok(false)
}

#[cfg(unix)]
fn readonly_file(path: &Path, executable: bool) -> Result<(), SkillError> {
    use std::os::unix::fs::PermissionsExt as _;

    let mode = if executable { 0o500 } else { 0o400 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|error| SkillError::io("freeze installed skill file", path, error))
}

#[cfg(unix)]
fn writable_file(path: &Path) -> Result<(), SkillError> {
    use std::os::unix::fs::PermissionsExt as _;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| SkillError::io("unfreeze skill staging file", path, error))
}

#[cfg(not(unix))]
fn readonly_file(path: &Path, _executable: bool) -> Result<(), SkillError> {
    let mut permissions = fs::metadata(path)
        .map_err(|error| SkillError::io("inspect installed skill file", path, error))?
        .permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions)
        .map_err(|error| SkillError::io("freeze installed skill file", path, error))
}

#[cfg(not(unix))]
fn writable_file(path: &Path) -> Result<(), SkillError> {
    let mut permissions = fs::metadata(path)
        .map_err(|error| SkillError::io("inspect skill staging file", path, error))?
        .permissions();
    permissions.set_readonly(false);
    fs::set_permissions(path, permissions)
        .map_err(|error| SkillError::io("unfreeze skill staging file", path, error))
}

#[cfg(unix)]
fn readonly_directory(path: &Path) -> Result<(), SkillError> {
    use std::os::unix::fs::PermissionsExt as _;

    fs::set_permissions(path, fs::Permissions::from_mode(0o500))
        .map_err(|error| SkillError::io("freeze installed skill directory", path, error))
}

#[cfg(not(unix))]
fn readonly_directory(path: &Path) -> Result<(), SkillError> {
    let mut permissions = fs::metadata(path)
        .map_err(|error| SkillError::io("inspect installed skill directory", path, error))?
        .permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions)
        .map_err(|error| SkillError::io("freeze installed skill directory", path, error))
}

#[cfg(unix)]
fn owner_only_directory(path: &Path) -> Result<(), SkillError> {
    use std::os::unix::fs::PermissionsExt as _;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| SkillError::io("restrict skill directory", path, error))
}

#[cfg(not(unix))]
fn owner_only_directory(_path: &Path) -> Result<(), SkillError> {
    Ok(())
}
