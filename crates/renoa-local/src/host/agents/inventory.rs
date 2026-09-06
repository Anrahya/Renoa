use std::path::Path;

use uuid::Uuid;

use crate::{
    LocalHostError,
    host_storage::{MANIFEST_FILE, SessionManifest, read_manifest_file},
};

pub(super) fn manifests(sessions: &Path) -> Result<Vec<SessionManifest>, LocalHostError> {
    let mut manifests = Vec::new();
    for entry in std::fs::read_dir(sessions)? {
        let entry = entry?;
        let Some(id) = entry
            .file_name()
            .to_str()
            .and_then(|name| Uuid::parse_str(name).ok())
        else {
            // Staging and deletion directories are not published sessions.
            continue;
        };
        if !entry.file_type()?.is_dir() {
            return Err(LocalHostError::InvalidRequest(
                "published session is not a directory".to_owned(),
            ));
        }
        let manifest = match read_manifest_file(&entry.path().join(MANIFEST_FILE)) {
            Ok(manifest) => manifest,
            Err(LocalHostError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                if !entry.path().try_exists()? {
                    continue;
                }
                return Err(error.into());
            }
            Err(error) => return Err(error),
        };
        if manifest.session_id.to_string() != id.to_string() {
            return Err(LocalHostError::InvalidRequest(
                "session manifest identity differs from its directory".to_owned(),
            ));
        }
        manifests.push(manifest);
    }
    Ok(manifests)
}
