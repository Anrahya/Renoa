use rusqlite::{Connection, params};
use serde_json::Value;

use super::*;
use crate::{AgentDefinition, AgentToolSelection, plugins::host::HostPluginId};

const MACHINE_TOOLS: [&str; 6] = [
    "bash",
    "edit_file",
    "find",
    "grep",
    "read_file",
    "write_file",
];
const FORMER_HOST_TOOLS: [&str; 13] = [
    "git_changes",
    "git_diff",
    "git_show",
    "plugin_manage",
    "plugin_search",
    "tool_execute",
    "skill_search",
    "skill_load",
    "agent_manage",
    "agent_documents",
    "routine_manage",
    "routine_results",
    "code_mode",
];

struct Fixture {
    alpha: AgentDefinition,
    operator: AgentDefinition,
    protocol_only: AgentDefinition,
    unchanged: AgentDefinition,
    creation: AgentCreateRequest,
    selection: AgentToolsUpdate,
    rename_operation: Uuid,
    rename: RenameAgent,
}

async fn seed(host: &LocalHost) -> Fixture {
    let (creator, origin) = system("migration-test");
    let creation = AgentCreateRequest::from_preset(
        Uuid::new_v4(),
        AgentPresetId::new(ALPHA_PRESET_ID).unwrap(),
        "Alpha",
    );
    let alpha = host
        .create_agent(
            creator.clone(),
            origin,
            creation.clone(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let selection = AgentToolsUpdate {
        operation_id: Uuid::new_v4(),
        id: alpha.id,
        expected_revision: 1,
        tools: MACHINE_TOOLS.into_iter().map(str::to_owned).collect(),
    };
    host.set_agent_tools(selection.clone()).await.unwrap();
    let rename_operation = Uuid::new_v4();
    let rename = RenameAgent {
        id: alpha.id,
        expected_name: "Alpha".to_owned(),
        name: "Renamed".to_owned(),
    };
    let alpha = host
        .rename_agent(
            alpha.id,
            rename_operation,
            rename.clone(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let operator = host
        .create_agent(
            creator.clone(),
            origin,
            AgentCreateRequest::from_preset(
                Uuid::new_v4(),
                AgentPresetId::new(ARCEE_PRESET_ID).unwrap(),
                "Operator",
            ),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let protocol_only = host
        .create_agent(
            creator.clone(),
            origin,
            specialist(Uuid::new_v4(), "Protocol only"),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let unchanged = host
        .create_agent(
            creator,
            origin,
            specialist(Uuid::new_v4(), "Unchanged").with_tools(["read_file".to_owned()]),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    Fixture {
        alpha,
        operator,
        protocol_only,
        unchanged,
        creation,
        selection,
        rename_operation,
        rename,
    }
}

fn legacy_tools(definition: &AgentDefinition, version: u32) -> BTreeSet<String> {
    let mut tools: BTreeSet<String> = FORMER_HOST_TOOLS
        .into_iter()
        .map(str::to_owned)
        .chain(definition.tool_selection.tools.iter().cloned())
        .collect();
    if definition.operational.documents.is_none() {
        tools.remove("agent_documents");
    }
    if version < 30 {
        tools.remove("plugin_manage");
        tools.remove("plugin_search");
        tools.extend(["extension_manage", "tool_search", "tool_load"].map(str::to_owned));
    }
    tools
}

fn receipt_selection(table: &str) -> &'static str {
    if table == "host_agent_tool_selection_operations" {
        ""
    } else {
        "/tool_selection"
    }
}

fn install_legacy_selections(db: &Connection, fixture: &Fixture, version: u32) {
    for definition in [&fixture.alpha, &fixture.operator, &fixture.protocol_only] {
        let tools = legacy_tools(definition, version);
        db.execute(
            "UPDATE host_agent_tool_selections SET tools_json=?1 WHERE agent_id=?2",
            params![
                serde_json::to_string(&tools).unwrap(),
                definition.id.to_string()
            ],
        )
        .unwrap();
    }
    for table in [
        "host_agent_creations",
        "host_agent_renames",
        "host_agent_tool_selection_operations",
    ] {
        let rows: Vec<(String, String)> = db
            .prepare(&format!("SELECT operation_id,result_json FROM {table}"))
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for (operation, encoded) in rows {
            let mut result: Value = serde_json::from_str(&encoded).unwrap();
            if result.get("id").and_then(Value::as_str) == Some(&fixture.unchanged.id.to_string()) {
                continue;
            }
            let definition = if table == "host_agent_tool_selection_operations" {
                fixture.alpha.clone()
            } else {
                serde_json::from_value(result.clone()).unwrap()
            };
            let tools = legacy_tools(&definition, version);
            result.pointer_mut(receipt_selection(table)).unwrap()["tools"] =
                serde_json::to_value(&tools).unwrap();
            db.execute(
                &format!("UPDATE {table} SET result_json=?1 WHERE operation_id=?2"),
                params![result.to_string(), operation],
            )
            .unwrap();
        }
    }
    db.execute(
        "UPDATE host_metadata SET schema_version=?1 WHERE singleton=1",
        [version],
    )
    .unwrap();
    db.pragma_update(None, "user_version", version).unwrap();
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one migration lifecycle proves reads, runtime resolution, receipt replay, edits, and reopening"
)]
async fn migrated_selections_remain_usable_through_host_consumers_and_replays() {
    for version in 28..=31 {
        let (directory, initial) = fixture();
        let fixture = seed(&initial).await;
        let workspace = directory.path().join("workspace");
        let original_prompt = initial
            .resolve_definition(fixture.operator.id)
            .await
            .unwrap()
            .system_prompt(&workspace)
            .unwrap();
        let db = Connection::open(&initial.config.database).unwrap();
        install_legacy_selections(&db, &fixture, version);
        drop(db);
        drop(initial);
        let migrated = host(directory.path());
        let mut alpha = fixture.alpha.clone();
        alpha.tool_selection.revision += 1;
        let mut operator = fixture.operator.clone();
        operator.tool_selection.revision += 1;
        let mut protocol_only = fixture.protocol_only.clone();
        protocol_only.tool_selection.revision += 1;
        assert_eq!(
            migrated.agent_definition(alpha.id).await.unwrap(),
            Some(alpha.clone())
        );
        assert_eq!(
            migrated.agent_definition(operator.id).await.unwrap(),
            Some(operator.clone())
        );
        assert_eq!(
            migrated.agent_definition(protocol_only.id).await.unwrap(),
            Some(protocol_only.clone())
        );
        assert!(protocol_only.tool_selection.tools.is_empty());
        assert_eq!(
            migrated
                .agent_definition(fixture.unchanged.id)
                .await
                .unwrap(),
            Some(fixture.unchanged.clone())
        );
        assert_eq!(
            migrated
                .list_agent_definitions(None, 20)
                .await
                .unwrap()
                .agents
                .len(),
            4
        );
        for expected in [&alpha, &operator, &protocol_only] {
            let resolved = migrated.resolve_definition(expected.id).await.unwrap();
            assert_eq!(resolved.selected_tools(), &expected.tool_selection);
            for plugin in HostPluginId::ALL {
                assert!(
                    crate::plugins::host::state::enabled(
                        &migrated.config.database,
                        expected.id,
                        plugin
                    )
                    .unwrap()
                );
            }
            let bindings = crate::host::runtime::protocol_bindings(
                &migrated.config,
                &resolved,
                &workspace,
                renoa_kernel::SessionId::new(),
                None,
                Vec::new(),
            )
            .unwrap();
            assert_eq!(
                bindings
                    .iter()
                    .map(renoa_agent_loop::AgentToolBinding::tool_name)
                    .collect::<BTreeSet<_>>(),
                BTreeSet::from(["plugin_manage", "plugin_search", "tool_execute"])
            );
        }
        assert_eq!(
            migrated
                .resolve_definition(operator.id)
                .await
                .unwrap()
                .system_prompt(&workspace)
                .unwrap(),
            original_prompt
        );

        let created = migrated
            .create_agent(
                system("migration-test").0,
                system("migration-test").1,
                fixture.creation.clone(),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(created.id, alpha.id);
        assert_eq!(created.name, "Alpha");
        assert_eq!(
            created.tool_selection,
            AgentToolSelection {
                revision: 2,
                tools: alpha.tool_selection.tools.clone()
            }
        );
        assert_eq!(
            migrated
                .rename_agent(
                    alpha.id,
                    fixture.rename_operation,
                    fixture.rename.clone(),
                    CancellationToken::new()
                )
                .await
                .unwrap(),
            alpha
        );
        assert_eq!(
            migrated
                .set_agent_tools(fixture.selection.clone())
                .await
                .unwrap(),
            alpha.tool_selection
        );

        let updated = migrated
            .set_agent_tools(AgentToolsUpdate {
                operation_id: Uuid::new_v4(),
                id: alpha.id,
                expected_revision: alpha.tool_selection.revision,
                tools: BTreeSet::from(["bash".to_owned()]),
            })
            .await
            .unwrap();
        assert_eq!(updated.revision, 4);
        assert!(matches!(
            migrated
                .set_agent_tools(AgentToolsUpdate {
                    operation_id: Uuid::new_v4(),
                    id: alpha.id,
                    expected_revision: 2,
                    tools: updated.tools.clone()
                })
                .await,
            Err(LocalHostError::AgentConflict(_))
        ));
        drop(migrated);
        let reopened = host(directory.path());
        assert_eq!(
            reopened.agent_tool_selection(alpha.id).await.unwrap(),
            updated
        );
        assert_eq!(
            reopened.agent_definition(operator.id).await.unwrap(),
            Some(operator)
        );
        assert_eq!(
            reopened.agent_definition(protocol_only.id).await.unwrap(),
            Some(protocol_only)
        );
        assert_eq!(
            reopened
                .agent_definition(fixture.unchanged.id)
                .await
                .unwrap(),
            Some(fixture.unchanged)
        );
    }
}

fn snapshot(db: &Connection) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    for table in [
        "host_agent_tool_selections",
        "host_agent_creations",
        "host_agent_renames",
        "host_agent_tool_selection_operations",
    ] {
        let (identity, value) = if table == "host_agent_tool_selections" {
            (
                "agent_id",
                "json_object('revision',revision,'tools',json(tools_json))",
            )
        } else {
            ("operation_id", "result_json")
        };
        rows.extend(
            db.prepare(&format!(
                "SELECT {identity}, {value} FROM {table} ORDER BY {identity}"
            ))
            .unwrap()
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap(),
        );
    }
    rows
}

#[tokio::test]
async fn invalid_or_overflowing_selection_migrations_roll_back_every_row_and_receipt() {
    for table in [
        "host_agent_tool_selections",
        "host_agent_creations",
        "host_agent_renames",
        "host_agent_tool_selection_operations",
    ] {
        for overflow in [false, true] {
            let (_directory, host) = fixture();
            let fixture = seed(&host).await;
            let db = Connection::open(&host.config.database).unwrap();
            install_legacy_selections(&db, &fixture, 31);
            let mut tools = legacy_tools(&fixture.alpha, 31);
            if !overflow {
                tools.insert("unrecognized_capability".to_owned());
            }
            if table == "host_agent_tool_selections" {
                db.execute("UPDATE host_agent_tool_selections SET tools_json=?1,revision=?2 WHERE agent_id=?3",
                    params![serde_json::to_string(&tools).unwrap(), if overflow { i64::MAX } else { 2 }, fixture.alpha.id.to_string()]).unwrap();
            } else {
                let (operation, encoded): (String, String) = db
                    .query_row(
                        &format!("SELECT operation_id,result_json FROM {table} LIMIT 1"),
                        [],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .unwrap();
                let mut result: Value = serde_json::from_str(&encoded).unwrap();
                let selection = result.pointer_mut(receipt_selection(table)).unwrap();
                selection["tools"] = serde_json::to_value(&tools).unwrap();
                if overflow {
                    selection["revision"] = Value::from(i64::MAX);
                }
                db.execute(
                    &format!("UPDATE {table} SET result_json=?1 WHERE operation_id=?2"),
                    params![result.to_string(), operation],
                )
                .unwrap();
            }
            let before = snapshot(&db);
            let error = crate::host::catalog::initialize(&host.config.database)
                .expect_err("reject before the migration commits");
            assert!(
                error.to_string().contains(if overflow {
                    "revision"
                } else {
                    "unrecognized_capability"
                }),
                "{error}"
            );
            assert_eq!(snapshot(&db), before);
            assert_eq!(
                db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
                    .unwrap(),
                31
            );
            assert_eq!(
                db.query_row(
                    "SELECT schema_version FROM host_metadata WHERE singleton=1",
                    [],
                    |r| r.get::<_, u32>(0)
                )
                .unwrap(),
                31
            );
        }
    }
}
