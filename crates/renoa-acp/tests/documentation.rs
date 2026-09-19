//! Documentation must name every environment variable its parser reads.

/// (document, parser, document text, parser source, only direct environment reads count).
const DOCS: &[(&str, &str, &str, &str, bool)] = &[
    (
        "docs/acp-v1.md",
        "crates/renoa-acp/src/config.rs",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/acp-v1.md")),
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/config.rs")),
        false,
    ),
    (
        "crates/renoa-telegram/README.md",
        "crates/renoa-telegram/src/config.rs",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../renoa-telegram/README.md"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../renoa-telegram/src/config.rs"
        )),
        false,
    ),
    (
        "crates/renoa-slack/README.md",
        "crates/renoa-slack/src/config.rs",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../renoa-slack/README.md"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../renoa-slack/src/config.rs"
        )),
        true,
    ),
];

fn is_environment_name(literal: &str) -> bool {
    literal.starts_with("RENOA_")
        && literal
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

fn string_literals(source: &str) -> Vec<&str> {
    let mut literals = Vec::new();
    let mut rest = source;
    while let Some(opening) = rest.find('"') {
        rest = &rest[opening + 1..];
        let Some(closing) = rest.find('"') else { break };
        literals.push(&rest[..closing]);
        rest = &rest[closing + 1..];
    }
    literals
}

fn environment_names(source: &str, environment_reads_only: bool) -> Vec<&str> {
    let mut names = Vec::new();
    for literal in string_literals(source) {
        let direct_read = ["var_os(\"", "var(\""].iter().any(|call| {
            source
                .match_indices(call)
                .any(|(offset, _)| source[offset + call.len()..].starts_with(literal))
        });
        if is_environment_name(literal)
            && !names.contains(&literal)
            && (!environment_reads_only || direct_read)
        {
            names.push(literal);
        }
    }
    names
}

#[test]
fn documentation_names_every_environment_variable_its_parser_reads() {
    let mut missing = Vec::new();
    for &(doc, parser, text, source, environment_reads_only) in DOCS {
        for name in environment_names(source, environment_reads_only) {
            if !text.contains(name) {
                missing.push(format!("{doc} does not mention {name}, read by {parser}"));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "documentation is missing environment variables:\n{}",
        missing.join("\n")
    );
}
