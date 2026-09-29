//! A finished operation keeps no copy of its effects' requests.
//!
//! A request is read only while its operation may still dispatch, replay, or
//! decide from it. Once the operation is completed, failed, or cancelled, the
//! request of each settled effect is released in the same transaction that
//! finished it; its binding, status, and outcome stay. A batch with an
//! unsettled child keeps every request, so an unknown outcome can still be
//! examined and abandoned. Schema 4 makes the column nullable and releases the
//! requests of operations that finished earlier.

use rusqlite::{Connection, Transaction, TransactionBehavior};
use serde_json::json;

use crate::{KernelError, schema::sqlite_error};

/// The requests that may go: settled effects of a finished operation whose
/// batch has no unsettled child. The one definition the trigger and the
/// upgrade share.
const FINISHED_REQUESTS: &str = "CREATE VIEW finished_effect_requests AS
        SELECT e.effect_id, o.operation_id, length(CAST(e.request_json AS BLOB)) AS bytes
        FROM effects AS e
        JOIN effect_batches AS b ON b.batch_id = e.batch_id
        JOIN operations AS o ON o.operation_id = b.operation_id
        WHERE o.phase IN ('completed', 'failed', 'cancelled')
          AND e.status = 'settled' AND e.request_json IS NOT NULL
          AND NOT EXISTS (
              SELECT 1 FROM effects AS sibling
              WHERE sibling.batch_id = e.batch_id AND sibling.status != 'settled'
          );

     CREATE TRIGGER release_finished_effect_requests
        AFTER UPDATE OF phase ON operations
     BEGIN
        UPDATE effects SET request_json = NULL
        WHERE effect_id IN (
            SELECT effect_id FROM finished_effect_requests
            WHERE operation_id = NEW.operation_id
        );
     END;";

/// Installs the release of finished requests in a new or upgraded schema.
pub(crate) fn install(transaction: &Transaction<'_>) -> Result<(), KernelError> {
    transaction
        .execute_batch(FINISHED_REQUESTS)
        .map_err(sqlite_error)
}

const MIGRATE_V3_TO_V4: &str = "ALTER TABLE effects RENAME TO effects_v3;

     CREATE TABLE effects (
        effect_id TEXT PRIMARY KEY NOT NULL,
        batch_id TEXT NOT NULL REFERENCES effect_batches(batch_id),
        position INTEGER NOT NULL CHECK (position >= 0),
        binding TEXT NOT NULL CHECK (length(binding) > 0),
        binding_revision TEXT NOT NULL CHECK (length(binding_revision) > 0),
        recovery TEXT NOT NULL CHECK (
            recovery IN ('safe_to_replay', 'never_replay')
        ),
        request_json TEXT CHECK (request_json IS NOT NULL OR status = 'settled'),
        status TEXT NOT NULL CHECK (
            status IN (
                'intent_committed', 'dispatch_started',
                'settled', 'outcome_unknown'
            )
        ),
        dispatch_count INTEGER NOT NULL CHECK (dispatch_count >= 0),
        outcome_json TEXT,
        UNIQUE (batch_id, position),
        CHECK ((status = 'settled') = (outcome_json IS NOT NULL))
     ) STRICT;

     INSERT INTO effects (
        effect_id, batch_id, position, binding, binding_revision, recovery,
        request_json, status, dispatch_count, outcome_json
     ) SELECT
        effect_id, batch_id, position, binding, binding_revision, recovery,
        request_json, status, dispatch_count, outcome_json
     FROM effects_v3;

     DROP TABLE effects_v3;";

/// Upgrades schema 3 to 4, releases the requests of operations that already
/// finished, and reclaims their space.
pub(crate) fn migrate_v3_to_v4(connection: &mut Connection) -> Result<(), KernelError> {
    connection
        .pragma_update(None, "foreign_keys", false)
        .map_err(sqlite_error)?;
    let migration = (|| {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        transaction
            .execute_batch(MIGRATE_V3_TO_V4)
            .map_err(sqlite_error)?;
        install(&transaction)?;
        let (effects, bytes): (i64, i64) = transaction
            .query_row(
                "SELECT count(*), coalesce(sum(bytes), 0) FROM finished_effect_requests",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(sqlite_error)?;
        transaction
            .execute(
                "UPDATE effects SET request_json = NULL
                 WHERE effect_id IN (SELECT effect_id FROM finished_effect_requests)",
                [],
            )
            .map_err(sqlite_error)?;
        let sessions = transaction
            .prepare("SELECT session_id FROM sessions ORDER BY session_id")
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(sqlite_error)?;
        transaction
            .pragma_update(None, "user_version", 4_u32)
            .map_err(sqlite_error)?;
        transaction.commit().map_err(sqlite_error)?;
        Ok((
            effects,
            json!({ "sessions": sessions, "effects": effects, "bytes": bytes }),
        ))
    })();
    let foreign_keys = connection
        .pragma_update(None, "foreign_keys", true)
        .map_err(sqlite_error);
    let (effects, released) = migration?;
    foreign_keys?;
    if effects > 0 {
        reclaim(connection, released);
    }
    Ok(())
}

/// Returns the released space to the file system. The release is already
/// committed, so a failure here costs only space and is reported, not raised.
fn reclaim(connection: &Connection, mut released: serde_json::Value) {
    let vacuumed = connection.execute_batch("VACUUM;");
    let level = if vacuumed.is_ok() { "info" } else { "warn" };
    if let Err(error) = vacuumed {
        released["vacuum_error"] = json!(error.to_string());
    }
    renoa_telemetry::event("renoa.kernel", level, "kernel_requests_released", &released);
}
