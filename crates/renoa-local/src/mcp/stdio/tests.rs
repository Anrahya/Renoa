use std::path::Path;

use super::{
    MAX_ARG_BYTES, MAX_ARGS, MAX_COMMAND_BYTES, MAX_CWD_BYTES, MAX_ENV_KEY_BYTES,
    MAX_ENV_VALUE_BYTES, MAX_ENV_VARS,
};

/// The adapter re-checks every stdio launch at its own boundary; each limit it
/// applies must be the Host's.
#[test]
fn the_adapter_applies_the_hosts_stdio_limits() {
    let limits = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../adapters/mcp-client-node/src/limits.ts"),
    )
    .expect("read the adapter limits");
    let adapter = |name: &str| -> usize {
        let line = limits
            .lines()
            .find(|line| line.starts_with(&format!("export const {name} = ")))
            .unwrap_or_else(|| panic!("the adapter defines {name}"));
        line.split_once(" = ")
            .and_then(|(_, value)| value.strip_suffix(';'))
            .expect("a constant expression")
            .split('*')
            .map(|factor| {
                factor
                    .trim()
                    .replace('_', "")
                    .parse::<usize>()
                    .expect("a whole-number factor")
            })
            .product()
    };
    for (name, host) in [
        ("MAX_STDIO_COMMAND_BYTES", MAX_COMMAND_BYTES),
        ("MAX_STDIO_ARGS", MAX_ARGS),
        ("MAX_STDIO_ARG_BYTES", MAX_ARG_BYTES),
        ("MAX_STDIO_ENV_VARS", MAX_ENV_VARS),
        ("MAX_STDIO_ENV_KEY_BYTES", MAX_ENV_KEY_BYTES),
        ("MAX_STDIO_ENV_VALUE_BYTES", MAX_ENV_VALUE_BYTES),
        ("MAX_STDIO_CWD_BYTES", MAX_CWD_BYTES),
    ] {
        assert_eq!(adapter(name), host, "{name}");
    }
}
