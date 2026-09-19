use rusqlite::Transaction;

use super::HostCatalogError;

pub(super) fn initialize(transaction: &Transaction<'_>) -> Result<(), HostCatalogError> {
    transaction.execute_batch(
        "CREATE TABLE host_identity (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            host_id TEXT NOT NULL UNIQUE CHECK (length(host_id) = 36)
        ) STRICT;
        UPDATE host_metadata SET schema_version = 14 WHERE singleton = 1;",
    )?;
    transaction.execute(
        "INSERT INTO host_identity(singleton, host_id) VALUES (1, ?1)",
        [uuid::Uuid::new_v4().to_string()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{sync::Barrier, thread};

    use super::super::{HostCatalogError, cutover, initialize, open_verified};

    #[test]
    fn concurrent_schema_thirteen_upgrade_preserves_catalog_and_publishes_one_host_identity() {
        let directory = tempfile::tempdir().expect("catalog directory");
        let database = directory.path().join("host.sqlite3");
        initialize(&database).expect("current schema");
        {
            let connection = open_verified(&database).expect("catalog");
            connection.execute_batch(
                "INSERT INTO mcp_integrations(integration_id, kind, endpoint, request_headers_json)
                 VALUES ('retained', 'direct_streamable_http', 'https://example.com/mcp', '{}');
                 DROP TABLE host_agents;
                 DROP TABLE host_identity;
                 UPDATE host_metadata SET schema_version = 13;
                 PRAGMA user_version = 13;",
            ).expect("schema thirteen fixture");
        }
        let refused = initialize(&database);
        assert!(
            matches!(&refused, Err(HostCatalogError::Invalid(message)) if message.contains("reset")),
            "an earlier data root must be refused until it is reset: {refused:?}"
        );
        let barrier = Barrier::new(2);
        thread::scope(|scope| {
            let first = scope.spawn(|| {
                barrier.wait();
                cutover(&database).expect("first cutover");
            });
            let second = scope.spawn(|| {
                barrier.wait();
                cutover(&database).expect("second cutover");
            });
            first.join().expect("first initializer");
            second.join().expect("second initializer");
        });
        let connection = open_verified(&database).expect("cut-over catalog");
        let host_id: String = connection
            .query_row("SELECT host_id FROM host_identity", [], |row| row.get(0))
            .expect("identity");
        uuid::Uuid::parse_str(&host_id).expect("valid Host identity");
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM host_identity", [], |row| row
                    .get::<_, u32>(0))
                .expect("one Host identity"),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM mcp_integrations WHERE integration_id = 'retained'",
                    [],
                    |row| row.get::<_, u32>(0)
                )
                .expect("retained integration"),
            1
        );
        drop(connection);
        initialize(&database).expect("ordinary restart");
        let connection = open_verified(&database).expect("catalog after restart");
        assert_eq!(
            connection
                .query_row("SELECT host_id FROM host_identity", [], |row| row
                    .get::<_, String>(0))
                .expect("same identity"),
            host_id
        );
    }
}
