//! Makes one chosen definition transaction's real COMMIT fail: a deferred
//! foreign key that names no agent is checked only when the transaction commits.
//! Armed by key, a creation's operation id or a moved agent's id, so parallel
//! tests never trip each other.

use std::{collections::BTreeSet, fmt::Display, sync::Mutex};

use uuid::Uuid;

static ARMED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

pub(super) fn arm(key: impl Display) {
    ARMED
        .lock()
        .expect("commit fault lock")
        .insert(key.to_string());
}

pub(super) fn inject(
    transaction: &rusqlite::Transaction<'_>,
    key: impl Display,
) -> rusqlite::Result<()> {
    if !ARMED
        .lock()
        .expect("commit fault lock")
        .remove(&key.to_string())
    {
        return Ok(());
    }
    transaction.execute_batch("PRAGMA defer_foreign_keys = ON")?;
    transaction.execute(
        "INSERT INTO host_agent_creations(operation_id, agent_id, request_json, result_json)
         VALUES (?1, ?2, '{}', '{}')",
        [Uuid::new_v4().to_string(), Uuid::new_v4().to_string()],
    )?;
    Ok(())
}
