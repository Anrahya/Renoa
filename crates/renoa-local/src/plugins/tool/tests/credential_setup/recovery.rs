use super::*;

/// The sign-in after the credential needs a link of its own, which the
/// replay cannot show, so the agent makes the same call again. Returns whether
/// the connection was published.
pub(super) async fn sign_in_again(
    run: &Run<'_>,
    consent: Consent,
    replayed: Result<String, String>,
) -> bool {
    let stopped = replayed.expect_err("the sign-in needs the person");
    assert!(stopped.contains("same arguments"), "{stopped}");
    let signed_in = call(run.tool, run.sink, "credential-sign-in", run.arguments).await;
    match consent {
        Consent::Given | Consent::Unhanded | Consent::Duplicate => {
            assert!(!signed_in.is_error, "{signed_in:?}");
            // A saved restart replayed with no one to show its new link to is
            // refused before it starts a sign-in.
            let restart =
                json!({"action": "authorize", "connection": run.connection, "restart": true});
            let operation =
                crate::mcp::oauth_operation_id(run.session, Some(run.command), "unshown");
            let refused = crate::plugins::replay_manage_call(
                &run.tool.manager,
                test_agent_id(1),
                run.directory,
                &restart,
                &operation,
                CancellationToken::new(),
            );
            let refused = tokio::time::timeout(Duration::from_secs(30), refused)
                .await
                .expect("refused without waiting on a link")
                .expect_err("a restart nobody can see");
            assert!(refused.contains("same arguments"), "{refused}");
            let begins =
                fs::read_to_string(run.directory.join("oauth-begins")).expect("OAuth begin log");
            assert_eq!(begins.lines().count(), 1, "no second sign-in began");
        }
        Consent::Callback => {
            assert_waiting(&signed_in);
            answer_sign_in(run.sink, "code=sign-in-code").await;
            let finished = replay(run, "credential-sign-in").await;
            let finished = finished.expect("the replay finishes the sign-in");
            assert!(finished.contains("catalog_loaded"), "{finished}");
        }
        Consent::Refused => {
            assert_waiting(&signed_in);
            answer_sign_in(run.sink, "error=access_denied").await;
            let refused = replay(run, "credential-sign-in").await;
            let refused = refused.expect_err("a refused sign-in did not finish");
            assert!(refused.contains("access_denied"), "{refused}");
            return false;
        }
        Consent::Expires => {
            assert_waiting(&signed_in);
            assert_eq!(awaiting_callbacks(run.database, run.connection), 1);
            recover_pending(
                run.tool,
                run.sink,
                run.directory,
                run.database,
                run.connection,
            )
            .await;
        }
    }
    true
}

/// The person leaves the first credential link unanswered and answers a second
/// setup of the same credential; the wait on the first sees it saved.
pub(super) async fn duplicate_setup(run: &Run<'_>) -> bool {
    let first = Box::pin(replay(run, "credential-connect"));
    let second = Box::pin(async {
        let again = call(run.tool, run.sink, "credential-again", run.arguments).await;
        assert_waiting(&again);
        replay(run, "credential-again").await
    });
    let (first, second) = tokio::time::timeout(Duration::from_secs(30), async {
        tokio::join!(first, second)
    })
    .await
    .expect("the first wait ends once the credential is saved");
    for finished in [first, second] {
        let finished = finished.expect("each setup finishes");
        assert!(finished.contains("catalog_loaded"), "{finished}");
    }
    true
}

/// The Host's setup wait replaying the agent's call `call_id`.
pub(super) async fn replay(run: &Run<'_>, call_id: &str) -> Result<String, String> {
    let operation = crate::mcp::oauth_operation_id(run.session, Some(run.command), call_id);
    let replayed = crate::plugins::replay_manage_call(
        &run.tool.manager,
        test_agent_id(1),
        run.directory,
        run.arguments,
        &operation,
        CancellationToken::new(),
    );
    // A replay that waited on a link nobody was shown would hang here.
    tokio::time::timeout(Duration::from_secs(30), replayed)
        .await
        .expect("the replay never waits on a link nobody saw")
}

/// The person follows the last sign-in link, and the provider sends them back
/// with `answer`.
async fn answer_sign_in(sink: &CredentialSubmitter, answer: &str) {
    let shown = sink.sign_in.lock().expect("sign-in link").clone();
    let link = Url::parse(&shown.expect("a sign-in link was shown")).expect("sign-in URL");
    let query = |name: &str| {
        link.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
            .expect("sign-in link parameter")
    };
    let callback = reqwest::Client::new()
        .get(format!(
            "{}?{answer}&state={}",
            query("redirect_uri"),
            query("state")
        ))
        .send()
        .await
        .expect("send provider callback");
    assert_eq!(callback.status(), reqwest::StatusCode::OK);
}

async fn recover_pending(
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
