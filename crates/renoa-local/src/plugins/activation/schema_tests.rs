use crate::{host::catalog, plugins::manager::integration_id};
use rusqlite::{Connection, params};
use std::path::Path;

const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const AGENT: &str = "00000000-0000-0000-0000-000000000001";

fn schema_30(path: &Path, selected: bool) {
    catalog::initialize(path).unwrap();
    let connection = Connection::open(path).unwrap();
    connection.execute_batch("DROP VIEW host_agent_enabled_mcp_connections;
        DROP TABLE host_plugin_admissions;
        DROP TABLE host_plugin_activation_operations;
        DROP TABLE host_agent_plugin_revisions;
        DROP TABLE host_agent_plugins;
        DROP TABLE host_plugin_provider_origins;
        DROP TABLE host_plugin_provider_families;
        DROP TABLE plugin_mcp_servers;
        CREATE TABLE plugin_mcp_servers (
            plugin_digest TEXT NOT NULL REFERENCES installed_plugins(plugin_digest) ON DELETE RESTRICT,
            server_id TEXT NOT NULL CHECK (length(server_id) BETWEEN 1 AND 128),
            transport TEXT NOT NULL CHECK (transport='streamable_http'),
            endpoint TEXT NOT NULL CHECK (length(endpoint)>0),
            request_headers_json TEXT NOT NULL CHECK(json_valid(request_headers_json) AND json_type(request_headers_json)='object'),
            PRIMARY KEY(plugin_digest,server_id)
        ) STRICT;
        UPDATE host_metadata SET schema_version=30 WHERE singleton=1;
        PRAGMA user_version=30;").unwrap();
    connection
        .execute(
            "INSERT INTO installed_plugins(plugin_digest,name) VALUES (?1,'fixture')",
            [DIGEST],
        )
        .unwrap();
    connection.execute("INSERT INTO plugin_mcp_servers VALUES (?1,'main','streamable_http','https://service.example/mcp','{}')",[DIGEST]).unwrap();
    if selected {
        crate::test_agents::insert_agent(path, AGENT);
        let integration = integration_id(DIGEST, "main");
        connection.execute("INSERT INTO mcp_integrations VALUES (?1,'direct_streamable_http','https://service.example/mcp','{}')",[integration.clone()]).unwrap();
        connection.execute("INSERT INTO mcp_connections(connection_id,integration_id,auth_kind) VALUES ('account',?1,'none')",[integration]).unwrap();
        connection.execute("INSERT INTO host_agent_mcp_connections(agent_id,connection_id) VALUES (?1,'account')",[AGENT]).unwrap();
    }
}

#[test]
fn unselected_schema_30_installs_gain_verified_ownership_without_activation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("host.sqlite3");
    schema_30(&path, false);
    catalog::initialize(&path).expect("migrate actual old table without integration_id");
    let connection = catalog::open_verified(&path).unwrap();
    let owner: String = connection
        .query_row("SELECT integration_id FROM plugin_mcp_servers", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(owner, integration_id(DIGEST, "main"));
    let activations: u32 = connection
        .query_row("SELECT count(*) FROM host_agent_plugins", [], |r| r.get(0))
        .unwrap();
    assert_eq!(activations, 0);
    catalog::initialize(&path).expect("convergent restart");
}

#[test]
fn schema_30_selections_require_explicit_reset_and_failed_migration_rolls_back() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("host.sqlite3");
    schema_30(&path, true);
    for _ in 0..2 {
        let error =
            catalog::initialize(&path).expect_err("must not silently drop or infer selections");
        assert!(error.to_string().contains("explicit Host reset"));
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
                .unwrap(),
            30
        );
        assert_eq!(connection.query_row("SELECT count(*) FROM pragma_table_info('plugin_mcp_servers') WHERE name='integration_id'",[],|r|r.get::<_,u32>(0)).unwrap(),0);
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM host_agent_mcp_connections WHERE agent_id=?1",
                    params![AGENT],
                    |r| r.get::<_, u32>(0)
                )
                .unwrap(),
            1
        );
    }
    catalog::cutover(&path).expect("operator's explicit clean break");
    catalog::initialize(&path).unwrap();
    let connection = catalog::open_verified(&path).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM host_agent_plugins", [], |r| r
                .get::<_, u32>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM mcp_connections", [], |r| r
                .get::<_, u32>(0))
            .unwrap(),
        1
    );
}
