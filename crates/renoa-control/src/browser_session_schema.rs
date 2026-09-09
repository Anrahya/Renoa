use rusqlite::{Connection, TransactionBehavior};

use crate::{ControlError, store::sqlite_error};

pub(crate) fn initialize(connection: &mut Connection) -> Result<(), ControlError> {
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sqlite_error)?;
    let legacy: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('browser_sessions'))
             AND NOT EXISTS(SELECT 1 FROM pragma_table_info('browser_sessions')
                            WHERE name='principal_id')",
            [],
            |row| row.get(0),
        )
        .map_err(sqlite_error)?;
    if legacy {
        tx.execute_batch("ALTER TABLE browser_sessions RENAME TO browser_sessions_legacy;")
            .map_err(sqlite_error)?;
    }
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS browser_sessions (
            token_hash BLOB PRIMARY KEY CHECK(length(token_hash) = 32),
            principal_id TEXT NOT NULL,
            credential_id BLOB REFERENCES passkeys(credential_id) ON DELETE CASCADE,
            expires_at_ms INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS browser_pairings (
            token_hash BLOB PRIMARY KEY CHECK(length(token_hash) = 32),
            principal_id TEXT NOT NULL,
            expires_at_ms INTEGER NOT NULL,
            claimed_session_hash BLOB CHECK(claimed_session_hash IS NULL OR length(claimed_session_hash)=32)
        );",
    )
    .map_err(sqlite_error)?;
    if legacy {
        // A missing passkey must fail the NOT NULL constraint, never silently drop a login.
        tx.execute_batch(
            "INSERT INTO browser_sessions(token_hash,principal_id,credential_id,expires_at_ms)
             SELECT s.token_hash,p.principal_id,s.credential_id,s.expires_at_ms
             FROM browser_sessions_legacy s LEFT JOIN passkeys p ON p.credential_id=s.credential_id;
             DROP TABLE browser_sessions_legacy;",
        )
        .map_err(sqlite_error)?;
    }
    tx.commit().map_err(sqlite_error)
}
