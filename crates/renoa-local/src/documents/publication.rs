//! Publication of an agent's default `SOUL.md`, and its withdrawal when the
//! operation that published it does not commit.

use std::{
    convert::identity,
    path::{Path, PathBuf},
};

use renoa_kernel::AgentId;

use super::{
    AgentDocumentStore,
    files::{
        Publication, PublicationRoot, Published, PublishedFile, SOUL_FILE, discard_publication,
        publication_root, publication_state, publish_document, restrict_directory,
    },
};
use crate::{AgentDefinitionError, AgentDocuments};

/// What one publication installed. Dropping it keeps the file; a creation that
/// fails after publishing calls [`SoulPublication::withdraw`] instead.
pub(crate) struct SoulPublication {
    root: PublicationRoot,
    path: PathBuf,
    /// `None` when an identical file was already there and was adopted.
    created: Option<PublishedFile>,
}

impl std::fmt::Debug for SoulPublication {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SoulPublication")
            .field("path", &self.path)
            .field("created", &self.created.is_some())
            .finish_non_exhaustive()
    }
}

impl SoulPublication {
    /// Whether this publication wrote the file, rather than adopting one.
    #[must_use]
    pub(crate) const fn created(&self) -> bool {
        self.created.is_some()
    }

    /// Removes the file and the empty directories this publication created,
    /// after `failure` stopped the operation, and hands `failure` back. An
    /// adopted file stays, because this attempt did not write it.
    ///
    /// # Errors
    ///
    /// Returns an error naming both `failure` and the removal when the created
    /// file cannot be removed.
    pub(crate) fn withdraw<E: std::fmt::Display>(
        self,
        failure: E,
    ) -> Result<E, AgentDefinitionError> {
        let withdrawn = match &self.created {
            Some(published) => discard_publication(&self.path, published, failure),
            None => Ok(failure),
        };
        self.root.cleanup_empty();
        withdrawn
    }
}

impl AgentDocumentStore {
    /// Adopts or publishes the exact default `SOUL.md` for a new agent.
    ///
    /// Validation runs before the first write, and a failure after it removes
    /// exactly the file and directories this attempt created, so a rejected
    /// creation leaves no publication behind. A matching existing file is
    /// adopted, so a retry after a crash between file publication and database
    /// commit succeeds. Conflicting pre-existing content fails closed. An agent
    /// that reads only `USER.md` publishes nothing: that file belongs to a person.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty document set, unsafe paths, conflicting
    /// content, or storage failures.
    pub(crate) fn publish(
        data_directory: &Path,
        agent: AgentId,
        enabled: AgentDocuments,
        soul: &'static str,
    ) -> Result<Option<SoulPublication>, AgentDefinitionError> {
        if !enabled.any() {
            return Err(AgentDefinitionError::EmptyDocumentSet);
        }
        if !enabled.soul {
            return Ok(None);
        }
        let root = publication_root(data_directory, agent)?;
        let path = root.path.join(SOUL_FILE);
        match publication_state(&path, soul) {
            Ok(Publication::Conflicting) => {
                root.cleanup_empty();
                return Err(AgentDefinitionError::DocumentConflict { path });
            }
            Ok(Publication::Absent | Publication::Identical) => {}
            Err(error) => {
                root.cleanup_empty();
                return Err(error);
            }
        }
        let created = match publish_document(&path, soul) {
            Ok(Published::Created(published)) => Some(published),
            Ok(Published::Adopted) => None,
            Err(error) => {
                root.cleanup_empty();
                return Err(error);
            }
        };
        let publication = SoulPublication {
            root,
            path,
            created,
        };
        if !publication.root.was_created()
            && let Err(error) = restrict_directory(&publication.root.path)
        {
            return Err(publication.withdraw(error).unwrap_or_else(identity));
        }
        Ok(Some(publication))
    }
}
