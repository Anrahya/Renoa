use super::*;
use renoa_local::{BotRecipe, BotRecord};

#[tokio::test]
async fn switching_agents_preserves_admitted_targets_and_isolates_bot_history_and_workspace() {
    let mut fixture = Fixture::new().await;
    let bot = news_bot(&fixture).await;
    fixture.admit("Ev1", "1.000001", "queued for Arcee").await;
    let select = format!("!agent {}", bot.id);
    fixture.admit("Ev2", "2.000001", &select).await;
    fixture.admit("Ev2", "2.000001", &select).await;
    fixture.admit("Ev3", "3.000001", "queued for News").await;
    fixture.admit("Ev4", "4.000001", "!new").await;
    fixture.admit("Ev5", "5.000001", "also for News").await;
    fixture.admit("Ev6", "6.000001", "!agent arcee").await;
    fixture.admit("Ev7", "7.000001", "back to Arcee").await;
    let expected = [
        fixture.worker.agent_id,
        bot.id,
        bot.id,
        bot.id,
        bot.id,
        fixture.worker.agent_id,
        fixture.worker.agent_id,
    ];
    let mut sessions = Vec::new();
    for id in expected {
        let work = fixture
            .worker
            .store
            .next_work()
            .await
            .expect("queue")
            .expect("work");
        assert_eq!(
            AgentId::from_uuid(
                fixture
                    .worker
                    .store
                    .session_agent(work.session_id)
                    .await
                    .expect("target")
            ),
            id
        );
        sessions.push(work.session_id);
        // Drop the live session before each command to prove durable routing.
        fixture.worker.session = None;
        fixture.worker.execute(work).await.expect("execute");
        if let Some(session) = &fixture.worker.session {
            assert_eq!(session.agent_id(), id);
        }
    }
    assert_ne!(sessions[0], sessions[1]);
    assert_eq!(sessions[1], sessions[2]);
    assert_ne!(sessions[2], sessions[3]);
    assert_eq!(sessions[3], sessions[4]);
    assert_ne!(sessions[4], sessions[5]);
    assert!(
        fixture
            .worker
            .store
            .next_work()
            .await
            .expect("queue")
            .is_none()
    );
    let bot_workspace = fixture
        .worker
        .host
        .bot_workspace(bot.id)
        .await
        .expect("workspace");
    assert_ne!(bot_workspace, fixture.worker.workspace);
    let replies = fixture
        .worker
        .store
        .run(|connection| {
            let mut stmt = connection
                .prepare("SELECT result FROM requests WHERE executes_model=1 ORDER BY seq")?;
            Ok(stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?)
        })
        .await
        .expect("results");
    assert_eq!(
        replies,
        [
            "Arcee executed this Slack request.",
            "News executed this Slack request.",
            "News executed this Slack request.",
            "Arcee executed this Slack request."
        ]
    );
    fixture.stop().await;
}

#[tokio::test]
async fn unknown_agent_selection_keeps_the_current_conversation() {
    let fixture = Fixture::new().await;
    let bot = news_bot(&fixture).await;
    fixture
        .admit("Ev1", "1.000001", &format!("!agent {}", bot.id))
        .await;
    fixture
        .admit("Ev2", "2.000001", &format!("!agent {}", Uuid::new_v4()))
        .await;
    let ids = fixture
        .worker
        .store
        .run(|connection| {
            let mut stmt = connection.prepare("SELECT session_id FROM requests ORDER BY seq")?;
            Ok(stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?)
        })
        .await
        .expect("bindings");
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0], ids[1]);
    fixture.stop().await;
}

async fn news_bot(fixture: &Fixture) -> BotRecord {
    fixture
        .worker
        .host
        .ensure_bot(BotRecord {
            id: AgentId::new(),
            created_by: fixture.worker.agent_id,
            recipe: BotRecipe {
                name: "News".to_owned(),
                instructions: "News specialist.".to_owned(),
                tools: ["read_file".to_owned()].into(),
                connections: std::collections::BTreeSet::new(),
            },
        })
        .await
        .expect("create bot")
}
