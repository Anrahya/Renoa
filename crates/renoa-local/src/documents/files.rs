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

pub(super) struct PublicationRoot {
    pub(super) path: PathBuf,
    parent: PathBuf,
    root_created: bool,
    parent_created: bool,
}

impl PublicationRoot {
    pub(super) const fn was_created(&self) -> bool {
        self.root_created
    }

    pub(super) fn cleanup_empty(&self) {
        if self.root_created {
            let _ = fs::remove_dir(&self.path);
        }
        if self.parent_created {
            let _ = fs::remove_dir(&self.parent);
        }
    }
}

pub(super) fn publication_root(
    data_directory: &Path,
    agent: AgentId,
    enabled: AgentDocuments,
) -> Result<PublicationRoot, AgentDefinitionError> {
    let data_directory = document_data_directory(data_directory, enabled)?;
    let directory = data_directory.join(DOCUMENT_DIRECTORY);
    let root = directory.join(agent.to_string());
    let parent_created = create_plain_directory(&directory, agent)?;
    let root_created = match create_plain_directory(&root, agent) {
        Ok(created) => created,
        Err(error) => {
            if parent_created {
                let _ = fs::remove_dir(&directory);
            }
            return Err(error);
        }
    };
    let prepared = PublicationRoot {
        path: root.clone(),
        parent: directory,
        root_created,
        parent_created,
    };
    let result = (|| {
        if root_created {
            restrict_directory(&root)?;
        }
        resolve_exact_root(&root, agent)
    })();
    match result {
        Ok(path) => Ok(PublicationRoot { path, ..prepared }),
        Err(error) => {
            prepared.cleanup_empty();
            Err(error)
        }
    }
}

pub(super) fn existing_document_root(
    data_directory: &Path,
    agent: AgentId,
    enabled: AgentDocuments,
) -> Result<PathBuf, AgentDefinitionError> {
    let data_directory = document_data_directory(data_directory, enabled)?;
    let directory = data_directory.join(DOCUMENT_DIRECTORY);
    require_plain_directory(&directory, agent)?;
    let root = directory.join(agent.to_string());
    require_plain_directory(&root, agent)?;
    resolve_exact_root(&root, agent)
}

fn document_data_directory(
    data_directory: &Path,
    enabled: AgentDocuments,
) -> Result<PathBuf, AgentDefinitionError> {
    if !enabled.any() {
        return Err(AgentDefinitionError::EmptyDocumentSet);
    }
    fs::canonicalize(data_directory)
        .map_err(|source| document_io("resolve Host data directory", data_directory, source))
}

fn resolve_exact_root(root: &Path, agent: AgentId) -> Result<PathBuf, AgentDefinitionError> {
    let resolved = fs::canonicalize(root)
        .map_err(|source| document_io("resolve agent document directory", root, source))?;
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

/// Creates one document directory component unless it already exists and
/// reports whether this attempt created it.
fn create_plain_directory(path: &Path, agent: AgentId) -> Result<bool, AgentDefinitionError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(false),
        Ok(_) => Err(AgentDefinitionError::DocumentsOutsideDataDirectory {
            agent,
            path: path.to_path_buf(),
        }),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => fs::create_dir(path)
            .map(|()| true)
            .map_err(|source| document_io("create agent document directory", path, source)),
        Err(source) => Err(document_io(
            "inspect agent document directory",
            path,
            source,
        )),
    }
}

fn require_plain_directory(path: &Path, agent: AgentId) -> Result<(), AgentDefinitionError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(AgentDefinitionError::DocumentsOutsideDataDirectory {
            agent,
            path: path.to_path_buf(),
        }),
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
#[derive(Clone, PartialEq, Eq)]
pub(super) enum Published {
    /// This attempt installed the file.
    Created(PublishedFile),
    /// The path already held exactly this content.
    Adopted,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct PublishedFile {
    identity: FileIdentity,
}

#[cfg(unix)]
#[derive(Clone, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

#[cfg(not(unix))]
#[derive(Clone, PartialEq, Eq)]
struct FileIdentity {
    length: u64,
    modified: Option<std::time::SystemTime>,
}

impl FileIdentity {
    fn read(file: &File, path: &Path) -> Result<Self, AgentDefinitionError> {
        let metadata = file
            .metadata()
            .map_err(|source| document_io("inspect published agent document", path, source))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            Ok(Self {
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {
                length: metadata.len(),
                modified: metadata.modified().ok(),
            })
        }
    }

    fn matches_path(&self, path: &Path) -> Result<bool, std::io::Error> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Ok(false);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            Ok(metadata.dev() == self.device && metadata.ino() == self.inode)
        }
        #[cfg(not(unix))]
        {
            Ok(metadata.len() == self.length && metadata.modified().ok() == self.modified)
        }
    }
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
    let identity = FileIdentity::read(temporary.as_file(), path)?;
    #[cfg(test)]
    fault::before_persist(path, content)?;
    match temporary.persist_noclobber(path) {
        Ok(file) => {
            let published = PublishedFile { identity };
            let failure = sync_publication(&file, parent, path).err();
            #[cfg(test)]
            let failure = failure.or_else(|| injected_post_persist_failure(path));
            if let Some(error) = failure {
                return Err(discard_publication(path, &published, error));
            }
            match published.identity.matches_path(path) {
                Ok(true) => {}
                Ok(false) => {
                    return Err(AgentDefinitionError::DocumentPublicationReplaced {
                        path: path.to_path_buf(),
                    });
                }
                Err(source) => {
                    let error = document_io("verify published agent document", path, source);
                    return Err(discard_publication(path, &published, error));
                }
            }
            if let Err(error) = require_regular_file(path) {
                return Err(discard_publication(path, &published, error));
            }
            Ok(Published::Created(published))
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
            Ok(Published::Adopted)
        }
        Err(error) => Err(document_io("publish agent document", path, error.error)),
    }
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
fn discard_publication(
    path: &Path,
    published: &PublishedFile,
    failure: AgentDefinitionError,
) -> AgentDefinitionError {
    match remove_published(path, published) {
        Ok(()) => failure,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => failure,
        Err(error) => AgentDefinitionError::PublicationCleanup {
            path: path.to_path_buf(),
            failure: failure.to_string(),
            cleanup: error.to_string(),
        },
    }
}

pub(super) fn remove_published(
    path: &Path,
    published: &PublishedFile,
) -> Result<(), std::io::Error> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    }
    if !published.identity.matches_path(path)? {
        return Err(std::io::Error::other(
            "the path no longer names the file installed by this publication",
        ));
    }
    fs::remove_file(path)
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

/// Test-only injection for the publication windows a real filesystem cannot
/// reach deterministically: a writer winning the race to the path, and a
/// failure between a successful persist and its sync.
#[cfg(test)]
pub(super) mod fault;
