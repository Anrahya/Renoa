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

/// The answer a memory job's turn gave Mnem, or the receipt of a turn that
/// gave none. A turn the model ended itself answers: with text, a reply;
/// with none, a decline. A reply stopped at the output limit, or for a tool
/// it was never offered, is incomplete: Dream takes its text as a reply, and
/// rejects and counts a draft without its closing tag, while compaction
/// declines it, so Mnem cuts the shortest earlier try or the block's own
/// input rather than saving half a line. One with no text at all is
/// `Truncated`, which stops only its notebook, and nothing is applied. A turn
/// that failed or was cancelled is `Unanswered`: nothing is applied, and
/// Mnem's give-up rule counts neither.
pub(super) fn answer(
    outcome: LocalTurnOutcome,
    kind: JobKind,
) -> Result<Answer, (Receipt, String)> {
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
            output,
            stop_reason: stop_reason @ (StopReason::Length | StopReason::ToolUse),
        } => {
            if output.trim().is_empty() {
                Err((
                    Receipt::Truncated,
                    format!("the reply stopped ({stop_reason:?}) before any text"),
                ))
            } else if kind == JobKind::Dream {
                Ok(Answer::Reply(output))
            } else {
                Ok(Answer::Declined)
            }
        }
        LocalTurnOutcome::Failed { reason } => Err((Receipt::Unanswered, reason)),
        LocalTurnOutcome::Cancelled => {
            Err((Receipt::Unanswered, "the job was cancelled".to_owned()))
        }
        LocalTurnOutcome::WaitingForInput { .. } => {
            Err((Receipt::Unanswered, "the job waited for input".to_owned()))
        }
        LocalTurnOutcome::Compacted { .. } => {
            Err((Receipt::Unanswered, "the job compacted instead".to_owned()))
        }
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
            Ok(Applied::Saved(how)) => (Receipt::Saved, None, Some(format!("{how:?}"))),
            Ok(Applied::Settled) => (Receipt::Settled, None, None),
            Ok(Applied::Retry(next)) => (Receipt::Retried, Some(next), None),
            Err(error) => refused(error),
        },
        Applying::Dream(id) => match memory.apply_dream(id, answer, local_time) {
            Ok(report) => {
                let detail = match &report.outcome {
                    Outcome::Rejected(why) => Some(why.clone()),
                    _ => None,
                };
                (dreamed(&report.outcome), None, detail)
            }
            Err(error) => refused(error),
        },
    }
}

/// Why a job with `receipt` stops its notebook or ends the run, if it does:
/// only a model that failed ends it.
pub(super) fn halt(receipt: Receipt, kind: JobKind) -> Option<Halt> {
    let kind = kind.as_str();
    match receipt {
        Receipt::Invalid => Some(Halt::Notebook(format!(
            "Mnem refused a {kind} answer as invalid"
        ))),
        Receipt::Failed => Some(Halt::Notebook(format!(
            "a notebook could not take a {kind} answer"
        ))),
        Receipt::Truncated => Some(Halt::Notebook(format!(
            "a {kind} reply stopped at the output limit before any text"
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
        | Receipt::Interrupted
        | Receipt::Abandoned => None,
    }
}

/// Mnem refused to make a job.
pub(in crate::plugins::host::memory) fn refusal(error: MemoryError) -> Halt {
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

/// Logs one event of `renoa.memory`'s jobs. Tests read them back.
pub(in crate::plugins::host::memory) fn event(
    level: &'static str,
    name: &'static str,
    fields: &serde_json::Value,
) {
    #[cfg(test)]
    EVENTS
        .lock()
        .expect("events")
        .push((level, name, fields.clone()));
    renoa_telemetry::event(PLUGIN_ID, level, name, fields);
}

/// Every event logged in this test process.
#[cfg(test)]
pub(crate) static EVENTS: std::sync::Mutex<Vec<(&str, &str, serde_json::Value)>> =
    std::sync::Mutex::new(Vec::new());

/// Logs one job's `memory_job` event: a warning when it stopped its notebook
/// or the run.
pub(super) fn trace(fields: &serde_json::Value, halted: bool) {
    event(if halted { "warn" } else { "info" }, "memory_job", fields);
}

/// Logs that the compaction bound stopped a notebook with work still due.
pub(super) fn trace_bound(fields: &serde_json::Value) {
    event("info", "memory_compaction_bounded", fields);
}

/// The receipts of one run, counted, the notebooks the compaction bound
/// stopped with summaries still due, then why it stopped.
pub(in crate::plugins::host::memory) fn summary(
    done: &[Done],
    bounded: usize,
    halt: Option<&Halt>,
) -> String {
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
    match bounded {
        0 => {}
        1 => summary.push_str(" Compaction reached its bound on 1 notebook with more due."),
        count => {
            summary.push_str(" Compaction reached its bound on ");
            summary.push_str(&count.to_string());
            summary.push_str(" notebooks with more due.");
        }
    }
    match halt {
        None => {}
        Some(Halt::Off) => summary.push_str(
            " Stopped: renoa.memory is off for this agent, or it is no longer the Host's main agent.",
        ),
        Some(Halt::Busy) => summary.push_str(" Stopped: another dream held a notebook."),
        Some(
            Halt::Notebook(reason)
            | Halt::Unopened(reason)
            | Halt::Stalled(reason)
            | Halt::Failed(reason),
        ) => {
            summary.push_str(" Stopped: ");
            summary.push_str(reason);
        }
        #[cfg(test)]
        Some(Halt::Crashed) => summary.push_str(" Stopped: injected crash."),
    }
    summary
}
