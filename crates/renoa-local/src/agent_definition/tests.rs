use std::path::PathBuf;

use super::AgentDefinition;

/// The published schema is generated from the type, never edited by hand.
/// `RENOA_WRITE_SCHEMAS=1 cargo test -p renoa-local agent_definition_schema`
/// rewrites it after a change to the definition.
#[test]
fn agent_definition_schema() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/schemas/agent-definition.schema.json");
    let schema = serde_json::to_value(schemars::schema_for!(AgentDefinition)).expect("schema");
    let generated = serde_json::to_string_pretty(&sorted(schema)).expect("schema JSON") + "\n";
    if std::env::var_os("RENOA_WRITE_SCHEMAS").is_some() {
        std::fs::write(&path, &generated).expect("write the schema");
    }
    let published = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        published == generated,
        "{} is not the schema of AgentDefinition; run RENOA_WRITE_SCHEMAS=1 cargo test -p renoa-local agent_definition_schema",
        path.display()
    );
}

/// Sorts every object's keys, so the file is the same whichever `serde_json`
/// features a build unifies: `preserve_order` keeps the generator's order.
fn sorted(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<_> = map.into_iter().collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            entries
                .into_iter()
                .map(|(key, value)| (key, sorted(value)))
                .collect()
        }
        serde_json::Value::Array(items) => items.into_iter().map(sorted).collect(),
        other => other,
    }
}
