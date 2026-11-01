//! Makes one chosen pin write fail at one stage. Armed by pin path, so
//! parallel tests never trip each other.

use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    sync::Mutex,
};

/// Where a pin write fails.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    /// After the staged file is written, before it replaces the pin.
    BeforeRename,
    /// After it replaced the pin, before the directory is synced.
    AfterRename,
}

static ARMED: Mutex<BTreeMap<PathBuf, Stage>> = Mutex::new(BTreeMap::new());

pub(crate) fn arm(pin: &Path, stage: Stage) {
    ARMED
        .lock()
        .expect("pin fault lock")
        .insert(pin.to_path_buf(), stage);
}

pub(super) fn inject(pin: &Path, reached: Stage) -> io::Result<()> {
    let mut armed = ARMED.lock().expect("pin fault lock");
    if armed.get(pin) != Some(&reached) {
        return Ok(());
    }
    armed.remove(pin);
    Err(io::Error::other(format!(
        "injected pin failure {reached:?}"
    )))
}
