pub(crate) fn event(level: &'static str, name: &'static str, fields: &serde_json::Value) {
    renoa_telemetry::event("renoa.coordinator", level, name, fields);
}
