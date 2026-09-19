//! Test-only Host fixtures shared by store-level tests across the crate.

use std::path::Path;

use rusqlite::Connection;

/// Creates one canonical agent row, with the tool-selection row the definition
/// store reads back.
///
/// Store-level fixtures work below `LocalHost`, so they cannot create an agent
/// through the canonical operation; this writes the row and the child row that
/// operation writes, in the same shape, so the binding table's foreign key holds
/// and a later canonical read of the fixture agent succeeds.
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
                creator_kind, creator_component
             ) VALUES (?1, 'Fixture', 0, 'provisioning', ?3, ?2, 'system', 'fixture')",
            rusqlite::params![
                agent,
                serde_json::to_string(&operational).expect("operational document"),
                crate::presets::SPECIALIST_PRESET_ID,
            ],
        )
        .expect("agent row");
    connection
        .execute(
            "INSERT INTO host_agent_tool_selections(agent_id, revision, tools_json)
             VALUES (?1, 1, '[]')",
            rusqlite::params![agent],
        )
        .expect("tool selection row");
}
