use std::{fs, path::Path, process::Command};

use tempfile::{TempDir, tempdir};
use tokio_util::sync::CancellationToken;

use super::super::{McpAuthorizationResolver, secret::OAuthSecretBundle};
use crate::mcp::{
    McpCatalogStore, McpConnectionAuth, McpCredentialResolver, McpOAuthRegistration,
    McpRequestHeaders,
};

pub(super) const CONNECTION: &str = "oauth-fixture";
pub(super) const ENDPOINT: &str = "https://mcp.example.test/mcp";

pub(super) struct Fixture {
    pub(super) _directory: TempDir,
    pub(super) store: McpCatalogStore,
    pub(super) resolver: McpAuthorizationResolver,
    pub(super) auth: McpConnectionAuth,
    pub(super) actions: std::path::PathBuf,
    pub(super) secret_writes: std::path::PathBuf,
    browser: std::path::PathBuf,
}

impl Fixture {
    pub(super) fn new() -> Self {
        let directory = tempdir().expect("temporary OAuth fixture");
        let database = directory.path().join("host.sqlite3");
        let store = McpCatalogStore::initialize(database).expect("initialize Host catalog");
        let auth = McpConnectionAuth::oauth(CONNECTION, ENDPOINT, McpOAuthRegistration::dynamic())
            .expect("OAuth connection reference");
        store
            .register_connection(
                "oauth-integration",
                CONNECTION,
                ENDPOINT,
                &McpRequestHeaders::default(),
                &auth,
            )
            .expect("register OAuth connection");
        let actions = directory.path().join("adapter-actions.txt");
        let secret_writes = directory.path().join("secret-writes.txt");
        let secret_data = directory.path().join("secret-data.json");
        let adapter = write_adapter(directory.path(), &actions);
        let no_op = compile_no_op(directory.path());
        let browser = compile_browser(directory.path());
        let secret_tool = compile_secret_tool(directory.path(), &secret_data, &secret_writes);
        let credentials = McpCredentialResolver::with_executables(no_op.clone(), secret_tool);
        let mut resolver = McpAuthorizationResolver::new(&store, Some(adapter), credentials);
        resolver.oauth.browser = no_op;
        Self {
            _directory: directory,
            store,
            resolver,
            auth,
            actions,
            secret_writes,
            browser,
        }
    }

    pub(super) fn enable_callback_browser(&mut self) {
        self.resolver.oauth.browser.clone_from(&self.browser);
    }

    pub(super) async fn secret_bundle(&self) -> OAuthSecretBundle {
        let credential = self.auth.oauth_credential_id().expect("fixture uses OAuth");
        self.resolver
            .oauth
            .secrets
            .load(credential, CancellationToken::new())
            .await
            .expect("load OAuth secret bundle")
            .expect("OAuth secret bundle exists")
    }

    pub(super) async fn store_bundle(&self, bundle: &OAuthSecretBundle) {
        let credential = self.auth.oauth_credential_id().expect("fixture uses OAuth");
        self.resolver
            .oauth
            .secrets
            .store(credential, bundle, CancellationToken::new())
            .await
            .expect("store OAuth secret bundle");
    }

    pub(super) fn action_count(&self, action: &str) -> usize {
        lines(&self.actions)
            .iter()
            .filter(|candidate| *candidate == action)
            .count()
    }

    pub(super) fn secret_write_count(&self) -> usize {
        lines(&self.secret_writes).len()
    }
}

fn lines(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn write_adapter(directory: &Path, actions: &Path) -> std::path::PathBuf {
    let path = directory.join("oauth-adapter.mjs");
    let actions = serde_json::to_string(&actions.to_string_lossy()).expect("encode action path");
    fs::write(
        &path,
        format!(
            r"
import fs from 'node:fs';
let input = '';
for await (const chunk of process.stdin) input += chunk;
const request = JSON.parse(input);
fs.appendFileSync({actions}, `${{request.action}}\n`);
const send = value => process.stdout.write(`${{JSON.stringify(value)}}\n`);
if (request.action === 'oauth_begin') {{
  const authorization = new URL(request.redirect_uri);
  authorization.searchParams.set('code', 'code-one');
  authorization.searchParams.set('state', request.csrf_state);
  authorization.searchParams.set('redirect_uri', request.redirect_uri);
  const state = {{
    schema_version: 1,
    mcp_endpoint: request.endpoint,
    csrf_state: request.csrf_state,
    redirect_uri: request.redirect_uri,
    authorization_url: authorization.href
  }};
  send({{
    wire_version: 7,
    event: 'oauth_redirect',
    authorization_url: authorization.href,
    oauth_state: state
  }});
}} else if (request.action === 'oauth_exchange') {{
  if (request.authorization_code !== 'code-one') process.exit(11);
  if (request.oauth_state.reject_exchange === true) {{
    send({{
      wire_version: 7,
      event: 'oauth_failed',
      failure: {{
        kind: 'protocol',
        certainty: 'definite',
        message: 'OAuth server rejected the credential request.',
        partial_changes_possible: true,
        diagnostic: {{code: 'invalid_grant', detail: 'authorization code was rejected'}}
      }},
      oauth_state: request.oauth_state
    }});
    process.exit(0);
  }}
  const state = {{
    ...request.oauth_state,
    access_token: 'access-one',
    needs_refresh: false
  }};
  send({{
    wire_version: 7,
    event: 'oauth_authorized',
    authorization: {{scheme: 'bearer', token: 'access-one'}},
    oauth_state: state
  }});
}} else if (request.action === 'oauth_token') {{
  if (request.oauth_state.needs_refresh === true) {{
    send({{
      wire_version: 7,
      event: 'oauth_refresh_required',
      oauth_state: request.oauth_state
    }});
  }} else {{
    send({{
      wire_version: 7,
      event: 'oauth_authorized',
      authorization: {{scheme: 'bearer', token: request.oauth_state.access_token}},
      oauth_state: request.oauth_state
    }});
  }}
}} else if (request.action === 'oauth_refresh') {{
  if (request.oauth_state.lose_refresh === true) process.exit(12);
  const state = {{
    ...request.oauth_state,
    access_token: 'access-two',
    needs_refresh: false
  }};
  send({{
    wire_version: 7,
    event: 'oauth_authorized',
    authorization: {{scheme: 'bearer', token: 'access-two'}},
    oauth_state: state
  }});
}} else {{
  process.exit(13);
}}
"
        ),
    )
    .expect("write OAuth adapter");
    path
}

fn compile_no_op(directory: &Path) -> std::path::PathBuf {
    compile(directory, "oauth-no-op", "fn main() {}")
}

fn compile_browser(directory: &Path) -> std::path::PathBuf {
    compile(
        directory,
        "oauth-browser",
        r#"
use std::{io::Write as _, net::TcpStream};

fn main() {
    let url = std::env::args().nth(1).expect("authorization URL");
    let rest = url.strip_prefix("http://127.0.0.1:").expect("loopback URL");
    let (port, target) = rest.split_once('/').expect("URL path");
    let mut stream = TcpStream::connect(("127.0.0.1", port.parse::<u16>().expect("port")))
        .expect("connect callback");
    write!(
        stream,
        "GET /{target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    ).expect("send callback");
}
"#,
    )
}

pub(super) fn compile_secret_tool(
    directory: &Path,
    data: &Path,
    writes: &Path,
) -> std::path::PathBuf {
    compile(
        directory,
        "oauth-secret-tool",
        &format!(
            r#"
use std::{{fs, io::Read as _, io::Write as _}};

fn main() {{
    let action = std::env::args().nth(1).expect("secret action");
    if action == "lookup" {{
        match fs::read({data:?}) {{
            Ok(bytes) => std::io::stdout().write_all(&bytes).expect("write secret"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => std::process::exit(1),
            Err(error) => panic!("read secret: {{error}}"),
        }}
    }} else if action == "store" {{
        let mut bytes = Vec::new();
        std::io::stdin().read_to_end(&mut bytes).expect("read secret");
        fs::write({data:?}, bytes).expect("store secret");
        fs::OpenOptions::new().create(true).append(true).open({writes:?})
            .expect("open write log").write_all(b"store\n").expect("log store");
    }} else {{
        std::process::exit(2);
    }}
}}
"#
        ),
    )
}

fn compile(directory: &Path, name: &str, source: &str) -> std::path::PathBuf {
    let source_path = directory.join(format!("{name}.rs"));
    let executable = directory.join(if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    });
    fs::write(&source_path, source).expect("write helper source");
    let status = Command::new("rustc")
        .args(["--edition", "2024"])
        .arg(&source_path)
        .arg("-o")
        .arg(&executable)
        .status()
        .expect("compile OAuth helper");
    assert!(
        status.success(),
        "OAuth helper compilation failed: {status}"
    );
    executable
}
