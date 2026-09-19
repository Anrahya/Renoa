//! Document file handling: paths, publication, and content-hash revisions.
//!
//! Files are the content source of truth for the owner-editable prompt
//! documents. Publication adopts an identical existing file and fails closed on
//! conflicting content, so a retry after a crash before the database commit
//! converges on the same files.

use std::{
    fs::{self, File},
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
};

use serde::Deserialize;

use renoa_kernel::AgentId;

use crate::{AgentDefinitionError, AgentDocuments, atomic_file::content_hash};

pub(super) const SOUL_FILE: &str = "SOUL.md";
pub(super) const USER_FILE: &str = "USER.md";
const DOCUMENT_DIRECTORY: &str = "agents";

pub(super) fn document_root(
    data_directory: &Path,
    agent: AgentId,
    enabled: AgentDocuments,
) -> Result<PathBuf, AgentDefinitionError> {
    if !enabled.any() {
        return Err(AgentDefinitionError::EmptyDocumentSet);
    }
    let data_directory = fs::canonicalize(data_directory)
        .map_err(|source| document_io("resolve Host data directory", data_directory, source))?;
    let directory = data_directory.join(DOCUMENT_DIRECTORY);
    let root = directory.join(agent.to_string());
    create_plain_directory(&directory, agent)?;
    create_plain_directory(&root, agent)?;
    restrict_directory(&root)?;
    let resolved = fs::canonicalize(&root)
        .map_err(|source| document_io("resolve agent document directory", &root, source))?;
    // Every component was created without following a link, so the root is the
    // agent's own directory only when it resolves to exactly this path.
    if resolved != root {
        return Err(AgentDefinitionError::DocumentsOutsideDataDirectory {
            agent,
            path: resolved,
        });
    }
    Ok(resolved)
}

/// Creates one document directory component unless it already exists, refusing
/// a symlink or any other file in its place before anything is written through
/// it.
fn create_plain_directory(path: &Path, agent: AgentId) -> Result<(), AgentDefinitionError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(AgentDefinitionError::DocumentsOutsideDataDirectory {
            agent,
            path: path.to_path_buf(),
        }),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => fs::create_dir(path)
            .map_err(|source| document_io("create agent document directory", path, source)),
        Err(source) => Err(document_io(
            "inspect agent document directory",
            path,
            source,
        )),
    }
}

pub(super) fn restrict_directory(path: &Path) -> Result<(), AgentDefinitionError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|source| document_io("restrict agent document directory", path, source))?;
    }
    Ok(())
}

pub(super) struct DocumentSnapshot {
    pub(super) content: String,
    pub(super) revision: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Document {
    Soul,
    User,
}

impl Document {
    pub(super) const fn file_name(self) -> &'static str {
        match self {
            Self::Soul => SOUL_FILE,
            Self::User => USER_FILE,
        }
    }

    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::Soul => "soul",
            Self::User => "user",
        }
    }
}

/// What a document path already holds for the content about to be published.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Publication {
    /// Nothing is published at the path.
    Absent,
    /// The path already holds exactly this content.
    Identical,
    /// The path holds different content.
    Conflicting,
}

/// Classifies one document path against `content`, failing when the path is a
/// symlink or not a readable regular file.
pub(super) fn publication_state(
    path: &Path,
    content: &str,
) -> Result<Publication, AgentDefinitionError> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            require_regular_file(path)?;
            let mut existing = Vec::new();
            File::open(path)
                .and_then(|mut file| file.read_to_end(&mut existing))
                .map_err(|source| document_io("read agent document", path, source))?;
            Ok(if existing == content.as_bytes() {
                Publication::Identical
            } else {
                Publication::Conflicting
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Publication::Absent),
        Err(source) => Err(document_io("inspect agent document", path, source)),
    }
}

/// What one publication did with its target path.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Published {
    /// This attempt installed the file.
    Created,
    /// The path already held exactly this content.
    Adopted,
}

pub(super) fn publish_document(
    path: &Path,
    content: &str,
) -> Result<Published, AgentDefinitionError> {
    match publication_state(path, content)? {
        Publication::Identical => return Ok(Published::Adopted),
        Publication::Conflicting => {
            return Err(AgentDefinitionError::DocumentConflict {
                path: path.to_path_buf(),
            });
        }
        Publication::Absent => {}
    }
    let parent = path
        .parent()
        .ok_or_else(|| AgentDefinitionError::DocumentPath {
            path: path.to_path_buf(),
        })?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".renoa-agent-")
        .tempfile_in(parent)
        .map_err(|source| document_io("create agent document staging file", path, source))?;
    temporary
        .write_all(content.as_bytes())
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|source| document_io("write agent document staging file", path, source))?;
    #[cfg(test)]
    fault::before_persist(path, content)?;
    match temporary.persist_noclobber(path) {
        Ok(file) => {
            let failure = sync_publication(&file, parent, path).err();
            #[cfg(test)]
            let failure = failure.or_else(|| injected_post_persist_failure(path));
            if let Some(error) = failure {
                // The path now holds this attempt's own publication, so the
                // failure removes it again and leaves nothing partial behind.
                return Err(discard_publication(path, error));
            }
        }
        // A writer outside the Host can take the path between the classification
        // above and this persist, so the winner is adopted only when it holds
        // exactly this content.
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            if publication_state(path, content)? != Publication::Identical {
                return Err(AgentDefinitionError::DocumentConflict {
                    path: path.to_path_buf(),
                });
            }
            return Ok(Published::Adopted);
        }
        Err(error) => {
            return Err(document_io("publish agent document", path, error.error));
        }
    }
    if let Err(error) = require_regular_file(path) {
        return Err(discard_publication(path, error));
    }
    Ok(Published::Created)
}

fn sync_publication(file: &File, parent: &Path, path: &Path) -> Result<(), AgentDefinitionError> {
    file.sync_all()
        .map_err(|source| document_io("sync agent document", path, source))?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| document_io("sync agent document directory", parent, source))
}

/// A test-injected failure between a successful persist and its sync.
#[cfg(test)]
fn injected_post_persist_failure(path: &Path) -> Option<AgentDefinitionError> {
    fault::after_persist(path).then(|| {
        document_io(
            "sync agent document",
            path,
            std::io::Error::other("injected post-persist failure"),
        )
    })
}

/// Removes one document this attempt installed and returns the failure that
/// followed it, or an error naming both problems when the removal fails too.
fn discard_publication(path: &Path, failure: AgentDefinitionError) -> AgentDefinitionError {
    match fs::remove_file(path) {
        Ok(()) => failure,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => failure,
        Err(error) => AgentDefinitionError::PublicationCleanup {
            path: path.to_path_buf(),
            failure: failure.to_string(),
            cleanup: error.to_string(),
        },
    }
}

pub(super) fn require_regular_file(path: &Path) -> Result<(), AgentDefinitionError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Err(AgentDefinitionError::DocumentNotFile {
                path: path.to_path_buf(),
            });
        }
        Err(source) => return Err(document_io("inspect agent document", path, source)),
    };
    if metadata.file_type().is_file() && !metadata.file_type().is_symlink() {
        Ok(())
    } else {
        Err(AgentDefinitionError::DocumentNotFile {
            path: path.to_path_buf(),
        })
    }
}

pub(super) fn append_document(
    target: &mut String,
    tag: &str,
    file: &str,
    snapshot: &DocumentSnapshot,
) {
    target.push('<');
    target.push_str(tag);
    target.push_str(" source=\"");
    target.push_str(file);
    target.push_str("\" revision=\"");
    target.push_str(&snapshot.revision);
    target.push_str("\">\n");
    target.push_str(snapshot.content.trim_end());
    target.push('\n');
    target.push_str("</");
    target.push_str(tag);
    target.push('>');
}

pub(super) fn document_io(
    operation: &'static str,
    path: &Path,
    source: std::io::Error,
) -> AgentDefinitionError {
    AgentDefinitionError::DocumentIo {
        operation,
        path: path.to_path_buf(),
        source,
    }
}

pub(super) fn revision(content: &[u8]) -> String {
    revision_from_hash(content_hash(content))
}

pub(super) fn revision_from_hash(hash: [u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(64);
    for byte in hash {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

pub(super) fn is_revision(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

/// Test-only injection for the publication windows a real filesystem cannot
/// reach deterministically: a writer winning the race to the path, and a
/// failure between a successful persist and its sync.
#[cfg(test)]
pub(super) mod fault {
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
}
