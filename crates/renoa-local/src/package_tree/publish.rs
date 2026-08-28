use std::{
    fs::{self, File, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
};

use uuid::Uuid;

use super::{CapturedFile, CapturedTree, TreeError, TreeLimits, verify};

pub(crate) fn initialize_store(path: &Path) -> Result<(), TreeError> {
    fs::create_dir_all(path).map_err(|error| TreeError::io("create package store", path, error))?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| TreeError::io("inspect package store", path, error))?;
    if !metadata.file_type().is_dir() {
        return Err(TreeError::Invalid(format!(
            "package store `{}` is not a real directory",
            path.display()
        )));
    }
    owner_only_directory(path)?;
    Ok(())
}

pub(crate) fn publish(
    store: &Path,
    tree: &CapturedTree,
    digest_domain: &[u8],
    limits: TreeLimits,
) -> Result<PathBuf, TreeError> {
    let target = store.join(&tree.digest);
    if target
        .try_exists()
        .map_err(|error| TreeError::io("inspect installed package", &target, error))?
    {
        verify(&target, &tree.digest, digest_domain, limits)?;
        freeze_directory(&target)?;
        sync_directory(store)?;
        return Ok(target);
    }
    let staging = store.join(format!(".installing-{}", Uuid::new_v4()));
    fs::create_dir(&staging)
        .map_err(|error| TreeError::io("create package staging directory", &staging, error))?;
    owner_only_directory(&staging)?;
    let result = write_tree(&staging, &tree.files).and_then(|()| {
        sync_tree(&staging)?;
        match fs::rename(&staging, &target) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::DirectoryNotEmpty
                ) =>
            {
                verify(&target, &tree.digest, digest_domain, limits)?;
                remove_staging(&staging)?;
                return Ok(target.clone());
            }
            Err(error) => return Err(TreeError::io("publish package", &target, error)),
        }
        freeze_directory(&target)?;
        sync_directory(store)?;
        Ok(target.clone())
    });
    match result {
        Ok(target) => Ok(target),
        Err(original) if staging.exists() => match remove_staging(&staging) {
            Ok(()) => Err(original),
            Err(cleanup) => Err(TreeError::Conflict(format!(
                "{original}; staging cleanup also failed: {cleanup}"
            ))),
        },
        Err(original) => Err(original),
    }
}

fn write_tree(root: &Path, files: &[CapturedFile]) -> Result<(), TreeError> {
    for captured in files {
        let path = destination(root, &captured.relative)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| TreeError::io("create package directory", parent, error))?;
            owner_only_directory(parent)?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| TreeError::io("create installed package file", &path, error))?;
        file.write_all(&captured.bytes)
            .map_err(|error| TreeError::io("write installed package file", &path, error))?;
        file.sync_all()
            .map_err(|error| TreeError::io("sync installed package file", &path, error))?;
        readonly_file(&path, captured.executable)?;
    }
    Ok(())
}

fn destination(root: &Path, relative: &str) -> Result<PathBuf, TreeError> {
    let mut path = root.to_path_buf();
    for component in relative.split('/') {
        if component.is_empty() || matches!(component, "." | "..") {
            return Err(TreeError::Invalid(format!(
                "captured package path `{relative}` is invalid"
            )));
        }
        path.push(component);
    }
    Ok(path)
}

fn sync_tree(root: &Path) -> Result<(), TreeError> {
    let mut directories = vec![root.to_path_buf()];
    let mut index = 0;
    while index < directories.len() {
        let directory = directories[index].clone();
        index += 1;
        for entry in fs::read_dir(&directory)
            .map_err(|error| TreeError::io("read installed package directory", &directory, error))?
        {
            let entry = entry.map_err(|error| {
                TreeError::io("read installed package entry", &directory, error)
            })?;
            if entry
                .file_type()
                .map_err(|error| TreeError::io("inspect installed entry", entry.path(), error))?
                .is_dir()
            {
                directories.push(entry.path());
            }
        }
    }
    for directory in directories.into_iter().rev() {
        File::open(&directory)
            .and_then(|file| file.sync_all())
            .map_err(|error| TreeError::io("sync installed package directory", directory, error))?;
    }
    Ok(())
}

fn remove_staging(path: &Path) -> Result<(), TreeError> {
    make_tree_writable(path)?;
    fs::remove_dir_all(path)
        .map_err(|error| TreeError::io("remove package staging directory", path, error))
}

fn make_tree_writable(root: &Path) -> Result<(), TreeError> {
    let mut directories = vec![root.to_path_buf()];
    let mut index = 0;
    while index < directories.len() {
        let directory = directories[index].clone();
        index += 1;
        owner_only_directory(&directory)?;
        for entry in fs::read_dir(&directory)
            .map_err(|error| TreeError::io("read package staging directory", &directory, error))?
        {
            let entry = entry
                .map_err(|error| TreeError::io("read package staging entry", &directory, error))?;
            let path = entry.path();
            if entry
                .file_type()
                .map_err(|error| TreeError::io("inspect package staging entry", &path, error))?
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

fn freeze_directory(root: &Path) -> Result<(), TreeError> {
    let mut directories = vec![root.to_path_buf()];
    let mut index = 0;
    while index < directories.len() {
        let directory = directories[index].clone();
        index += 1;
        for entry in fs::read_dir(&directory)
            .map_err(|error| TreeError::io("read installed package directory", &directory, error))?
        {
            let entry = entry.map_err(|error| {
                TreeError::io("read installed package entry", &directory, error)
            })?;
            let path = entry.path();
            let file_type = entry
                .file_type()
                .map_err(|error| TreeError::io("inspect installed package entry", &path, error))?;
            if file_type.is_dir() {
                directories.push(path);
            } else if file_type.is_file() {
                readonly_file(&path, executable_file(&path)?)?;
            } else {
                return Err(TreeError::Invalid(format!(
                    "installed package contains a non-file entry `{}`",
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

fn sync_directory(path: &Path) -> Result<(), TreeError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| TreeError::io("sync package store", path, error))
}

#[cfg(unix)]
fn executable_file(path: &Path) -> Result<bool, TreeError> {
    use std::os::unix::fs::PermissionsExt as _;
    Ok(fs::metadata(path)
        .map_err(|error| TreeError::io("inspect installed package file", path, error))?
        .permissions()
        .mode()
        & 0o111
        != 0)
}

#[cfg(not(unix))]
fn executable_file(_path: &Path) -> Result<bool, TreeError> {
    Ok(false)
}

#[cfg(unix)]
fn readonly_file(path: &Path, executable: bool) -> Result<(), TreeError> {
    use std::os::unix::fs::PermissionsExt as _;
    let mode = if executable { 0o500 } else { 0o400 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|error| TreeError::io("freeze installed package file", path, error))
}

#[cfg(not(unix))]
fn readonly_file(path: &Path, _executable: bool) -> Result<(), TreeError> {
    let mut permissions = fs::metadata(path)
        .map_err(|error| TreeError::io("inspect installed package file", path, error))?
        .permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions)
        .map_err(|error| TreeError::io("freeze installed package file", path, error))
}

#[cfg(unix)]
fn writable_file(path: &Path) -> Result<(), TreeError> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| TreeError::io("unfreeze package staging file", path, error))
}

#[cfg(not(unix))]
fn writable_file(path: &Path) -> Result<(), TreeError> {
    let mut permissions = fs::metadata(path)
        .map_err(|error| TreeError::io("inspect package staging file", path, error))?
        .permissions();
    permissions.set_readonly(false);
    fs::set_permissions(path, permissions)
        .map_err(|error| TreeError::io("unfreeze package staging file", path, error))
}

#[cfg(unix)]
fn readonly_directory(path: &Path) -> Result<(), TreeError> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o500))
        .map_err(|error| TreeError::io("freeze installed package directory", path, error))
}

#[cfg(not(unix))]
fn readonly_directory(path: &Path) -> Result<(), TreeError> {
    let mut permissions = fs::metadata(path)
        .map_err(|error| TreeError::io("inspect installed package directory", path, error))?
        .permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions)
        .map_err(|error| TreeError::io("freeze installed package directory", path, error))
}

#[cfg(unix)]
fn owner_only_directory(path: &Path) -> Result<(), TreeError> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| TreeError::io("restrict package directory", path, error))
}

#[cfg(not(unix))]
fn owner_only_directory(_path: &Path) -> Result<(), TreeError> {
    Ok(())
}
