//! The one RCP message every stored execution event must fit.
//!
//! Publication sends an event in a batch of its own when nothing smaller
//! shares its message, so an event that cannot fit one message would stop its
//! task's publication for good. [`fit`] bounds an event before it is stored or
//! counted. It depends on the event's kind alone, so the live record of an
//! event and the end-of-turn projection of the same event still agree.

use renoa_protocol::{
    CommandId, ExecutionEvent, ExecutionEventId, ExecutionEventKind, ExecutionId, ExecutionTerminal,
};
use renoa_rcp_wire::{ClientMessage, MAX_APPLICATION_MESSAGE_BYTES, TaskId};
use serde_json::Value;
use uuid::Uuid;

/// What replaces the end of a text cut to fit.
const CUT_MARK: &str = " … (cut: too long for one RCP message)";

/// Whether these events, published together, fit one RCP message.
pub(crate) fn fits_one_message(
    task_id: TaskId,
    command_id: CommandId,
    events: &[ExecutionEvent],
) -> Result<bool, serde_json::Error> {
    let message = ClientMessage::PublishExecutionEvents {
        task_id,
        command_id,
        events: events.to_vec(),
    };
    Ok(serde_json::to_vec(&message)?.len() <= MAX_APPLICATION_MESSAGE_BYTES)
}

/// `kind`, or a bounded copy of it when it cannot fit one message alone. A
/// tool's arguments or output that do not fit are left out with a note of
/// their size: no surface shows them. A message or a terminal reason is cut
/// to the longest beginning that fits.
pub(crate) fn fit(kind: ExecutionEventKind) -> Result<ExecutionEventKind, serde_json::Error> {
    if fits_alone(&kind)? {
        return Ok(kind);
    }
    Ok(match kind {
        ExecutionEventKind::ToolStarted {
            call_id,
            name,
            arguments,
        } => ExecutionEventKind::ToolStarted {
            call_id,
            name,
            arguments: Value::String(left_out(serde_json::to_vec(&arguments)?.len())),
        },
        ExecutionEventKind::ToolFinished {
            call_id,
            output,
            is_error,
        } => ExecutionEventKind::ToolFinished {
            call_id,
            output: left_out(output.len()),
            is_error,
        },
        ExecutionEventKind::AssistantMessage { text } => {
            cut(&text, |text| ExecutionEventKind::AssistantMessage { text })?
        }
        ExecutionEventKind::ExecutionTerminated {
            terminal: ExecutionTerminal::Failed { error },
        } => cut(&error, |error| ExecutionEventKind::ExecutionTerminated {
            terminal: ExecutionTerminal::Failed { error },
        })?,
        ExecutionEventKind::ExecutionTerminated {
            terminal: ExecutionTerminal::Cancelled { reason },
        } => cut(&reason, |reason| ExecutionEventKind::ExecutionTerminated {
            terminal: ExecutionTerminal::Cancelled { reason },
        })?,
        // Fixed-size kinds always fit.
        kind @ (ExecutionEventKind::ExecutionStarted
        | ExecutionEventKind::TurnStarted
        | ExecutionEventKind::ExecutionTerminated {
            terminal: ExecutionTerminal::Completed,
        }) => kind,
    })
}

/// Whether `kind` fits one message alone. Every identifier is a UUID of fixed
/// width, and the sequence and time take their widest values, so the answer
/// holds for any event of this kind.
fn fits_alone(kind: &ExecutionEventKind) -> Result<bool, serde_json::Error> {
    let event = ExecutionEvent {
        event_id: ExecutionEventId::from_uuid(Uuid::nil()),
        execution_id: ExecutionId::from_uuid(Uuid::nil()),
        sequence: u64::MAX,
        recorded_at_ms: i64::MIN,
        kind: kind.clone(),
    };
    fits_one_message(
        TaskId::from_uuid(Uuid::nil()),
        CommandId::from_uuid(Uuid::nil()),
        &[event],
    )
}

fn left_out(bytes: usize) -> String {
    format!("(Left out: {bytes} bytes, more than one RCP message carries.)")
}

/// The kind `make` builds from the longest beginning of `text` that fits,
/// followed by [`CUT_MARK`]. A beginning ends on a character boundary.
fn cut(
    text: &str,
    make: impl Fn(String) -> ExecutionEventKind,
) -> Result<ExecutionEventKind, serde_json::Error> {
    let ends = text.char_indices().map(|(end, _)| end).collect::<Vec<_>>();
    let build = |end: usize| make(format!("{}{CUT_MARK}", &text[..end]));
    // `ends[0]` is the empty beginning, which fits; find the last end that fits.
    let (mut fits, mut fails) = (0, ends.len());
    while fails - fits > 1 {
        let middle = fits + (fails - fits) / 2;
        if fits_alone(&build(ends[middle]))? {
            fits = middle;
        } else {
            fails = middle;
        }
    }
    Ok(build(ends.get(fits).copied().unwrap_or(0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fits(kind: &ExecutionEventKind) -> bool {
        fits_alone(kind).expect("encode")
    }

    #[test]
    fn an_event_that_fits_is_kept_as_it_is() {
        let kind = ExecutionEventKind::ToolFinished {
            call_id: "read".to_owned(),
            output: "x".repeat(1000),
            is_error: false,
        };
        assert_eq!(fit(kind.clone()).expect("fit"), kind);
    }

    #[test]
    fn a_tool_output_too_large_for_one_message_is_left_out() {
        let output = format!(
            "[{{\"type\":\"image\",\"data\":\"{}\"}}]",
            "A".repeat(MAX_APPLICATION_MESSAGE_BYTES)
        );
        let fitted = fit(ExecutionEventKind::ToolFinished {
            call_id: "read".to_owned(),
            output: output.clone(),
            is_error: true,
        })
        .expect("fit");
        assert!(fits(&fitted));
        assert_eq!(
            fitted,
            ExecutionEventKind::ToolFinished {
                call_id: "read".to_owned(),
                output: left_out(output.len()),
                is_error: true,
            }
        );
    }

    #[test]
    fn tool_arguments_too_large_for_one_message_are_left_out() {
        let fitted = fit(ExecutionEventKind::ToolStarted {
            call_id: "write".to_owned(),
            name: "write_file".to_owned(),
            arguments: serde_json::json!({ "content": "\"".repeat(MAX_APPLICATION_MESSAGE_BYTES / 2) }),
        })
        .expect("fit");
        assert!(fits(&fitted));
        assert!(matches!(
            fitted,
            ExecutionEventKind::ToolStarted { arguments: Value::String(note), .. }
                if note.starts_with("(Left out:")
        ));
    }

    #[test]
    fn a_long_message_is_cut_to_the_longest_beginning_that_fits() {
        // Quotes double when encoded, and the last character is four bytes.
        let text = format!("{}é😀", "\"".repeat(MAX_APPLICATION_MESSAGE_BYTES / 2));
        let fitted = fit(ExecutionEventKind::AssistantMessage { text: text.clone() }).expect("fit");
        let ExecutionEventKind::AssistantMessage { text: cut_text } = &fitted else {
            panic!("a message stays a message");
        };
        let kept = cut_text.strip_suffix(CUT_MARK).expect("cut mark");
        assert!(text.starts_with(kept));
        assert!(fits(&fitted));
        let one_more =
            &text[..kept.len() + text[kept.len()..].chars().next().expect("rest").len_utf8()];
        assert!(
            !fits(&ExecutionEventKind::AssistantMessage {
                text: format!("{one_more}{CUT_MARK}")
            }),
            "one more character would not fit"
        );
        assert_eq!(
            fit(fitted.clone()).expect("refit"),
            fitted,
            "fitting is stable"
        );
    }

    #[test]
    fn a_long_failure_reason_is_cut_to_fit() {
        let fitted = fit(ExecutionEventKind::ExecutionTerminated {
            terminal: ExecutionTerminal::Failed {
                error: "e".repeat(MAX_APPLICATION_MESSAGE_BYTES),
            },
        })
        .expect("fit");
        assert!(fits(&fitted));
    }
}
