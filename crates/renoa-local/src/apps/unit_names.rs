//! Transient unit names: which Host and app a `systemd --user` unit belongs to.

use std::{fs, path::Path};

use sha2::{Digest as _, Sha256};

const UNIT_PREFIX: &str = "renoa-app-";

fn home_hash(database: &Path) -> String {
    let canonical = fs::canonicalize(database).unwrap_or_else(|_| database.to_path_buf());
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_os_str().as_encoded_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(12);
    for byte in &digest[..6] {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{byte:02x}");
    }
    hex
}

pub(super) fn unit_name(database: &Path, app_name: &str) -> String {
    format!("{UNIT_PREFIX}{}-{app_name}.service", home_hash(database))
}

/// `_` never appears in an app name, so a staging unit cannot collide with
/// another app's unit and [`unit_owner`] can tell them apart.
const STAGING_MARKER: &str = "_stage-";

pub(super) fn staging_unit_name(database: &Path, app_name: &str, seq: u64) -> String {
    let pid = std::process::id();
    format!(
        "{UNIT_PREFIX}{}-{app_name}{STAGING_MARKER}{pid}-{seq}.service",
        home_hash(database)
    )
}

pub(super) struct UnitOwner<'a> {
    pub name: &'a str,
    pub staging: bool,
}

/// The app a unit belongs to, from its name between the home prefix and `.service`.
pub(super) fn unit_owner(stem: &str) -> UnitOwner<'_> {
    match stem.split_once(STAGING_MARKER) {
        Some((name, _)) => UnitOwner {
            name,
            staging: true,
        },
        None => UnitOwner {
            name: stem,
            staging: false,
        },
    }
}

pub(super) fn unit_prefix_for(database: &Path) -> String {
    format!("{UNIT_PREFIX}{}-", home_hash(database))
}
