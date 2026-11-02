//! How a memory job's turn and Mnem's verdict on its answer become the job's
//! receipt, which receipts stop a notebook or end the run, and how a job and
//! a run are reported: by job kinds and Mnem's outcomes, never by what a note
//! or an answer says.

use std::path::Path;

use jiff::civil::DateTime;
use mnem::{
    compactor::{Applied, SummaryJob},
    dream::Outcome,
    memory::MemoryError,
    model::Answer,
};
use renoa_agent::StopReason;
use uuid::Uuid;

use super::{
    super::{
        PLUGIN_ID, for_jobs,
        jobs::{Applying, Done, JobKind, Receipt},
    },
    Halt,
};
use crate::LocalTurnOutcome;

/// A job's receipt, the job Mnem asked for next when a compaction line was
/// too long, and what went wrong, for the trace only.
pub(super) type Verdict = (Receipt, Option<SummaryJob>, Option<String>);

/// The answer a memory job's turn gave Mnem. Only a turn the model ended
/// itself answers: with text, a reply; with none, a decline. A turn that
/// failed, stopped at its output limit, or was cancelled never reached an
/// answer, so nothing is applied and Mnem's give-up rule does not count it.
pub(super) fn answer(outcome: LocalTurnOutcome) -> Result<Answer, String> {
    match outcome {
        LocalTurnOutcome::Completed {
            output,
            stop_reason: StopReason::Stop,
        } => Ok(if output.trim().is_empty() {
            Answer::Declined
        } else {
            Answer::Reply(output)
        }),
        LocalTurnOutcome::Completed {
            stop_reason: stop_reason @ (StopReason::Length | StopReason::ToolUse),
            ..
        } => Err(format!("the answer stopped early ({stop_reason:?})")),
        LocalTurnOutcome::Failed { reason } => Err(reason),
        LocalTurnOutcome::Cancelled => Err("the job was cancelled".to_owned()),
        LocalTurnOutcome::WaitingForInput => Err("the job waited for input".to_owned()),
        LocalTurnOutcome::Compacted { .. } => Err("the job compacted instead".to_owned()),
    }
}

/// Applies `answer` to the notebook of `principal`; blocks on Mnem. Mnem
/// settles an answer it applied already, so applying again after a crash
/// changes nothing.
pub(super) fn apply(
    data_directory: &Path,
    principal: Uuid,
    applying: &Applying,
    answer: Answer,
    local_time: DateTime,
) -> Verdict {
    let memory = match for_jobs(data_directory, principal) {
        Ok(Some(memory)) => memory,
        Ok(None) => {
            return (
                Receipt::Failed,
                None,
                Some("the notebook is gone".to_owned()),
            );
        }
        Err(error) => return (Receipt::Failed, None, Some(error.to_string())),
    };
    match applying {
        Applying::Compaction(id) => match memory.apply_compaction(id, answer) {
            Ok(Applied::Saved(_)) => (Receipt::Saved, None, None),
            Ok(Applied::Settled) => (Receipt::Settled, None, None),
            Ok(Applied::Retry(next)) => (Receipt::Retried, Some(next), None),
            Err(error) => refused(error),
        },
        Applying::Dream(id) => match memory.apply_dream(id, answer, local_time) {
            Ok(report) => (dreamed(&report.outcome), None, None),
            Err(error) => refused(error),
        },
    }
}

/// Why a job with `receipt` stops its notebook or ends the run, if it does.
pub(super) fn halt(receipt: Receipt, kind: JobKind) -> Option<Halt> {
    let kind = kind.as_str();
    match receipt {
        Receipt::Invalid => Some(Halt::Notebook(format!(
            "Mnem refused a {kind} answer as invalid"
        ))),
        Receipt::Failed => Some(Halt::Notebook(format!(
            "a notebook could not take a {kind} answer"
        ))),
        Receipt::Unanswered => Some(Halt::Failed(format!(
            "the model did not answer a {kind} job"
        ))),
        Receipt::Busy => Some(Halt::Busy),
        Receipt::Saved
        | Receipt::Settled
        | Receipt::Retried
        | Receipt::Dreamed
        | Receipt::Declined
        | Receipt::Rejected
        | Receipt::Conflict
        | Receipt::Interrupted => None,
    }
}

/// Mnem refused to make a job.
pub(super) fn refusal(error: MemoryError) -> Halt {
    match error {
        MemoryError::Busy { .. } => Halt::Busy,
        MemoryError::Invalid { message } | MemoryError::Conflict { message } => {
            Halt::Notebook(message)
        }
        MemoryError::Corrupt { path, detail } => {
            Halt::Notebook(format!("{} cannot be read: {detail}", path.display()))
        }
        MemoryError::Io { path, source } => Halt::Notebook(format!("{}: {source}", path.display())),
    }
}

/// The receipt for an apply Mnem refused, and its message.
fn refused(error: MemoryError) -> Verdict {
    let (receipt, detail) = match error {
        MemoryError::Invalid { message } => (Receipt::Invalid, message),
        MemoryError::Conflict { message } => (Receipt::Conflict, message),
        MemoryError::Busy { message } => (Receipt::Busy, message),
        MemoryError::Corrupt { path, detail } => (
            Receipt::Failed,
            format!("{} cannot be read: {detail}", path.display()),
        ),
        MemoryError::Io { path, source } => {
            (Receipt::Failed, format!("{}: {source}", path.display()))
        }
    };
    (receipt, None, Some(detail))
}

/// The receipt for a Dream Mnem took. Only the first four come from an
/// apply; the rest name the same states for a run.
const fn dreamed(outcome: &Outcome) -> Receipt {
    match outcome {
        Outcome::Dreamed { .. } => Receipt::Dreamed,
        Outcome::Declined => Receipt::Declined,
        Outcome::Rejected(_) => Receipt::Rejected,
        Outcome::Settled | Outcome::Tidied { .. } => Receipt::Settled,
        Outcome::Busy => Receipt::Busy,
        Outcome::Unavailable(_) => Receipt::Unanswered,
    }
}

/// Logs one job's `memory_job` event: a warning when it stopped its notebook
/// or the run.
pub(super) fn trace(fields: &serde_json::Value, halted: bool) {
    let level = if halted { "warn" } else { "info" };
    renoa_telemetry::event(PLUGIN_ID, level, "memory_job", fields);
}

/// The receipts of one run, counted, then why it stopped.
pub(super) fn summary(done: &[Done], halt: Option<&Halt>) -> String {
    let mut parts = Vec::new();
    for kind in [JobKind::Compaction, JobKind::Dream] {
        let counted: Vec<String> = Receipt::ALL
            .into_iter()
            .filter_map(|receipt| {
                let count = done
                    .iter()
                    .filter(|job| job.kind == kind && job.receipt == Some(receipt))
                    .count();
                (count > 0).then(|| format!("{count} {}", receipt.as_str()))
            })
            .collect();
        if !counted.is_empty() {
            parts.push(format!("{}: {}", kind.as_str(), counted.join(", ")));
        }
    }
    let mut summary = if parts.is_empty() {
        "Nothing was due.".to_owned()
    } else {
        format!("{}.", parts.join("; "))
    };
    match halt {
        None => {}
        Some(Halt::Off) => summary.push_str(
            " Stopped: renoa.memory is off for this agent, or it is no longer the Host's main agent.",
        ),
        Some(Halt::Busy) => summary.push_str(" Stopped: another dream held a notebook."),
        Some(Halt::Notebook(reason) | Halt::Failed(reason)) => {
            summary.push_str(" Stopped: ");
            summary.push_str(reason);
        }
        #[cfg(test)]
        Some(Halt::Crashed) => summary.push_str(" Stopped: injected crash."),
    }
    summary
}
