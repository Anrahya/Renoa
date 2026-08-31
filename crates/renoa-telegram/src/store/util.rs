use std::{path::Path, time::SystemTime};

use uuid::Uuid;

use super::StoreError;

pub(super) fn require_payload(kind: &str, payload: Option<String>) -> Result<String, StoreError> {
    payload.ok_or_else(|| StoreError::Invalid(format!("{kind} update has no payload")))
}

pub(super) fn parse_uuid(value: &str, name: &str) -> Result<Uuid, StoreError> {
    Uuid::parse_str(value)
        .map_err(|_| StoreError::Invalid(format!("stored {name} identity is not a UUID")))
}

pub(super) fn require_one(
    changed: usize,
    update_id: i64,
    from: &str,
    to: &str,
) -> Result<(), StoreError> {
    if changed == 1 {
        Ok(())
    } else {
        Err(StoreError::Invalid(format!(
            "update {update_id} cannot transition from {from} to {to}"
        )))
    }
}

pub(super) fn draft_id(request_id: Uuid) -> i64 {
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&request_id.as_bytes()[..8]);
    let candidate = i64::from_be_bytes(bytes) & i64::MAX;
    if candidate == 0 { 1 } else { candidate }
}

pub(super) fn encoded_path(path: &Path) -> Vec<u8> {
    path.as_os_str().as_encoded_bytes().to_vec()
}

pub(super) fn now_ms() -> Result<i64, StoreError> {
    let millis = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| StoreError::Invalid("system clock precedes the Unix epoch".to_owned()))?
        .as_millis();
    i64::try_from(millis)
        .map_err(|_| StoreError::Invalid("system time exceeded SQLite integer".to_owned()))
}

#[cfg(unix)]
pub(super) fn restrict_directory(path: &Path) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt as _;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
pub(super) fn restrict_directory(_path: &Path) -> Result<(), std::io::Error> {
    Ok(())
}
