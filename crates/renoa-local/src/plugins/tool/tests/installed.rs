use super::*;

#[tokio::test]
async fn installed_reuse_rejects_connection_fields_before_any_profile_mutation() {
    let fixture = LocalPackageFixture::new();
    crate::plugins::tests::write_exa_plugin(
        &fixture.directory.path().join("source"),
        "https://example.com/mcp",
    );
    let digest = fixture.digest().await;
    call(
        &fixture.tool,
        json!({"action":"install","source_path":"source","expected_digest":digest}),
    )
    .await;
    let profile = crate::AgentProfileId::new(crate::ALPHA_PROFILE_ID).expect("profile");
    for (key, value) in [
        ("server", json!("exa")),
        ("connection", json!("new")),
        ("credential", json!({"kind":"oauth"})),
        ("replace", json!(true)),
    ] {
        let mut arguments =
            json!({"action":"add","source":{"kind":"installed","package_digest":digest}});
        arguments[key] = value;
        let output = invoke_tool(
            Some(&fixture.tool),
            ToolCall {
                id: format!("invalid-reuse-{key}"),
                name: TOOL_NAME.to_owned(),
                arguments,
                thought_signature: None,
                namespace: None,
            },
            CancellationToken::new(),
            None,
        )
        .await
        .expect("definite rejection");
        assert!(output.is_error);
        assert!(output.content.iter().any(|block| matches!(block, ContentBlock::Text { text } if text.contains("reuse only enables skills"))));
    }
    assert!(
        fixture
            .skills
            .summaries(profile.as_str(), fixture.directory.path())
            .expect("no skill attachment")
            .is_empty()
    );
    assert!(
        fixture
            .tool
            .manager
            .connection_statuses(&profile)
            .await
            .expect("no connections")
            .is_empty()
    );
    let database = rusqlite::Connection::open(fixture.directory.path().join("host.sqlite3"))
        .expect("database");
    for table in [
        "mcp_connections",
        "mcp_catalogs",
        "mcp_oauth_flows",
        "mcp_oauth_receipts",
    ] {
        let count: i64 = database
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .expect("no credential or catalog mutation");
        assert_eq!(count, 0, "{table} changed");
    }
}

#[tokio::test]
async fn installed_reuse_is_idempotent_and_requires_a_verified_host_package() {
    let fixture = LocalPackageFixture::new();
    let digest = fixture.digest().await;
    call(&fixture.tool, package_add(&digest, None)).await;
    fs::remove_dir_all(fixture.directory.path().join("source")).expect("remove original source");
    let reuse = json!({"action":"add","source":{"kind":"installed","package_digest":digest}});
    let first = call(&fixture.tool, reuse.clone()).await;
    assert_eq!(first["source"], "installed");
    assert_eq!(first["skills"]["accepted"], json!(["review"]));
    assert_eq!(first, call(&fixture.tool, reuse).await);
    assert_eq!(
        fixture
            .tool
            .manager
            .list()
            .await
            .expect("one package")
            .len(),
        1
    );

    for invalid_source in [
        json!({"kind":"installed","package_digest":"a".repeat(64)}),
        json!({"kind":"installed","package_digest":"../../outside"}),
        json!({"kind":"installed","package_digest":digest,"source_path":"source"}),
    ] {
        let output = invoke_tool(
            Some(&fixture.tool),
            ToolCall {
                id: "invalid-reuse".to_owned(),
                name: TOOL_NAME.to_owned(),
                arguments: json!({"action":"add","source":invalid_source}),
                thought_signature: None,
                namespace: None,
            },
            CancellationToken::new(),
            None,
        )
        .await
        .expect("definite rejection");
        assert!(output.is_error);
    }
    assert_eq!(
        fixture
            .tool
            .manager
            .list()
            .await
            .expect("no extra packages")
            .len(),
        1
    );
}
