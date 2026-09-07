use super::*;

pub(super) async fn recover_pending(
    tool: &ManageTool,
    sink: &CredentialSubmitter,
    directory: &std::path::Path,
    database: &std::path::Path,
    connection: &str,
) {
    assert!(matches!(
        sink.catalog.connection_config(connection),
        Err(McpHostError::NotFound(_))
    ));
    let db = rusqlite::Connection::open(database).expect("Host database");
    assert_eq!(db.execute("UPDATE mcp_oauth_flows SET expires_at_ms=1 WHERE connection_id=?1 AND phase='awaiting_callback'",[connection]).expect("expire waiting consent"),1);
    fs::remove_file(directory.join("wait-for-consent")).expect("allow next authorization");
    let installed = tool.manager.list().await.expect("retained packages");
    assert_eq!(installed.len(), 1);
    for (call_id, restart) in [
        ("expired-connect", false),
        ("restart-connect", true),
        ("restart-connect", true),
    ] {
        let output = invoke_tool(Some(tool),ToolCall {
            id:call_id.to_owned(), name:TOOL_NAME.to_owned(),
            arguments:json!({"action":"connect","package_digest":installed[0].digest(),"server":"credential-test","connection":connection,"credential":{"kind":"oauth"},"restart":restart}),
            thought_signature:None,namespace:None,
        },CancellationToken::new(),Some(sink)).await.expect("tool result");
        assert_eq!(output.is_error, !restart, "{output:?}");
        if !restart {
            let [ContentBlock::Text { text }] = output.content.as_slice() else {
                panic!("error text")
            };
            assert!(text.contains("connect"));
            assert!(text.contains("restart=true"));
        }
    }
    assert_eq!(
        fs::read_to_string(directory.join("oauth-begins"))
            .expect("OAuth begin log")
            .lines()
            .count(),
        2
    );
}
