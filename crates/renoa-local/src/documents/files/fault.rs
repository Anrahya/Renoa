use std::{cell::RefCell, fs, path::Path};

use super::{AgentDefinitionError, document_io};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::documents) enum Injection {
    /// An identical file appears at the path just before the persist.
    IdenticalWinner,
    /// A conflicting file appears at the path just before the persist.
    ConflictingWinner,
    /// The persist succeeds and the sync that follows it fails.
    PostPersistFailure,
    /// The path is replaced after this attempt reports creating it.
    ReplacementAfterCreation,
}

thread_local! {
    static ARMED: RefCell<Vec<(&'static str, Injection)>> = const { RefCell::new(Vec::new()) };
}

pub(in crate::documents) fn arm(document: &'static str, injection: Injection) {
    ARMED.with(|armed| {
        let mut armed = armed.borrow_mut();
        armed.retain(|(name, _)| *name != document);
        armed.push((document, injection));
    });
}

pub(in crate::documents) fn disarm() {
    ARMED.with(|armed| armed.borrow_mut().clear());
}

fn armed_for(path: &Path) -> Option<Injection> {
    let name = path.file_name().and_then(|name| name.to_str());
    ARMED.with(|armed| {
        armed
            .borrow()
            .iter()
            .find(|(document, _)| Some(*document) == name)
            .map(|(_, injection)| *injection)
    })
}

pub(super) fn before_persist(path: &Path, content: &str) -> Result<(), AgentDefinitionError> {
    match armed_for(path) {
        Some(Injection::IdenticalWinner) => fs::write(path, content)
            .map_err(|source| document_io("inject an identical winner", path, source)),
        Some(Injection::ConflictingWinner) => fs::write(path, "injected winner\n")
            .map_err(|source| document_io("inject a conflicting winner", path, source)),
        _ => Ok(()),
    }
}

pub(super) fn after_persist(path: &Path) -> bool {
    armed_for(path) == Some(Injection::PostPersistFailure)
}

pub(in crate::documents) fn after_created(path: &Path) -> Result<(), AgentDefinitionError> {
    if armed_for(path) != Some(Injection::ReplacementAfterCreation) {
        return Ok(());
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("document");
    let replacement = path.with_file_name(format!(".replacement-{name}"));
    fs::write(&replacement, "replacement writer\n")
        .and_then(|()| fs::rename(&replacement, path))
        .map_err(|source| document_io("inject a replacement writer", path, source))
}
