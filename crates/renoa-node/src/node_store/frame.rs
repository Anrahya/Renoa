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
/// to the longest beginning that fits. A tool's call id or name still too
/// long after that is left out the same way, so every result fits.
pub(crate) fn fit(kind: ExecutionEventKind) -> Result<ExecutionEventKind, serde_json::Error> {
    if fits_alone(&kind)? {
        return Ok(kind);
    }
    let fitted = fit_payload(kind)?;
    if fits_alone(&fitted)? {
        return Ok(fitted);
    }
    // Only a tool's call id, name, and raw activity may still prevent fit.
    Ok(match fitted {
        ExecutionEventKind::ToolStarted {
            call_id,
            name,
            arguments,
            activity: _,
        } => ExecutionEventKind::ToolStarted {
            call_id: left_out(call_id.len()),
            name: left_out(name.len()),
            arguments,
            activity: None,
        },
        ExecutionEventKind::ToolFinished {
            call_id,
            output,
            is_error,
        } => ExecutionEventKind::ToolFinished {
            call_id: left_out(call_id.len()),
            output,
            is_error,
        },
        kind @ (ExecutionEventKind::ExecutionStarted
        | ExecutionEventKind::TurnStarted
        | ExecutionEventKind::AssistantMessage { .. }
        | ExecutionEventKind::ExecutionTerminated { .. }) => kind,
    })
}

/// `kind` with its tool payload left out, or its text cut to fit.
fn fit_payload(kind: ExecutionEventKind) -> Result<ExecutionEventKind, serde_json::Error> {
    Ok(match kind {
        ExecutionEventKind::ToolStarted {
            call_id,
            name,
            arguments,
            activity,
        } => ExecutionEventKind::ToolStarted {
            call_id,
            name,
            arguments: Value::String(left_out(serde_json::to_vec(&arguments)?.len())),
            activity,
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
            activity: Some("Writing config file".to_owned()),
        })
        .expect("fit");
        assert!(fits(&fitted));
        assert!(matches!(
            fitted,
            ExecutionEventKind::ToolStarted {
                arguments: Value::String(note),
                activity: Some(activity),
                ..
            } if note.starts_with("(Left out:") && activity == "Writing config file"
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
    fn a_tool_call_id_too_long_for_one_message_is_left_out_on_both_events() {
        let call_id = "c".repeat(MAX_APPLICATION_MESSAGE_BYTES);
        let started = fit(ExecutionEventKind::ToolStarted {
            call_id: call_id.clone(),
            name: "read_file".to_owned(),
            arguments: serde_json::json!({}),
            activity: None,
        })
        .expect("fit");
        let finished = fit(ExecutionEventKind::ToolFinished {
            call_id: call_id.clone(),
            output: "done".to_owned(),
            is_error: false,
        })
        .expect("fit");
        assert!(fits(&started) && fits(&finished));
        let (
            ExecutionEventKind::ToolStarted { call_id: start, .. },
            ExecutionEventKind::ToolFinished {
                call_id: finish, ..
            },
        ) = (&started, &finished)
        else {
            panic!("tool events stay tool events");
        };
        assert_eq!(start, &left_out(call_id.len()));
        assert_eq!(start, finish, "a call's start and finish still pair up");
        assert_eq!(fit(started.clone()).expect("refit"), started);
    }

    #[test]
    fn a_long_cancellation_reason_is_cut_to_fit() {
        let fitted = fit(ExecutionEventKind::ExecutionTerminated {
            terminal: ExecutionTerminal::Cancelled {
                reason: "r".repeat(MAX_APPLICATION_MESSAGE_BYTES),
            },
        })
        .expect("fit");
        assert!(fits(&fitted));
        assert!(matches!(
            &fitted,
            ExecutionEventKind::ExecutionTerminated {
                terminal: ExecutionTerminal::Cancelled { reason },
            } if reason.ends_with(CUT_MARK)
        ));
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

    #[test]
    fn oversized_raw_activity_falls_back_so_fit_still_fits() {
        let fitted = fit(ExecutionEventKind::ToolStarted {
            call_id: "call_1".to_owned(),
            name: "read_file".to_owned(),
            arguments: serde_json::json!({}),
            activity: Some("a".repeat(MAX_APPLICATION_MESSAGE_BYTES)),
        })
        .expect("fit");
        assert!(fits(&fitted));
        assert!(matches!(
            fitted,
            ExecutionEventKind::ToolStarted { activity: None, .. }
        ));
    }
}
