use std::collections::HashMap;

use renoa_protocol::CommandEnvelope;
use rusqlite::{Connection, params};
use serde_json::{Map, Value, json};

use crate::{ControlError, TaskEventKind};

pub(crate) fn add_execution_command_causation(
    connection: &mut Connection,
) -> Result<(), ControlError> {
    let transaction = connection.transaction().map_err(sqlite_error)?;
    let events = {
        let mut statement = transaction
            .prepare("SELECT event_id, kind_json FROM task_events ORDER BY task_id, sequence")
            .map_err(sqlite_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(sqlite_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(sqlite_error)?
    };
    let mut execution_events = Vec::new();
    for (event_id, kind_json) in events {
        let mut kind: Value = serde_json::from_str(&kind_json)
            .map_err(|error| ControlError::store(format!("invalid task event JSON: {error}")))?;
        let kind_object = object(&mut kind, "task event")?;
        if kind_object.get("type").and_then(Value::as_str) != Some("execution_event") {
            continue;
        }
        execution_events.push((event_id, kind));
    }
    if !execution_events.is_empty() && !has_table(&transaction, "execution_event_streams")? {
        return Err(ControlError::store(
            "execution task events exist without durable execution command bindings",
        ));
    }
    let execution_commands = if execution_events.is_empty() {
        HashMap::new()
    } else {
        let mut statement = transaction
            .prepare("SELECT execution_id, command_id FROM execution_event_streams")
            .map_err(sqlite_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(sqlite_error)?;
        rows.collect::<Result<HashMap<_, _>, _>>()
            .map_err(sqlite_error)?
    };
    for (event_id, mut kind) in execution_events {
        let kind_object = object(&mut kind, "task event")?;
        let event = kind_object
            .get_mut("event")
            .ok_or_else(|| ControlError::store("execution task event is missing event"))?;
        let execution_id = string_field(
            object(event, "execution event")?,
            "executionId",
            "execution event",
        )?;
        let command_id = execution_commands.get(&execution_id).ok_or_else(|| {
            ControlError::store(format!(
                "execution task event {event_id} has no command binding for execution {execution_id}"
            ))
        })?;
        if let Some(existing) = kind_object.get("commandId") {
            if existing.as_str() != Some(command_id) {
                return Err(ControlError::store(format!(
                    "execution task event {event_id} has conflicting command causation"
                )));
            }
            continue;
        }
        kind_object.insert("commandId".to_owned(), Value::String(command_id.clone()));
        let migrated = serde_json::to_string(&kind)
            .map_err(|error| ControlError::store(format!("task event encoding failed: {error}")))?;
        transaction
            .execute(
                "UPDATE task_events SET kind_json = ?2 WHERE event_id = ?1",
                params![event_id, migrated],
            )
            .map_err(sqlite_error)?;
    }
    transaction
        .execute_batch("PRAGMA user_version = 6;")
        .map_err(sqlite_error)?;
    transaction.commit().map_err(sqlite_error)
}

pub(crate) fn remove_harness_configuration(
    connection: &mut Connection,
) -> Result<(), ControlError> {
    let transaction = connection.transaction().map_err(sqlite_error)?;
    rewrite_command_json(&transaction)?;
    rewrite_command_events(&transaction)?;
    for (table, column) in [
        ("tasks", "agent_id"),
        ("tasks", "agent_json"),
        ("commands", "agent_json"),
    ] {
        if has_column(&transaction, table, column)? {
            transaction
                .execute_batch(&format!("ALTER TABLE {table} DROP COLUMN {column};"))
                .map_err(sqlite_error)?;
        }
    }
    transaction
        .execute_batch("PRAGMA user_version = 5;")
        .map_err(sqlite_error)?;
    transaction.commit().map_err(sqlite_error)
}

fn rewrite_command_json(connection: &Connection) -> Result<(), ControlError> {
    let rows = {
        let mut statement = connection
            .prepare("SELECT command_id, command_json FROM commands")
            .map_err(sqlite_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(sqlite_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(sqlite_error)?
    };
    for (command_id, json) in rows {
        let command: CommandEnvelope = serde_json::from_str(&json)
            .map_err(|error| ControlError::store(format!("invalid command JSON: {error}")))?;
        let migrated = serde_json::to_string(&command)
            .map_err(|error| ControlError::store(format!("command encoding failed: {error}")))?;
        if migrated != json {
            connection
                .execute(
                    "UPDATE commands SET command_json = ?2 WHERE command_id = ?1",
                    params![command_id, migrated],
                )
                .map_err(sqlite_error)?;
        }
    }
    Ok(())
}

fn rewrite_command_events(connection: &Connection) -> Result<(), ControlError> {
    let rows = {
        let mut statement = connection
            .prepare("SELECT event_id, kind_json FROM task_events")
            .map_err(sqlite_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(sqlite_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(sqlite_error)?
    };
    for (event_id, json) in rows {
        let value: Value = serde_json::from_str(&json)
            .map_err(|error| ControlError::store(format!("invalid task event JSON: {error}")))?;
        if value.get("type").and_then(Value::as_str) != Some("command_submitted") {
            continue;
        }
        let kind: TaskEventKind = serde_json::from_value(value)
            .map_err(|error| ControlError::store(format!("invalid task event JSON: {error}")))?;
        let migrated = serde_json::to_string(&kind)
            .map_err(|error| ControlError::store(format!("task event encoding failed: {error}")))?;
        if migrated != json {
            connection
                .execute(
                    "UPDATE task_events SET kind_json = ?2 WHERE event_id = ?1",
                    params![event_id, migrated],
                )
                .map_err(sqlite_error)?;
        }
    }
    Ok(())
}

fn has_table(connection: &Connection, table: &str) -> Result<bool, ControlError> {
    connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1
            )",
            [table],
            |row| row.get(0),
        )
        .map_err(sqlite_error)
}

fn has_column(connection: &Connection, table: &str, expected: &str) -> Result<bool, ControlError> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(sqlite_error)?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(sqlite_error)?;
    for column in columns {
        if column.map_err(sqlite_error)? == expected {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn migrate_v3_execution_events(connection: &mut Connection) -> Result<(), ControlError> {
    let transaction = connection.transaction().map_err(sqlite_error)?;
    transaction
        .execute_batch(
            "ALTER TABLE run_event_streams RENAME TO execution_event_streams;
             ALTER TABLE execution_event_streams RENAME COLUMN run_id TO execution_id;",
        )
        .map_err(sqlite_error)?;
    let rows = {
        let mut statement = transaction
            .prepare("SELECT source_id, kind_json FROM task_events ORDER BY task_id, sequence")
            .map_err(sqlite_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(sqlite_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(sqlite_error)?
    };
    for (source_id, kind_json) in rows {
        let Some(migrated) = migrate_run_event(&source_id, &kind_json)? else {
            continue;
        };
        transaction
            .execute(
                "UPDATE task_events SET source_id = ?2, kind_json = ?3 WHERE source_id = ?1",
                params![source_id, migrated.source_id, migrated.kind_json],
            )
            .map_err(sqlite_error)?;
    }
    transaction
        .execute_batch("PRAGMA user_version = 4;")
        .map_err(sqlite_error)?;
    transaction.commit().map_err(sqlite_error)
}

struct MigratedEvent {
    source_id: String,
    kind_json: String,
}

fn migrate_run_event(
    source_id: &str,
    kind_json: &str,
) -> Result<Option<MigratedEvent>, ControlError> {
    let mut root: Value = serde_json::from_str(kind_json)
        .map_err(|error| ControlError::store(format!("invalid task event JSON: {error}")))?;
    let root = object(&mut root, "task event")?;
    if root.get("type").and_then(Value::as_str) != Some("run_event") {
        return Ok(None);
    }
    let event = root
        .get_mut("event")
        .ok_or_else(|| ControlError::store("run event is missing event"))?;
    let event = object(event, "run event")?;
    let event_id = string_field(event, "eventId", "run event")?;
    let expected_source = format!("run:{event_id}");
    if source_id != expected_source {
        return Err(ControlError::store(format!(
            "run event source {source_id} does not match event {event_id}"
        )));
    }
    let execution_id = event
        .remove("runId")
        .ok_or_else(|| ControlError::store("run event is missing runId"))?;
    event.insert("executionId".to_owned(), execution_id);
    let old_kind = event
        .remove("kind")
        .ok_or_else(|| ControlError::store("run event is missing kind"))?;
    event.insert("kind".to_owned(), migrate_run_event_kind(old_kind)?);
    root.insert(
        "type".to_owned(),
        Value::String("execution_event".to_owned()),
    );
    Ok(Some(MigratedEvent {
        source_id: format!("execution:{event_id}"),
        kind_json: serde_json::to_string(&Value::Object(root.clone()))
            .map_err(|error| ControlError::store(format!("task event encoding failed: {error}")))?,
    }))
}

fn migrate_run_event_kind(mut kind: Value) -> Result<Value, ControlError> {
    let kind = object(&mut kind, "run event kind")?;
    let kind_type = string_field(kind, "type", "run event kind")?;
    match kind_type.as_str() {
        "run_started" => Ok(json!({ "type": "execution_started" })),
        "model_requested" => Ok(json!({ "type": "turn_started" })),
        "model_responded" => {
            let response = object_field(kind, "response", "model_responded")?;
            let text = string_field(response, "text", "model response")?;
            Ok(json!({ "type": "assistant_message", "text": text }))
        }
        "capability_requested" => {
            let call = object_field(kind, "call", "capability_requested")?;
            Ok(json!({
                "type": "tool_started",
                "call_id": string_field(call, "callId", "capability call")?,
                "name": string_field(call, "name", "capability call")?,
                "arguments": call.get("arguments").cloned().ok_or_else(|| {
                    ControlError::store("capability call is missing arguments")
                })?
            }))
        }
        "capability_completed" => {
            let call_id = string_field(kind, "call_id", "capability_completed")?;
            let outcome = object_field(kind, "outcome", "capability_completed")?;
            let model_view = outcome
                .get("modelView")
                .ok_or_else(|| ControlError::store("capability outcome is missing modelView"))?;
            let output = model_view.as_str().map_or_else(
                || serde_json::to_string(model_view).expect("JSON value is serializable"),
                ToOwned::to_owned,
            );
            let is_error = outcome
                .get("isError")
                .and_then(Value::as_bool)
                .ok_or_else(|| ControlError::store("capability outcome is missing isError"))?;
            Ok(json!({
                "type": "tool_finished",
                "call_id": call_id,
                "output": output,
                "is_error": is_error
            }))
        }
        "run_terminated" => {
            let terminal = object_field(kind, "terminal", "run_terminated")?;
            let status = string_field(terminal, "status", "terminal state")?;
            let terminal = match status.as_str() {
                "completed" => json!({ "status": "completed" }),
                "failed" => json!({
                    "status": "failed",
                    "error": string_field(terminal, "error", "failed terminal state")?
                }),
                "cancelled" => json!({
                    "status": "cancelled",
                    "reason": string_field(terminal, "reason", "cancelled terminal state")?
                }),
                _ => {
                    return Err(ControlError::store(format!(
                        "unknown terminal status {status}"
                    )));
                }
            };
            Ok(json!({ "type": "execution_terminated", "terminal": terminal }))
        }
        _ => Err(ControlError::store(format!(
            "unknown run event kind {kind_type}"
        ))),
    }
}

fn object<'a>(
    value: &'a mut Value,
    context: &str,
) -> Result<&'a mut Map<String, Value>, ControlError> {
    value
        .as_object_mut()
        .ok_or_else(|| ControlError::store(format!("{context} is not an object")))
}

fn object_field<'a>(
    map: &'a mut Map<String, Value>,
    field: &str,
    context: &str,
) -> Result<&'a mut Map<String, Value>, ControlError> {
    let value = map
        .get_mut(field)
        .ok_or_else(|| ControlError::store(format!("{context} is missing {field}")))?;
    object(value, field)
}

fn string_field(
    object: &Map<String, Value>,
    field: &str,
    context: &str,
) -> Result<String, ControlError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| ControlError::store(format!("{context} is missing string {field}")))
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "the owned signature is required by Result::map_err"
)]
fn sqlite_error(error: rusqlite::Error) -> ControlError {
    ControlError::store(format!("SQLite error: {error}"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::migrate_run_event_kind;

    #[test]
    fn every_v3_run_kind_has_the_expected_baseline_projection() {
        let cases = [
            (
                json!({ "type": "run_started", "command": {}, "agent": {} }),
                json!({ "type": "execution_started" }),
            ),
            (
                json!({ "type": "model_requested", "round": 4 }),
                json!({ "type": "turn_started" }),
            ),
            (
                json!({
                    "type": "model_responded",
                    "round": 4,
                    "response": {
                        "text": "answer",
                        "capabilityCalls": [],
                        "truncated": false
                    }
                }),
                json!({ "type": "assistant_message", "text": "answer" }),
            ),
            (
                json!({
                    "type": "capability_requested",
                    "ordinal": 2,
                    "call": { "callId": "call-1", "name": "read", "arguments": { "path": "a" } }
                }),
                json!({
                    "type": "tool_started",
                    "call_id": "call-1",
                    "name": "read",
                    "arguments": { "path": "a" }
                }),
            ),
            (
                json!({
                    "type": "capability_completed",
                    "ordinal": 2,
                    "call_id": "call-1",
                    "outcome": { "modelView": { "text": "contents" }, "isError": false }
                }),
                json!({
                    "type": "tool_finished",
                    "call_id": "call-1",
                    "output": "{\"text\":\"contents\"}",
                    "is_error": false
                }),
            ),
            (
                json!({
                    "type": "run_terminated",
                    "terminal": { "status": "completed", "output": "answer" }
                }),
                json!({
                    "type": "execution_terminated",
                    "terminal": { "status": "completed" }
                }),
            ),
            (
                json!({
                    "type": "run_terminated",
                    "terminal": { "status": "failed", "error": "failure" }
                }),
                json!({
                    "type": "execution_terminated",
                    "terminal": { "status": "failed", "error": "failure" }
                }),
            ),
            (
                json!({
                    "type": "run_terminated",
                    "terminal": { "status": "cancelled", "reason": "stopped" }
                }),
                json!({
                    "type": "execution_terminated",
                    "terminal": { "status": "cancelled", "reason": "stopped" }
                }),
            ),
        ];

        for (old, expected) in cases {
            assert_eq!(
                migrate_run_event_kind(old).expect("migrate v3 event"),
                expected
            );
        }
    }
}
