//! How a memory job settled: Mnem's verdict on its answer, or why none was
//! applied. The names are the receipts `host_memory_jobs` stores; its `CHECK`
//! admits exactly these, which a test in `jobs` holds.

use crate::host::catalog::HostCatalogError;

/// How a job settled: Mnem's answer to its apply, or why it was not applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::plugins::host::memory) enum Receipt {
    /// Compaction saved the summary.
    Saved,
    /// Nothing more to do: the answer was applied already, or a later job
    /// covered it.
    Settled,
    /// Compaction's line was too long; the next job asks again.
    Retried,
    /// Dream wrote a new `MIND.md`.
    Dreamed,
    /// The model declined; Dream counted it toward giving up.
    Declined,
    /// Dream refused the draft and counted it toward giving up.
    Rejected,
    /// The job was stale: the notebook moved on since it was made.
    Conflict,
    /// Compaction cannot use the answer, such as one that repeats the prompt.
    /// Dream never refuses an answer this way: it rejects a bad draft.
    Invalid,
    /// Another dream held the notebook.
    Busy,
    /// The notebook could not be read or written.
    Failed,
    /// The model call failed, so nothing was applied.
    Unanswered,
    /// The reply stopped at the output limit, or for a tool, before any
    /// text, so nothing was applied and only its notebook stopped.
    Truncated,
    /// A restart found this Dream answer's apply begun and its receipt
    /// unwritten, so it is not applied again.
    Interrupted,
    /// Its command could not run on `ATTEMPTS` runs, so it was closed without
    /// an answer, and the agent moved to a new memory session.
    Abandoned,
}

impl Receipt {
    pub(in crate::plugins::host::memory) const ALL: [Self; 14] = [
        Self::Saved,
        Self::Settled,
        Self::Retried,
        Self::Dreamed,
        Self::Declined,
        Self::Rejected,
        Self::Conflict,
        Self::Invalid,
        Self::Busy,
        Self::Failed,
        Self::Unanswered,
        Self::Truncated,
        Self::Interrupted,
        Self::Abandoned,
    ];

    /// The stored name, the same as the serialized one.
    pub(in crate::plugins::host::memory) const fn as_str(self) -> &'static str {
        match self {
            Self::Saved => "saved",
            Self::Settled => "settled",
            Self::Retried => "retried",
            Self::Dreamed => "dreamed",
            Self::Declined => "declined",
            Self::Rejected => "rejected",
            Self::Conflict => "conflict",
            Self::Invalid => "invalid",
            Self::Busy => "busy",
            Self::Failed => "failed",
            Self::Unanswered => "unanswered",
            Self::Truncated => "truncated",
            Self::Interrupted => "interrupted",
            Self::Abandoned => "abandoned",
        }
    }

    pub(super) fn parse(stored: &str) -> Result<Self, HostCatalogError> {
        Self::ALL
            .into_iter()
            .find(|receipt| receipt.as_str() == stored)
            .ok_or_else(|| {
                HostCatalogError::Invalid(format!("unknown memory job receipt `{stored}`"))
            })
    }
}
