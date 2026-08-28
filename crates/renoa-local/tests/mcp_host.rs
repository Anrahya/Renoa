use std::{
    io::{Read as _, Write as _},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
    thread,
    time::Duration,
};

use renoa_local::{LocalHost, LocalHostAdapters, LocalHostError, ModelProvider};
use serde_json::{Value, json};
use tempfile::tempdir;

#[path = "mcp_host/vertical.rs"]
mod vertical;

#[tokio::test]
async fn host_discovers_enables_and_restores_one_real_mcp_connection() {
    let workspace = workspace_root();
    let adapter = compiled_adapter(&workspace);
    let directory = tempdir().expect("temporary directory");
    let data = directory.path().join("data");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind MCP fixture");
    let address = listener.local_addr().expect("MCP fixture address");
    let endpoint = format!("http://127.0.0.1:{}/mcp", address.port());
    let server = thread::spawn(move || serve_discovery(&listener));
    let host = new_host(&data, Some(&adapter));
    host.register_direct_mcp_connection("fixture", "primary", &endpoint)
        .await
        .expect("register MCP piece");

    let refreshed = host.refresh_mcp_catalog("primary").await;
    let _ = TcpStream::connect(address);
    server.join().expect("MCP fixture thread");
    let refreshed = refreshed.expect("refresh through compiled MCP adapter");
    assert_eq!(refreshed.connection_id(), "primary");
    assert_eq!(refreshed.endpoint(), endpoint);
    assert_eq!(refreshed.tools().len(), 1);
    assert_eq!(refreshed.tools()[0].name(), "echo");
    assert_eq!(refreshed.rejected_tools().len(), 1);
    assert_eq!(refreshed.rejected_tools()[0].name(), Some("bad name"));
    host.enable_alpha_mcp_connection("primary")
        .await
        .expect("enable connection for Alpha");
    drop(host);

    assert!(data.join("host.sqlite3").is_file());
    assert!(data.join("sessions").is_dir());
    let reopened = new_host(&data, Some(&adapter));
    assert_eq!(
        reopened
            .mcp_catalog("primary")
            .await
            .expect("restore catalog"),
        refreshed
    );
    let enabled = reopened
        .alpha_mcp_connection_ids()
        .await
        .expect("restore Alpha connection binding");
    assert_eq!(enabled, ["primary"]);
}

#[tokio::test]
async fn catalog_refresh_requires_an_explicit_mcp_adapter() {
    let directory = tempdir().expect("temporary directory");
    let host = new_host(&directory.path().join("data"), None);
    host.register_direct_mcp_connection("fixture", "primary", "http://127.0.0.1:43127/mcp")
        .await
        .expect("registration does not need an adapter process");

    let error = host
        .refresh_mcp_catalog("primary")
        .await
        .expect_err("refresh must not guess an adapter");

    assert!(matches!(error, LocalHostError::Configuration(_)));
}

fn new_host(data: &Path, adapter: Option<&Path>) -> LocalHost {
    LocalHost::new(
        data,
        data.join("unused-model-adapter.mjs"),
        vec![ModelProvider::Xai],
        ModelProvider::Xai,
        "unused-model",
        data.join("unused-credentials.sqlite3"),
        LocalHostAdapters::new(adapter, None),
    )
    .expect("create local Host")
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/renoa-local is two levels below the workspace")
        .to_path_buf()
}

fn compiled_adapter(workspace: &Path) -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT
        .get_or_init(|| {
            let root = workspace.join("adapters/mcp-client-node");
            let adapter = root.join("dist/src/main.js");
            let status = Command::new("pnpm")
                .args([
                    "--dir",
                    root.to_str().expect("adapter path is UTF-8"),
                    "build",
                ])
                .status()
                .expect("build @renoa/mcp-client");
            assert!(status.success(), "MCP adapter build failed: {status}");
            assert!(
                adapter.is_file(),
                "MCP adapter build produced no entrypoint"
            );
            adapter
        })
        .clone()
}

fn serve_discovery(listener: &TcpListener) {
    for expected_method in ["server/discover", "tools/list"] {
        let (mut stream, _) = listener.accept().expect("accept MCP fixture request");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("configure MCP request timeout");
        let request = read_http_request(&mut stream).expect("read MCP request");
        if request.is_empty() {
            return;
        }
        let body = request
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|index| &request[index + 4..])
            .expect("MCP HTTP body");
        let rpc: Value = serde_json::from_slice(body).expect("decode MCP JSON-RPC request");
        assert_eq!(rpc["method"], expected_method);
        let result = if expected_method == "server/discover" {
            json!({
                "resultType": "complete",
                "supportedVersions": ["2026-07-28"],
                "capabilities": {"tools": {}}
            })
        } else {
            json!({
                "resultType": "complete",
                "tools": [
                    {
                        "name": "echo",
                        "description": "Echo one string.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {"text": {"type": "string"}},
                            "required": ["text"]
                        }
                    },
                    {
                        "name": "bad name",
                        "inputSchema": {"type": "object"}
                    }
                ],
                "ttlMs": 0,
                "cacheScope": "private"
            })
        };
        let response = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": rpc["id"],
            "result": result
        }))
        .expect("encode MCP JSON-RPC response");
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            response.len()
        )
        .expect("write MCP response headers");
        stream
            .write_all(&response)
            .expect("write MCP response body");
        stream.flush().expect("flush MCP response");
    }
}

fn read_http_request(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 8_192];
    let mut expected_length = None;
    loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Ok(request);
        }
        request.extend_from_slice(&buffer[..read]);
        if expected_length.is_none()
            && let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n")
        {
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.split_once(':')
                        .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .map(|(_, value)| value.trim())
                })
                .ok_or_else(|| std::io::Error::other("missing Content-Length"))?
                .parse::<usize>()
                .map_err(std::io::Error::other)?;
            expected_length = Some(header_end + 4 + content_length);
        }
        if expected_length.is_some_and(|length| request.len() >= length) {
            return Ok(request);
        }
    }
}
