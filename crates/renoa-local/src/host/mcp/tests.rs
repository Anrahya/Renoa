use std::{fs, path::Path, process::Command, sync::Arc};

use tempfile::tempdir;

use super::LocalHost;
use crate::{
    LocalHostAdapters, LocalModelConfiguration, ModelProvider, alpha_profile,
    mcp::{McpAuthorizationResolver, McpConnectionAuth, McpCredentialResolver},
};

const TOKEN: &str = "fixture-github-secret-token";

#[tokio::test]
async fn gh_reference_resolves_only_for_adapter_stdin_and_never_enters_host_state() {
    let directory = tempdir().expect("temporary authenticated MCP fixture");
    let fixtures = directory.path().join("fixtures");
    let data = directory.path().join("data");
    fs::create_dir(&fixtures).expect("create fixture directory");
    let arguments = fixtures.join("gh-arguments.txt");
    let gh = compile_fake_gh(&fixtures, &arguments);
    let adapter = write_fake_adapter(&fixtures);
    let mut host = LocalHost::new(
        &data,
        LocalModelConfiguration::new(
            fixtures.join("unused-model-adapter.mjs"),
            vec![ModelProvider::Xai],
            ModelProvider::Xai,
            "unused-model",
            fixtures.join("unused-credentials.sqlite3"),
        ),
        vec![alpha_profile()],
        LocalHostAdapters::new(Some(&adapter)),
    )
    .expect("create Host");
    let config = Arc::get_mut(&mut host.config).expect("test owns Host config");
    config.mcp_authorizations = McpAuthorizationResolver::new(
        &config.mcp_catalog,
        config.mcp_adapter.clone(),
        McpCredentialResolver::with_gh_executable(gh),
    );
    host.register_gh_cli_mcp_connection(
        "github",
        "github",
        "https://example.com/mcp",
        "github.com",
        "Anrahya",
    )
    .await
    .expect("register gh reference");

    let snapshot = host
        .refresh_mcp_catalog("github")
        .await
        .expect("refresh authenticated catalog");

    assert_eq!(snapshot.tools().len(), 1);
    assert_eq!(snapshot.tools()[0].description(), "token [REDACTED]");
    assert_eq!(
        fs::read_to_string(arguments).expect("read exact gh arguments"),
        "auth\ntoken\n--hostname\ngithub.com\n--user\nAnrahya"
    );
    let database = fs::read(data.join("host.sqlite3")).expect("read Host database");
    assert!(
        !database
            .windows(TOKEN.len())
            .any(|window| window == TOKEN.as_bytes())
    );
    let config = host
        .config
        .mcp_catalog
        .connection_config("github")
        .expect("load stored connection");
    assert_eq!(
        config.auth,
        McpConnectionAuth::GhCli {
            hostname: "github.com".to_owned(),
            account: "Anrahya".to_owned(),
        }
    );
}

fn compile_fake_gh(directory: &Path, arguments: &Path) -> std::path::PathBuf {
    let source = directory.join("fake-gh.rs");
    let executable = directory.join(if cfg!(windows) {
        "fake-gh.exe"
    } else {
        "fake-gh"
    });
    fs::write(
        &source,
        format!(
            r#"
fn main() {{
    std::fs::write(
        {arguments:?},
        std::env::args().skip(1).collect::<Vec<_>>().join("\n"),
    ).expect("write arguments");
    println!({TOKEN:?});
}}
"#
        ),
    )
    .expect("write fake gh source");
    let status = Command::new("rustc")
        .args(["--edition", "2024"])
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .status()
        .expect("compile fake gh");
    assert!(status.success(), "fake gh compilation failed: {status}");
    executable
}

fn write_fake_adapter(directory: &Path) -> std::path::PathBuf {
    let adapter = directory.join("fake-mcp-adapter.mjs");
    fs::write(
        &adapter,
        format!(
            r#"
let input = "";
for await (const chunk of process.stdin) input += chunk;
const request = JSON.parse(input);
if (
  request.wire_version !== 9 ||
  request.action !== "discover" ||
  request.credential?.scheme !== "header" ||
  request.credential?.name !== "authorization" ||
  request.credential?.prefix !== "Bearer " ||
  request.credential?.secret !== {token} ||
  process.argv.slice(2).length !== 0 ||
  Object.values(process.env).some(value => value?.includes({token}))
) process.exit(9);
process.stdout.write(JSON.stringify({{
  wire_version: 9,
  event: "discovered",
  catalog: {{
    endpoint: request.endpoint,
    protocol_version: "2026-07-28",
    adapter_revision: "mcp-client-node-v0.9.0",
    tools: [{{
      name: "get_me",
      description: `token ${{request.credential.secret}}`,
      input_schema: {{type: "object"}},
      model_input_schema: {{type: "object"}}
    }}],
    rejected_tools: []
  }}
}}) + "\n");
"#,
            token = serde_json::to_string(TOKEN).expect("encode fixture token")
        ),
    )
    .expect("write fake MCP adapter");
    adapter
}
