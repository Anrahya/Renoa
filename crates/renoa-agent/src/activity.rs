pub use renoa_protocol::{MAX_ACTIVITY_BYTES, normalize_activity};
use serde_json::{Map, Value, json};

use crate::ToolCall;

/// Optional tool-call argument describing the current work in one short line.
pub const ACTIVITY_FIELD: &str = "activity";

/// Schema description attached to [`ACTIVITY_FIELD`] on every model-visible tool.
pub const TOOL_ACTIVITY_DESCRIPTION: &str =
    "3-8 word present-tense line for the owner, e.g. \"Analyzing the xyz repo\".";

/// Reports whether `schema` can carry the synthetic [`ACTIVITY_FIELD`] without
/// colliding with a tool's own `"activity"` parameter or top-level union combinators.
#[must_use]
pub fn schema_accepts_synthetic_activity(schema: &Value) -> bool {
    let Some(obj) = schema.as_object() else {
        return false;
    };
    if obj.contains_key("anyOf") || obj.contains_key("oneOf") {
        return false;
    }
    match obj.get("properties") {
        Some(Value::Object(props)) => !props.contains_key(ACTIVITY_FIELD),
        None => true,
        Some(_) => false,
    }
}

fn synthetic_activity_property() -> Value {
    json!({
        "type": "string",
        "description": TOOL_ACTIVITY_DESCRIPTION,
    })
}

/// Reports whether an advertised tool schema carries Renoa's injected
/// synthetic [`ACTIVITY_FIELD`], as opposed to omitting it or defining its own.
#[must_use]
pub fn schema_uses_synthetic_activity(schema: &Value) -> bool {
    let Some(obj) = schema.as_object() else {
        return false;
    };
    if obj.contains_key("anyOf") || obj.contains_key("oneOf") {
        return false;
    }
    let Some(Value::Object(props)) = obj.get("properties") else {
        return false;
    };
    props
        .get(ACTIVITY_FIELD)
        .is_some_and(|prop| *prop == synthetic_activity_property())
}

/// Returns a copy of `schema` with optional [`ACTIVITY_FIELD`] present in `properties`.
#[must_use]
pub fn inject_activity_field(schema: &Value) -> Value {
    if !schema_accepts_synthetic_activity(schema) {
        return schema.clone();
    }
    let mut out = schema.clone();
    let Some(obj) = out.as_object_mut() else {
        return out;
    };
    let property = synthetic_activity_property();
    if let Some(props) = obj.get_mut("properties").and_then(Value::as_object_mut) {
        props.insert(ACTIVITY_FIELD.to_owned(), property);
    } else {
        let mut props = Map::new();
        props.insert(ACTIVITY_FIELD.to_owned(), property);
        obj.insert("properties".to_owned(), Value::Object(props));
    }
    out
}

/// Extracts and normalizes [`ACTIVITY_FIELD`] from raw tool-call JSON arguments.
#[must_use]
pub fn extract_tool_activity(arguments: &Value) -> Option<String> {
    arguments
        .as_object()?
        .get(ACTIVITY_FIELD)?
        .as_str()
        .and_then(normalize_activity)
}

/// Returns `arguments` with [`ACTIVITY_FIELD`] removed before tool dispatch.
#[must_use]
pub fn strip_activity_field(arguments: &Value) -> Value {
    let mut out = arguments.clone();
    if let Some(obj) = out.as_object_mut() {
        obj.remove(ACTIVITY_FIELD);
    }
    out
}

/// Returns `call` with [`ACTIVITY_FIELD`] stripped from its arguments.
#[must_use]
pub fn strip_tool_activity(mut call: ToolCall) -> ToolCall {
    if let Some(obj) = call.arguments.as_object_mut() {
        obj.remove(ACTIVITY_FIELD);
    }
    call
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        ACTIVITY_FIELD, MAX_ACTIVITY_BYTES, TOOL_ACTIVITY_DESCRIPTION, extract_tool_activity,
        inject_activity_field, normalize_activity, schema_accepts_synthetic_activity,
        schema_uses_synthetic_activity, strip_activity_field, strip_tool_activity,
    };
    use crate::ToolCall;

    #[test]
    fn normalize_activity_drops_over_long_instead_of_truncating() {
        assert_eq!(normalize_activity(""), None);
        assert_eq!(normalize_activity("   \u{200B}\t "), None);
        assert_eq!(normalize_activity("first\nsecond"), None);
        assert_eq!(normalize_activity("bad\x1b[31mcolor"), None);
        assert_eq!(normalize_activity("zero\0byte"), None);
        assert_eq!(
            normalize_activity("  \u{200B}Reading \u{202E}main.rs  "),
            Some("Reading main.rs".to_owned())
        );

        let exact = "a".repeat(MAX_ACTIVITY_BYTES);
        assert_eq!(normalize_activity(&exact), Some(exact));
        assert_eq!(
            normalize_activity(&"a".repeat(MAX_ACTIVITY_BYTES + 1)),
            None
        );
        assert_eq!(
            normalize_activity(&"é".repeat(MAX_ACTIVITY_BYTES / 2 + 1)),
            None
        );
    }

    #[test]
    fn inject_activity_field_adds_optional_string_without_mutating_required() {
        let with_props = inject_activity_field(&json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" }
            },
            "required": ["path"],
            "additionalProperties": false
        }));
        assert_eq!(
            with_props["properties"][ACTIVITY_FIELD],
            json!({
                "type": "string",
                "description": TOOL_ACTIVITY_DESCRIPTION,
            })
        );
        assert_eq!(with_props["required"], json!(["path"]));
        assert_eq!(with_props["additionalProperties"], json!(false));

        let bare = inject_activity_field(&json!({ "type": "object" }));
        assert_eq!(bare["properties"][ACTIVITY_FIELD]["type"], json!("string"));

        let own_activity = json!({
            "type": "object",
            "properties": {
                "activity": { "type": "string", "description": "Workout kind" }
            },
            "required": ["activity"]
        });
        assert!(!schema_uses_synthetic_activity(&own_activity));
        assert!(!schema_accepts_synthetic_activity(&own_activity));
        assert_eq!(inject_activity_field(&own_activity), own_activity);
        assert!(schema_uses_synthetic_activity(&with_props));
        assert!(schema_uses_synthetic_activity(&bare));

        let union = json!({
            "type": "object",
            "properties": { "x": { "type": "string" } },
            "oneOf": [{ "required": ["x"] }]
        });
        assert!(!schema_accepts_synthetic_activity(&union));
        assert_eq!(inject_activity_field(&union), union);
    }

    #[test]
    fn extract_and_strip_handle_valid_and_invalid_values() {
        assert_eq!(
            extract_tool_activity(&json!({ "path": "a.rs", "activity": "  Reading a.rs " })),
            Some("Reading a.rs".to_owned())
        );
        assert_eq!(extract_tool_activity(&json!({ "activity": 42 })), None);
        assert_eq!(extract_tool_activity(&json!({ "activity": "   " })), None);
        assert_eq!(
            extract_tool_activity(&json!({ "activity": "one\ntwo" })),
            None
        );

        assert_eq!(
            strip_activity_field(&json!({ "path": "a.rs", "activity": "Reading a.rs" })),
            json!({ "path": "a.rs" })
        );
        let call = strip_tool_activity(ToolCall {
            id: "1".to_owned(),
            name: "read".to_owned(),
            arguments: json!({ "path": "a.rs", "activity": "Reading a.rs" }),
            thought_signature: None,
            namespace: None,
        });
        assert_eq!(call.arguments, json!({ "path": "a.rs" }));
    }
}
