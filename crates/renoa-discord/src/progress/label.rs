//! How a tool call is named in a progress message.
//!
//! Host plugin tools run through generic dispatch tools: `tool_execute` names
//! the tool in its `reference` argument, and a `code_mode` script calls tools
//! by reference in its source. A reference reads
//! `<kind>:<connection>:<digest>:<tool>`, and is shown as `connection.tool`.
//! This is presentation only: anything unrecognized shows the tool's own name.

use serde_json::Value;

/// The most tools one `code_mode` call is labelled with.
const MAX_SCRIPT_TOOLS: usize = 3;

pub(super) fn tool_label(name: &str, arguments: &Value) -> String {
    let label = match name {
        "tool_execute" => arguments
            .get("reference")
            .and_then(Value::as_str)
            .and_then(reference_label),
        "code_mode" => arguments
            .get("source")
            .and_then(Value::as_str)
            .and_then(script_label),
        _ => None,
    };
    label.unwrap_or_else(|| name.to_owned())
}

fn reference_label(reference: &str) -> Option<String> {
    let mut parts = reference.split(':');
    let (_kind, connection, digest, tool) =
        (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some()
        || connection.is_empty()
        || tool.is_empty()
        || digest.len() != 64
        || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    Some(format!("{connection}.{tool}"))
}

fn script_label(source: &str) -> Option<String> {
    let mut tools: Vec<String> = Vec::new();
    for (start, _) in source.match_indices("mcp:") {
        let reference: String = source[start..]
            .chars()
            .take_while(|character| {
                character.is_ascii_alphanumeric() || matches!(character, ':' | '_' | '-' | '.')
            })
            .collect();
        if let Some(label) = reference_label(&reference)
            && !tools.contains(&label)
        {
            tools.push(label);
        }
    }
    if tools.is_empty() {
        return None;
    }
    let more = tools.len() > MAX_SCRIPT_TOOLS;
    tools.truncate(MAX_SCRIPT_TOOLS);
    Some(format!(
        "code: {}{}",
        tools.join(", "),
        if more { ", …" } else { "" }
    ))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::tool_label;

    const DIGEST: &str = "24a8fdd19b0418aba86855b531f34897c83e4be1f6dbae06da40512c0fefb219";

    #[test]
    fn a_dispatched_plugin_tool_is_named_by_its_connection_and_tool() {
        let arguments = json!({
            "reference": format!("mcp:exa:{DIGEST}:web_search_exa"),
            "arguments": {"query": "news"},
        });
        assert_eq!(tool_label("tool_execute", &arguments), "exa.web_search_exa");
    }

    #[test]
    fn a_script_is_named_by_the_distinct_tools_it_calls() {
        let source = format!(
            "a = await plugin(\"mcp:exa:{DIGEST}:web_search_exa\", {{}})\n\
             b = await plugin(\"mcp:exa:{DIGEST}:web_search_exa\", {{}})\n\
             c = await plugin('mcp:notion:{DIGEST}:search', {{}})"
        );
        assert_eq!(
            tool_label("code_mode", &json!({ "source": source })),
            "code: exa.web_search_exa, notion.search"
        );
    }

    #[test]
    fn anything_unrecognized_keeps_the_tool_name() {
        assert_eq!(tool_label("bash", &json!({"command": "ls"})), "bash");
        assert_eq!(
            tool_label(
                "tool_execute",
                &json!({"reference": "mcp:exa:short:web_search_exa"})
            ),
            "tool_execute"
        );
        assert_eq!(
            tool_label("code_mode", &json!({"source": "print(1)"})),
            "code_mode"
        );
    }
}
