//! Test-only Host fixtures shared by store-level tests across the crate.

use std::path::Path;

use rusqlite::Connection;

/// Creates the canonical agent row one connection binding references.
///
/// Store-level fixtures work below `LocalHost`, so they cannot create an agent
/// through the canonical operation; this writes the same row that operation
/// would, so the binding table's foreign key holds in those fixtures.
pub(crate) fn insert_agent(path: &Path, agent: &str) {
    let operational = crate::AgentOperationalDefinition {
        instructions: "Fixture.".to_owned(),
        behavior: crate::AgentBehavior {
            turn_timing: crate::TurnTiming::Off,
            workspace_instructions: crate::WorkspaceInstructions::Off,
            automatic_compaction: None,
        },
        documents: None,
        provider_restriction: None,
    };
    let connection = Connection::open(path).expect("open Host catalog");
    connection
        .execute(
            "INSERT INTO host_agents(
                agent_id, name, created_at_ms, created_via, preset_id, operational_json,
                creator_kind, creator_host_id, creator_principal_id
             ) VALUES (?1, 'Fixture', 0, 'provisioning', NULL, ?2, 'principal', ?3, 'fixture')",
            rusqlite::params![
                agent,
                serde_json::to_string(&operational).expect("operational document"),
                uuid::Uuid::new_v4().to_string(),
            ],
        )
        .expect("agent row");
}
