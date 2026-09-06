use std::{fs::Metadata, path::Path};

/// Checks a regular credential file against the local Host's private-file rules.
///
/// On Unix, accept owner-only files or a read-only `0440` credential directly
/// inside the launcher's trusted credential directory (`0500` or `0550`) with
/// matching owner and group. Pass only a trusted launcher value such as
/// `CREDENTIALS_DIRECTORY`, never a model-selected directory. Symlinks are not
/// accepted: callers must obtain `metadata` with `symlink_metadata`.
#[must_use]
pub fn credential_file_is_private(
    path: &Path,
    metadata: &Metadata,
    credentials_directory: Option<&Path>,
) -> bool {
    if !metadata.file_type().is_file() {
        return false;
    }
    private_permissions(path, metadata, credentials_directory)
}

#[cfg(unix)]
fn private_permissions(path: &Path, metadata: &Metadata, directory: Option<&Path>) -> bool {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let mode = metadata.permissions().mode() & 0o777;
    if mode.trailing_zeros() >= 6 {
        return true;
    }
    mode == 0o440
        && directory == path.parent()
        && directory.is_some_and(|directory| {
            std::fs::symlink_metadata(directory).is_ok_and(|parent| {
                parent.file_type().is_dir()
                    && matches!(parent.permissions().mode() & 0o777, 0o500 | 0o550)
                    && parent.uid() == metadata.uid()
                    && parent.gid() == metadata.gid()
            })
        })
}

#[cfg(not(unix))]
fn private_permissions(_: &Path, _: &Metadata, _: Option<&Path>) -> bool {
    true
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    #[test]
    fn private_files_and_trusted_credential_mounts_have_one_shared_rule() {
        let root = tempfile::tempdir().expect("credential root");
        let directory = root.path().join("credentials");
        std::fs::create_dir(&directory).expect("credential directory");
        let path = directory.join("token");
        std::fs::write(&path, "test-token").expect("credential file");
        let check = |trusted: Option<&Path>| {
            credential_file_is_private(
                &path,
                &std::fs::symlink_metadata(&path).expect("metadata"),
                trusted,
            )
        };
        for mode in [0o400, 0o600] {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
                .expect("private mode");
            assert!(check(None));
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o440))
            .expect("credential mode");
        assert!(!check(None));
        for mode in [0o500, 0o550] {
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(mode))
                .expect("mount mode");
            assert!(check(Some(&directory)));
            assert!(!check(Some(root.path())));
        }
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o750))
            .expect("writable directory");
        assert!(!check(Some(&directory)));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .expect("public file");
        assert!(!check(Some(&directory)));
        let link = root.path().join("alias");
        symlink(&path, &link).expect("symlink");
        assert!(!credential_file_is_private(
            &link,
            &std::fs::symlink_metadata(&link).expect("link metadata"),
            None
        ));
    }
}
