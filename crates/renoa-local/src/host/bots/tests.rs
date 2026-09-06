use std::{fs, path::Path, sync::Arc};

use renoa_agent::{AgentEvent, AgentEventSink, BoxFuture, ContentBlock};
use tempfile::tempdir;
use uuid::Uuid;

use super::*;
use crate::host::HostInitialization;
use crate::{ARCEE_PROFILE_ID, LocalTurnOutcome, ModelProvider};

struct Quiet;
impl AgentEventSink for Quiet {
    fn emit(&self, _: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

#[tokio::test]
async fn a_model_creates_a_durable_bot_that_another_live_host_can_execute_after_restart() {
    let directory = tempdir().expect("fixture");
    let root = directory.path();
    fs::create_dir(root.join("workspace")).expect("workspace");
    fs::write(root.join("model.mjs"), include_str!("test_model.mjs")).expect("model");
    fs::write(root.join("auth.sqlite"), "").expect("auth boundary");
    let host = make_host(root);
    let other = make_host(root);
    let session = host
        .create_session(
            &AgentProfileId::new(ARCEE_PROFILE_ID).expect("id"),
            &root.join("workspace"),
        )
        .await
        .expect("parent");
    let request = Uuid::new_v4();
    let result = session
        .execute_turn(request, vec![ContentBlock::text("create")], Arc::new(Quiet))
        .await
        .expect("create through model");
    assert!(matches!(result, LocalTurnOutcome::Completed { .. }));
    let bots = other.list_bots(None).await.expect("shared inventory");
    assert_eq!(bots.bots.len(), 1);
    let bot = other
        .bot(bots.bots[0].id)
        .await
        .expect("lookup")
        .expect("bot");
    assert!(
        other
            .profile_ids()
            .await
            .expect("profiles")
            .contains(&profile_id(bot.id).expect("profile"))
    );
    assert_eq!(bot.created_by, session.agent_id());
    assert_eq!(host.ensure_bot(bot.clone()).await.expect("idempotent"), bot);
    let mut conflict = bot.clone();
    conflict.recipe.name = "Changed".to_owned();
    assert!(matches!(
        host.ensure_bot(conflict).await,
        Err(LocalHostError::AgentConflict(_))
    ));
    let mut invalid = bot.clone();
    invalid.id = AgentId::new();
    invalid.recipe.connections.insert("missing".to_owned());
    assert!(host.ensure_bot(invalid.clone()).await.is_err());
    assert!(
        host.agent(invalid.id)
            .await
            .expect("rollback lookup")
            .is_none()
    );
    let id = Uuid::new_v4();
    let child = other
        .ensure_agent_session(bot.id, &root.join("workspace"), id)
        .await
        .expect("hot loaded specialist");
    let outcome = child
        .execute_turn(
            Uuid::new_v4(),
            vec![ContentBlock::text("verify")],
            Arc::new(Quiet),
        )
        .await
        .expect("run specialist");
    assert!(
        matches!(outcome,LocalTurnOutcome::Completed {output,..} if output=="specialist ready")
    );
    drop(child);
    drop(other);
    let restarted = make_host(root);
    let child = restarted
        .ensure_agent_session(bot.id, &root.join("workspace"), id)
        .await
        .expect("restore specialist");
    let outcome = child
        .execute_turn(
            Uuid::new_v4(),
            vec![ContentBlock::text("verify")],
            Arc::new(Quiet),
        )
        .await
        .expect("run restored specialist");
    assert!(
        matches!(outcome,LocalTurnOutcome::Completed {output,..} if output=="specialist ready")
    );
    assert_eq!(
        restarted.list_bots(None).await.expect("persistent roster"),
        bots
    );
}

fn make_host(root: &Path) -> LocalHost {
    LocalHost::assemble(HostInitialization {
        data_directory: root.join("data"),
        bridge: root.join("model.mjs"),
        providers: vec![ModelProvider::Xai],
        initial_provider: ModelProvider::Xai,
        initial_model: "fixture".to_owned(),
        initial_reasoning: None,
        credential_store: root.join("auth.sqlite"),
        mcp_adapter: None,
        mcp_registry_adapter: None,
        shared_plugin_registry: None,
        global_skill_source: None,
        oauth_relay: None,
        profiles: vec![
            AgentProfile::new(ARCEE_PROFILE_ID, "Create specialists.").expect("profile"),
        ],
    })
    .expect("Host")
}
