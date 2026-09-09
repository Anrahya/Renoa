use super::*;
use crate::{
    AgentProfileId, AgentRecord, BotRecipe,
    host::bots::{profile_id, store},
};

#[tokio::test]
async fn edits_survive_restart_preserve_creation_replay_and_reject_stale_writers() {
    let directory = tempfile::tempdir().expect("Host");
    super::super::tests::prepare_fixture(directory.path());
    let host = super::super::tests::make_host(directory.path());
    let creator = AgentId::new();
    host.ensure_agent(AgentRecord {
        id: creator,
        profile: AgentProfileId::new(crate::ARCEE_PROFILE_ID).expect("profile"),
        name: "Operator".to_owned(),
        created_by: None,
    })
    .await
    .expect("operator");
    let bot = BotRecord {
        id: AgentId::new(),
        created_by: creator,
        recipe: BotRecipe {
            name: "Inspector".to_owned(),
            instructions: "Inspect a workspace".to_owned(),
            tools: BTreeSet::new(),
            connections: BTreeSet::new(),
        },
    };
    host.ensure_bot(bot.clone()).await.expect("bot");
    // Exercise upgrade from the prior catalog while preserving the old recipe.
    let db = catalog::open_verified(&host.config.database).expect("database");
    db.execute_batch(
        "DROP TABLE host_bot_tool_operations; DROP TABLE host_bot_tool_selections;
        UPDATE host_metadata SET schema_version=24; PRAGMA user_version=24;",
    )
    .expect("old catalog");
    drop(db);
    drop(host);
    let host = super::super::tests::make_host(directory.path());
    assert_eq!(
        host.bot_tool_selection(bot.id)
            .await
            .expect("default")
            .revision,
        0
    );
    let edit = BotToolsUpdate {
        operation_id: Uuid::new_v4(),
        id: bot.id,
        expected_revision: 0,
        tools: ["git_changes", "git_diff", "git_show"]
            .map(str::to_owned)
            .into(),
    };
    let first = host.configure_bot_tools(edit.clone()).await.expect("edit");
    assert_eq!(first.revision, 1);
    assert_eq!(
        host.ensure_bot(bot.clone()).await.expect("creation replay"),
        bot
    );
    let profile = store::profile(&host.config.database, &profile_id(bot.id).expect("profile"))
        .expect("runtime profile");
    assert_eq!(profile.selected_tools.as_ref(), Some(&edit.tools));
    let workspace =
        crate::LocalWorkspace::open(directory.path().join("workspace")).expect("workspace");
    assert_eq!(
        workspace
            .selected_kernel_tool_bindings(profile.selected_tools.as_ref())
            .len(),
        3
    );
    let mut stale = edit.clone();
    stale.operation_id = Uuid::new_v4();
    assert!(host.configure_bot_tools(stale).await.is_err());
    let second = BotToolsUpdate {
        operation_id: Uuid::new_v4(),
        expected_revision: 1,
        tools: ["read_file".to_owned()].into(),
        ..edit.clone()
    };
    host.configure_bot_tools(second).await.expect("later edit");
    drop(host);
    let host = super::super::tests::make_host(directory.path());
    assert_eq!(
        host.configure_bot_tools(edit.clone())
            .await
            .expect("old retry"),
        first
    );
    assert_eq!(
        host.bot_tool_selection(bot.id)
            .await
            .expect("current")
            .revision,
        2
    );
    let mut conflict = edit;
    conflict.tools.insert("bash".to_owned());
    assert!(host.configure_bot_tools(conflict).await.is_err());
    assert_eq!(host.bot(bot.id).await.expect("lookup"), Some(bot));
}
