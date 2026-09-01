use std::{io::Write as _, time::SystemTime};

pub(crate) fn event(level: &'static str, name: &'static str, fields: &serde_json::Value) {
    let timestamp_ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok());
    let record = serde_json::json!({
        "timestamp_ms": timestamp_ms,
        "level": level,
        "component": "renoa.node",
        "event": name,
        "fields": fields,
    });
    let Ok(mut encoded) = serde_json::to_vec(&record) else {
        return;
    };
    encoded.push(b'\n');
    let _ = std::io::stderr().lock().write_all(&encoded);
}
