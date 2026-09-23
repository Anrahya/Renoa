use std::collections::HashSet;

use monty_types::{CallArgs, MontyObject, ObjectRef};
use renoa_agent_loop::{CodeMcpCall, MAX_CODE_MCP_ARGUMENT_BYTES, MAX_CODE_MCP_REFERENCE_BYTES};
use serde_json::{Map, Number, Value};

const MAX_VALUE_DEPTH: usize = 16;

pub(super) fn mcp_call(call_id: u32, args: &CallArgs) -> Result<CodeMcpCall, String> {
    if args.kwargs().len() != 0 || args.args().len() != 2 {
        return Err(
            "mcp(reference, arguments) requires exactly two positional arguments".to_owned(),
        );
    }
    let mut positional = args.args();
    let reference = positional
        .next()
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or_else(|| "MCP reference must be a string".to_owned())?;
    if reference.is_empty() || reference.len() > MAX_CODE_MCP_REFERENCE_BYTES {
        return Err("MCP reference is empty or too long".to_owned());
    }
    let arguments = to_json(positional.next().expect("checked argument count"), 0)?;
    if !arguments.is_object() {
        return Err("MCP arguments must be a dictionary".to_owned());
    }
    let encoded = serde_json::to_vec(&arguments).map_err(|error| error.to_string())?;
    if encoded.len() > MAX_CODE_MCP_ARGUMENT_BYTES {
        return Err("MCP arguments exceed 256 KiB".to_owned());
    }
    Ok(CodeMcpCall {
        call_id,
        reference,
        arguments,
    })
}

pub(super) fn to_json(value: ObjectRef<'_>, depth: usize) -> Result<Value, String> {
    if depth > MAX_VALUE_DEPTH {
        return Err("Python value nesting exceeds Code Mode's limit".to_owned());
    }
    match value.type_name() {
        "NoneType" => Ok(Value::Null),
        "bool" => value
            .as_bool()
            .map(Value::Bool)
            .ok_or_else(|| "invalid Python bool".to_owned()),
        "int" => value
            .as_int()
            .map(|number| Value::Number(number.into()))
            .ok_or_else(|| "Python integer does not fit in a signed 64-bit JSON number".to_owned()),
        "float" => value
            .as_float()
            .and_then(Number::from_f64)
            .map(Value::Number)
            .ok_or_else(|| "Python float is not a finite JSON number".to_owned()),
        "str" => value
            .as_str()
            .map(|text| Value::String(text.to_owned()))
            .ok_or_else(|| "invalid Python string".to_owned()),
        "list" | "tuple" => {
            let items = value
                .items()
                .ok_or_else(|| "invalid Python sequence".to_owned())?;
            items
                .into_iter()
                .map(|item| to_json(item, depth + 1))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array)
        }
        "dict" => {
            let pairs = value
                .pairs()
                .ok_or_else(|| "invalid Python dictionary".to_owned())?;
            let mut output = Map::new();
            let mut seen = HashSet::with_capacity(pairs.len());
            for (key, entry) in pairs {
                let key = key
                    .as_str()
                    .ok_or_else(|| "Python dictionary keys must be strings".to_owned())?;
                if !seen.insert(key.to_owned()) {
                    return Err("Python dictionary repeats a JSON key".to_owned());
                }
                output.insert(key.to_owned(), to_json(entry, depth + 1)?);
            }
            Ok(Value::Object(output))
        }
        kind => Err(format!(
            "Python `{kind}` cannot cross the Code Mode JSON boundary"
        )),
    }
}

pub(super) fn from_json(value: &Value, depth: usize) -> Result<MontyObject, String> {
    if depth > MAX_VALUE_DEPTH {
        return Err("MCP result nesting exceeds Code Mode's limit".to_owned());
    }
    match value {
        Value::Null => Ok(MontyObject::none()),
        Value::Bool(value) => Ok(MontyObject::bool(*value)),
        Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                Ok(MontyObject::int(value))
            } else if value.is_u64() {
                Err("MCP result integer exceeds Python boundary's signed 64-bit limit".to_owned())
            } else {
                value
                    .as_f64()
                    .map(MontyObject::float)
                    .ok_or_else(|| "MCP result has an unsupported number".to_owned())
            }
        }
        Value::String(value) => Ok(MontyObject::string(value.clone())),
        Value::Array(values) => values
            .iter()
            .map(|value| from_json(value, depth + 1))
            .collect::<Result<Vec<_>, _>>()
            .map(MontyObject::list),
        Value::Object(values) => values
            .iter()
            .map(|(key, value)| {
                Ok((
                    MontyObject::string(key.clone()),
                    from_json(value, depth + 1)?,
                ))
            })
            .collect::<Result<Vec<_>, String>>()
            .map(MontyObject::dict),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::from_json;

    #[test]
    fn rejects_large_unsigned_mcp_integers_instead_of_rounding_them() {
        let result = from_json(&json!(u64::MAX), 0);
        assert!(result.is_err_and(|error| error.contains("signed 64-bit limit")));
    }
}
