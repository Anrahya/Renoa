//! The one diagnostic record format every Renoa process writes.
//!
//! Each record is a single JSON line on standard error, so a service manager
//! such as journald keeps it and one reader can correlate records across the
//! coordinator, nodes, and surfaces by the identities in `fields`.

use std::{io::Write as _, time::SystemTime};

/// Writes one structured diagnostic record for `component`.
///
/// Serialization or write failures are dropped: diagnostics never change the
/// outcome of the operation they describe.
pub fn event(
    component: &'static str,
    level: &'static str,
    name: &'static str,
    fields: &serde_json::Value,
) {
    let Ok(mut encoded) = serde_json::to_vec(&record(component, level, name, fields)) else {
        return;
    };
    encoded.push(b'\n');
    let _ = std::io::stderr().lock().write_all(&encoded);
}

fn record(
    component: &'static str,
    level: &'static str,
    name: &'static str,
    fields: &serde_json::Value,
) -> serde_json::Value {
    let timestamp_ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok());
    serde_json::json!({
        "timestamp_ms": timestamp_ms,
        "level": level,
        "component": component,
        "event": name,
        "fields": fields,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_record_names_its_component_event_and_fields() {
        let record = super::record(
            "renoa.test",
            "info",
            "task_opened",
            &serde_json::json!({"task_id": "t"}),
        );
        assert_eq!(record["component"], "renoa.test");
        assert_eq!(record["level"], "info");
        assert_eq!(record["event"], "task_opened");
        assert_eq!(record["fields"]["task_id"], "t");
        assert!(record["timestamp_ms"].as_u64().is_some());
    }
}
