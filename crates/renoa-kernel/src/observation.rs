//! Non-owning projections of committed execution state, without payload hydration.

use std::{path::Path, time::Duration};

use rusqlite::{Connection, OpenFlags, OptionalExtension};

use crate::{
    AgentId, CommandId, KernelError, OperationId, OperationStatus, SessionId,
    admission::{from_sql_integer, parse_agent_id, parse_command_id, parse_operation_id},
    operation_phase::OperationPhase,
    schema::{OPERATION_STATE_VERSION, SCHEMA_VERSION, sqlite_error},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationObservation {
    pub operation_id: OperationId,
    pub command_id: CommandId,
    pub position: u64,
    pub status: OperationStatus,
}

/// Committed state, not a process heartbeat. An unfinished operation can outlive
/// its worker; `Running` alone does not prove a process is currently making progress.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionObservation {
    pub agent_id: AgentId,
    pub event_count: u64,
    pub queued_operations: u64,
    pub active_operation: Option<OperationObservation>,
    pub latest_operation: Option<OperationObservation>,
}

/// Reads one existing session without acquiring execution ownership or migrating
/// storage. One `SQLite` read transaction covers the entire projection. No command,
/// checkpoint, effect payload, or transcript is loaded.
///
/// # Errors
/// Returns missing storage/session, incompatible schema, corruption or read errors.
pub fn observe_session(
    path: &Path,
    session_id: SessionId,
) -> Result<SessionObservation, KernelError> {
    let mut db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(sqlite_error)?;
    db.busy_timeout(Duration::from_secs(5))
        .map_err(sqlite_error)?;
    let tx = db.transaction().map_err(sqlite_error)?;
    let version: u32 = tx
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(sqlite_error)?;
    if version != SCHEMA_VERSION {
        return Err(KernelError::UnsupportedSchema {
            found: version,
            supported: SCHEMA_VERSION,
        });
    }
    let (agent, active, event_count): (String, Option<String>, i64) = tx.query_row(
        "SELECT agent_id, active_operation_id, next_event_sequence FROM sessions WHERE session_id=?1",
        [session_id.to_string()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).optional().map_err(sqlite_error)?.ok_or(KernelError::SessionNotFound(session_id))?;
    let queued: i64 = tx
        .query_row(
            "SELECT count(*) FROM operations WHERE session_id=?1 AND phase='queued'",
            [session_id.to_string()],
            |r| r.get(0),
        )
        .map_err(sqlite_error)?;
    let active_operation = active
        .as_deref()
        .map(|id| {
            operation(&tx, session_id, Some(id))?.ok_or_else(|| {
                KernelError::Corrupt("active operation does not belong to its session".to_owned())
            })
        })
        .transpose()?;
    let latest_operation = operation(&tx, session_id, None)?;
    let snapshot = SessionObservation {
        agent_id: parse_agent_id(&agent)?,
        event_count: from_sql_integer(event_count, "event count")?,
        queued_operations: from_sql_integer(queued, "queued operation count")?,
        active_operation,
        latest_operation,
    };
    tx.commit().map_err(sqlite_error)?;
    Ok(snapshot)
}

fn operation(
    db: &Connection,
    session: SessionId,
    id: Option<&str>,
) -> Result<Option<OperationObservation>, KernelError> {
    let mut query = db
        .prepare(
            "SELECT operation_id, command_id, position, phase, state_version FROM operations
         WHERE session_id=?1 AND (?2 IS NULL OR operation_id=?2) ORDER BY position DESC LIMIT 1",
        )
        .map_err(sqlite_error)?;
    let mut rows = query
        .query(rusqlite::params![session.to_string(), id])
        .map_err(sqlite_error)?;
    let Some(row) = rows.next().map_err(sqlite_error)? else {
        return Ok(None);
    };
    let version: u32 = row.get(4).map_err(sqlite_error)?;
    if version != OPERATION_STATE_VERSION {
        return Err(KernelError::UnsupportedStateVersion {
            found: version,
            supported: OPERATION_STATE_VERSION,
        });
    }
    Ok(Some(OperationObservation {
        operation_id: parse_operation_id(&row.get::<_, String>(0).map_err(sqlite_error)?)?,
        command_id: parse_command_id(&row.get::<_, String>(1).map_err(sqlite_error)?)?,
        position: from_sql_integer(row.get(2).map_err(sqlite_error)?, "operation position")?,
        status: OperationPhase::from_database(&row.get::<_, String>(3).map_err(sqlite_error)?)?
            .status(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentId, Command, Kernel};

    struct ObservingLoop {
        path: std::path::PathBuf,
        session: SessionId,
        fail: bool,
    }

    impl crate::LoopPlugin for ObservingLoop {
        fn decide(&self, _: crate::LoopInput) -> Result<crate::LoopDecision, crate::LoopError> {
            let view =
                observe_session(&self.path, self.session).expect("observe inside executing loop");
            assert_eq!(
                view.active_operation.expect("active operation").status,
                OperationStatus::Running
            );
            if self.fail {
                return Err(crate::LoopError::new("injected interruption"));
            }
            Ok(crate::LoopDecision::Complete {
                checkpoint: crate::Checkpoint::new(1, serde_json::json!({})),
                events: vec![crate::NewEvent::new("done", serde_json::json!({}))],
            })
        }
    }

    #[tokio::test]
    async fn observes_execution_interruption_and_completion_without_claiming_liveness() {
        let dir = tempfile::tempdir().expect("directory");
        let path = dir.path().join("kernel.sqlite3");
        let kernel = Kernel::open(&path).expect("owner");
        let agent = AgentId::new();
        let session = SessionId::new();
        kernel.create_agent(agent).expect("agent");
        kernel.create_session(session, agent).expect("session");
        kernel
            .submit(
                session,
                Command::new(CommandId::new(), serde_json::json!({})),
            )
            .expect("admit");
        let runtime = |fail| {
            crate::Runtime::new(
                crate::LoopBinding::new(
                    "observe",
                    "1",
                    std::sync::Arc::new(ObservingLoop {
                        path: path.clone(),
                        session,
                        fail,
                    }),
                ),
                1,
                "same-config",
                Vec::new(),
            )
            .expect("runtime")
        };
        assert!(kernel.drive(session, &runtime(true)).await.is_err());
        drop(kernel);
        let interrupted = observe_session(&path, session).expect("read with no worker");
        assert_eq!(
            interrupted.active_operation.expect("unfinished").status,
            OperationStatus::Running
        );
        let kernel = Kernel::open(&path).expect("resume owner");
        kernel
            .drive(session, &runtime(false))
            .await
            .expect("complete");
        let finished = observe_session(&path, session).expect("read completed");
        assert!(finished.active_operation.is_none());
        assert_eq!(
            finished.latest_operation.expect("completed").status,
            OperationStatus::Completed
        );
        assert_eq!(finished.event_count, 1);
    }

    #[test]
    fn observation_coexists_with_owner_and_sees_later_commits_without_reading_payloads() {
        let dir = tempfile::tempdir().expect("directory");
        let path = dir.path().join("kernel.sqlite3");
        let kernel = Kernel::open(&path).expect("owner");
        let agent = AgentId::new();
        let session = SessionId::new();
        kernel.create_agent(agent).expect("agent");
        kernel.create_session(session, agent).expect("session");
        assert_eq!(
            observe_session(&path, session)
                .expect("empty")
                .latest_operation,
            None
        );
        let admission = kernel
            .submit(
                session,
                Command::new(
                    CommandId::new(),
                    serde_json::json!({"secret": "not part of inventory"}),
                ),
            )
            .expect("admit");
        let view = observe_session(&path, session).expect("observe while owned");
        assert_eq!(view.agent_id, agent);
        assert_eq!(view.queued_operations, 1);
        assert_eq!(
            view.latest_operation.as_ref().expect("latest").operation_id,
            admission.operation_id
        );
        assert!(!format!("{view:?}").contains("secret"));
        assert!(matches!(
            Kernel::open(&path),
            Err(KernelError::AlreadyRunning { .. })
        ));
        drop(kernel);
        assert_eq!(
            observe_session(&path, session).expect("owner stopped"),
            view
        );
        let owner = Kernel::open(&path).expect("observer retains no lock");
        owner
            .submit(
                session,
                Command::new(CommandId::new(), serde_json::json!({})),
            )
            .expect("second admission");
        assert_eq!(
            observe_session(&path, session)
                .expect("new snapshot")
                .queued_operations,
            2
        );
    }

    #[test]
    fn observation_never_creates_or_migrates_storage() {
        let dir = tempfile::tempdir().expect("directory");
        let path = dir.path().join("missing.sqlite3");
        let session = SessionId::new();
        assert!(observe_session(&path, session).is_err());
        assert!(!path.exists());
        drop(Kernel::open(&path).expect("initialize"));
        let db = Connection::open(&path).expect("fixture");
        db.pragma_update(None, "user_version", 1)
            .expect("old schema");
        assert!(matches!(
            observe_session(&path, session),
            Err(KernelError::UnsupportedSchema { found: 1, .. })
        ));
        assert_eq!(
            db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .expect("version"),
            1
        );
    }
}
