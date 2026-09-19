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

pub(super) fn publish_document(path: &Path, content: &str) -> Result<(), AgentDefinitionError> {
    match publication_state(path, content)? {
        Publication::Identical => return Ok(()),
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
    match temporary.persist_noclobber(path) {
        Ok(file) => {
            file.sync_all()
                .map_err(|source| document_io("sync agent document", path, source))?;
            File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|source| document_io("sync agent document directory", parent, source))?;
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
        }
        Err(error) => {
            return Err(document_io("publish agent document", path, error.error));
        }
    }
    require_regular_file(path)
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
