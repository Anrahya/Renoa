use renoa_agent::ToolError;
use serde::de::DeserializeOwned;
use serde_json::Value;

pub(crate) fn decode<T: DeserializeOwned>(arguments: Value) -> Result<T, ToolError> {
    serde_json::from_value(arguments)
        .map_err(|error| ToolError::invalid_input(format!("invalid tool arguments: {error}")))
}

pub(crate) fn non_empty(name: &str, value: &str) -> Result<(), ToolError> {
    if value.is_empty() {
        return Err(ToolError::invalid_input(format!(
            "{name} must not be empty"
        )));
    }
    Ok(())
}

pub(crate) fn bounded_limit(requested: Option<usize>, maximum: usize) -> Result<usize, ToolError> {
    let limit = requested.unwrap_or(maximum);
    if !(1..=maximum).contains(&limit) {
        return Err(ToolError::invalid_input(format!(
            "limit must be between 1 and {maximum}"
        )));
    }
    Ok(limit)
}
