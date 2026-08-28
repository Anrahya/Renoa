use renoa_agent::ContentBlock;
use serde::Deserialize;
use serde_json::Value;

use crate::mcp::{McpAuthorization, McpRemoteFailure};

const WIRE_VERSION: u32 = 4;
const MAX_CONTENT_BLOCKS: usize = 256;
const MAX_STRUCTURED_CONTENT_BYTES: usize = 4 * 1_024 * 1_024;

#[derive(Clone, Debug)]
pub(in crate::mcp) struct McpCallResult {
    pub(in crate::mcp) content: Vec<ContentBlock>,
    pub(in crate::mcp) details: Option<Value>,
    pub(in crate::mcp) is_error: bool,
}

impl McpCallResult {
    pub(super) fn redact_authorization(&mut self, authorization: Option<&McpAuthorization>) {
        let Some(authorization) = authorization else {
            return;
        };
        for block in &mut self.content {
            match block {
                ContentBlock::Text { text } => authorization.redact_text(text),
                ContentBlock::Image { data, mime_type } => {
                    authorization.redact_text(data);
                    authorization.redact_text(mime_type);
                }
            }
        }
        if let Some(details) = &mut self.details {
            authorization.redact_json(details);
        }
    }
}

#[derive(Clone, Debug)]
pub(super) enum CallTerminal {
    Completed(McpCallResult),
    Failed(McpRemoteFailure),
}

#[derive(Debug)]
pub(super) struct ParsedCall {
    pub(super) dispatch_started: bool,
    pub(super) terminal: Option<CallTerminal>,
}

#[derive(Debug)]
pub(super) struct ParseFailure {
    pub(super) message: String,
    pub(super) dispatch_started: bool,
    pub(super) definite_terminal_evidence: bool,
}

pub(super) fn parse_call_records(encoded: &[u8]) -> Result<ParsedCall, ParseFailure> {
    let mut dispatch_started = false;
    for line in encoded
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let header =
            serde_json::from_slice::<RecordHeader>(line).map_err(|error| ParseFailure {
                message: format!("decode record header: {error}"),
                dispatch_started,
                definite_terminal_evidence: false,
            })?;
        if header.wire_version != WIRE_VERSION {
            return Err(ParseFailure {
                message: format!(
                    "adapter wire version {} is unsupported; expected {WIRE_VERSION}",
                    header.wire_version
                ),
                dispatch_started,
                definite_terminal_evidence: false,
            });
        }
        match header.event.as_str() {
            "dispatch_started" => {
                if dispatch_started {
                    return Err(ParseFailure {
                        message: "adapter returned more than one dispatch transition".to_owned(),
                        dispatch_started,
                        definite_terminal_evidence: false,
                    });
                }
                serde_json::from_slice::<DispatchRecord>(line).map_err(|error| ParseFailure {
                    message: format!("decode dispatch record: {error}"),
                    dispatch_started: true,
                    definite_terminal_evidence: false,
                })?;
                dispatch_started = true;
            }
            "completed" => {
                if !dispatch_started {
                    return Err(ParseFailure {
                        message: "adapter completed a tool call before its dispatch transition"
                            .to_owned(),
                        dispatch_started,
                        definite_terminal_evidence: true,
                    });
                }
                let record = serde_json::from_slice::<CompletedRecord>(line).map_err(|error| {
                    ParseFailure {
                        message: format!("decode completed record: {error}"),
                        dispatch_started,
                        definite_terminal_evidence: true,
                    }
                })?;
                let result = record.result.project().map_err(|message| ParseFailure {
                    message,
                    dispatch_started,
                    definite_terminal_evidence: true,
                })?;
                return Ok(ParsedCall {
                    dispatch_started,
                    terminal: Some(CallTerminal::Completed(result)),
                });
            }
            "failed" => {
                let record =
                    serde_json::from_slice::<FailedRecord>(line).map_err(|error| ParseFailure {
                        message: format!("decode failed record: {error}"),
                        dispatch_started,
                        definite_terminal_evidence: true,
                    })?;
                record
                    .failure
                    .validate_wire()
                    .map_err(|message| ParseFailure {
                        message: format!("invalid failed record: {message}"),
                        dispatch_started,
                        definite_terminal_evidence: true,
                    })?;
                return Ok(ParsedCall {
                    dispatch_started,
                    terminal: Some(CallTerminal::Failed(record.failure)),
                });
            }
            event => {
                return Err(ParseFailure {
                    message: format!("adapter returned unexpected '{event}' record for a call"),
                    dispatch_started,
                    definite_terminal_evidence: false,
                });
            }
        }
    }
    Ok(ParsedCall {
        dispatch_started,
        terminal: None,
    })
}

#[derive(Deserialize)]
pub(super) struct RecordHeader {
    pub(super) wire_version: u32,
    pub(super) event: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DispatchRecord {
    #[serde(rename = "wire_version")]
    _wire_version: u32,
    #[serde(rename = "event")]
    _event: DispatchEvent,
}

#[derive(Deserialize)]
enum DispatchEvent {
    #[serde(rename = "dispatch_started")]
    Started,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompletedRecord {
    #[serde(rename = "wire_version")]
    _wire_version: u32,
    #[serde(rename = "event")]
    _event: CompletedEvent,
    result: WireToolResult,
}

#[derive(Deserialize)]
enum CompletedEvent {
    #[serde(rename = "completed")]
    Completed,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FailedRecord {
    #[serde(rename = "wire_version")]
    _wire_version: u32,
    #[serde(rename = "event")]
    _event: FailedEvent,
    failure: McpRemoteFailure,
}

#[derive(Deserialize)]
enum FailedEvent {
    #[serde(rename = "failed")]
    Failed,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireToolResult {
    content: Vec<ContentBlock>,
    structured_content: Value,
    is_error: bool,
}

impl WireToolResult {
    fn project(self) -> Result<McpCallResult, String> {
        if self.content.is_empty() || self.content.len() > MAX_CONTENT_BLOCKS {
            return Err(format!(
                "completed result must contain 1-{MAX_CONTENT_BLOCKS} content blocks"
            ));
        }
        let details = project_structured_content(self.structured_content)?;
        Ok(McpCallResult {
            content: self.content,
            details,
            is_error: self.is_error,
        })
    }
}

fn project_structured_content(value: Value) -> Result<Option<Value>, String> {
    let Value::Object(mut object) = value else {
        return Err("completed structured_content must be an object".to_owned());
    };
    let Some(Value::Bool(present)) = object.remove("present") else {
        return Err("completed structured_content.present must be boolean".to_owned());
    };
    if present {
        let details = object.remove("value").ok_or_else(|| {
            "completed structured_content is missing its present value".to_owned()
        })?;
        if !object.is_empty() {
            return Err("completed structured_content contains unknown fields".to_owned());
        }
        if serde_json::to_vec(&details)
            .map_err(|error| format!("encode completed structured_content: {error}"))?
            .len()
            > MAX_STRUCTURED_CONTENT_BYTES
        {
            return Err(format!(
                "completed structured_content exceeds {MAX_STRUCTURED_CONTENT_BYTES} bytes"
            ));
        }
        Ok(Some(details))
    } else if object.is_empty() {
        Ok(None)
    } else {
        Err("absent structured_content contains an unexpected value".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::{CallTerminal, parse_call_records};
    use crate::mcp::McpOutcomeCertainty;
    use renoa_agent::ContentBlock;
    use serde_json::Value;

    #[test]
    fn completed_error_preserves_order_duplicates_and_null_details() {
        let parsed = parse_call_records(
            br#"{"wire_version":4,"event":"dispatch_started"}
{"wire_version":4,"event":"completed","result":{"content":[{"type":"text","text":"same"},{"type":"image","data":"aW1hZ2U=","mime_type":"image/png"},{"type":"text","text":"same"}],"structured_content":{"present":true,"value":null},"is_error":true}}
"#,
        )
        .expect("valid call stream");
        let Some(CallTerminal::Completed(result)) = parsed.terminal else {
            panic!("expected completed terminal")
        };

        assert_eq!(
            result.content,
            vec![
                ContentBlock::text("same"),
                ContentBlock::image("aW1hZ2U=", "image/png"),
                ContentBlock::text("same"),
            ]
        );
        assert_eq!(result.details, Some(Value::Null));
        assert!(result.is_error);
    }

    #[test]
    fn typed_unknown_failure_survives_the_call_wire() {
        let parsed = parse_call_records(
            br#"{"wire_version":4,"event":"dispatch_started"}
{"wire_version":4,"event":"failed","failure":{"kind":"transport","certainty":"unknown","message":"response lost","partial_changes_possible":true,"diagnostic":{"code":"ECONNRESET","detail":"socket closed"}}}
"#,
        )
        .expect("valid call stream");
        let Some(CallTerminal::Failed(failure)) = parsed.terminal else {
            panic!("expected failed terminal")
        };

        assert_eq!(failure.certainty(), McpOutcomeCertainty::Unknown);
        assert!(failure.partial_changes_possible());
        assert_eq!(failure.diagnostic_code(), Some("ECONNRESET"));
    }

    #[test]
    fn records_after_the_first_terminal_cannot_replace_it() {
        let parsed = parse_call_records(
            br#"{"wire_version":4,"event":"dispatch_started"}
{"wire_version":4,"event":"completed","result":{"content":[{"type":"text","text":"first"}],"structured_content":{"present":false},"is_error":false}}
{"wire_version":4,"event":"failed","failure":{"kind":"internal","certainty":"definite","message":"late","partial_changes_possible":false,"diagnostic":{"detail":"late"}}}
"#,
        )
        .expect("first terminal is authoritative");
        let Some(CallTerminal::Completed(result)) = parsed.terminal else {
            panic!("expected first completed terminal")
        };
        assert_eq!(result.content, vec![ContentBlock::text("first")]);
    }

    #[test]
    fn completion_requires_a_prior_dispatch_transition() {
        let error = parse_call_records(
            br#"{"wire_version":4,"event":"completed","result":{"content":[{"type":"text","text":"impossible"}],"structured_content":{"present":false},"is_error":false}}
"#,
        )
        .expect_err("completion without dispatch violates the call state machine");

        assert!(error.definite_terminal_evidence);
        assert!(error.message.contains("before its dispatch transition"));
    }

    #[test]
    fn unknown_failure_cannot_deny_possible_remote_changes() {
        let error = parse_call_records(
            br#"{"wire_version":4,"event":"dispatch_started"}
{"wire_version":4,"event":"failed","failure":{"kind":"transport","certainty":"unknown","message":"lost","partial_changes_possible":false,"diagnostic":{"detail":"socket closed"}}}
"#,
        )
        .expect_err("unknown outcomes must admit possible changes");

        assert!(error.message.contains("unknown failure"));
    }
}
