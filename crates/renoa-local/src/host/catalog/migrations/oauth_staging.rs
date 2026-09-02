pub(crate) const MIGRATE_V12_TO_V13: &str = "
    CREATE TABLE mcp_oauth_flows_v13 (
        connection_id TEXT PRIMARY KEY CHECK (length(connection_id) > 0),
        operation_id TEXT NOT NULL CHECK (length(operation_id) BETWEEN 1 AND 512),
        phase TEXT NOT NULL CHECK (
            phase IN (
                'begin_in_flight', 'awaiting_callback', 'callback_ready',
                'exchange_in_flight', 'refresh_in_flight', 'unknown'
            )
        ),
        callback_port INTEGER,
        callback_relay_id TEXT,
        expires_at_ms INTEGER,
        CHECK (
            (phase IN ('begin_in_flight', 'awaiting_callback', 'callback_ready',
                       'exchange_in_flight')
             AND ((callback_port BETWEEN 1 AND 65535 AND callback_relay_id IS NULL)
                  OR (callback_port IS NULL AND length(callback_relay_id) = 36))
             AND expires_at_ms > 0)
            OR
            (phase IN ('refresh_in_flight', 'unknown')
             AND callback_port IS NULL
             AND callback_relay_id IS NULL
             AND expires_at_ms IS NULL)
        )
    ) STRICT;

    INSERT INTO mcp_oauth_flows_v13(
        connection_id, operation_id, phase, callback_port, callback_relay_id, expires_at_ms
    )
    SELECT connection_id, operation_id, phase, callback_port, callback_relay_id, expires_at_ms
    FROM mcp_oauth_flows;

    CREATE TABLE mcp_oauth_receipts_v13 (
        connection_id TEXT NOT NULL CHECK (length(connection_id) > 0),
        operation_id TEXT NOT NULL CHECK (length(operation_id) BETWEEN 1 AND 512),
        outcome_json TEXT NOT NULL CHECK (
            length(outcome_json) BETWEEN 1 AND 16384
            AND json_valid(outcome_json)
            AND json_type(outcome_json) = 'object'
        ),
        PRIMARY KEY (connection_id, operation_id)
    ) STRICT;

    INSERT INTO mcp_oauth_receipts_v13(connection_id, operation_id, outcome_json)
    SELECT connection_id, operation_id, outcome_json FROM mcp_oauth_receipts;

    DROP TABLE mcp_oauth_receipts;
    ALTER TABLE mcp_oauth_receipts_v13 RENAME TO mcp_oauth_receipts;
    DROP TABLE mcp_oauth_flows;
    ALTER TABLE mcp_oauth_flows_v13 RENAME TO mcp_oauth_flows;

    UPDATE host_metadata SET schema_version = 13 WHERE singleton = 1;
";
