use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use futures_util::FutureExt as _;

use super::{Command, EDIT_INTERVAL, IDLE_LIMIT, Step, resume, show, ticker};
use crate::{
    api::DiscordApi,
    store::{ProgressTarget, SurfaceStore},
};

const COMMAND: &str = "33333333-3333-4333-8333-333333333333";

fn target() -> ProgressTarget {
    ProgressTarget {
        channel_id: "202".to_owned(),
        reply_to: Some("101".to_owned()),
    }
}

/// A command whose steps all arrive at one instant.
struct Timed(Command, Instant);

impl Timed {
    fn apply(&mut self, step: Step) {
        self.0.apply(step, self.1);
    }

    fn render(&self) -> Option<String> {
        self.0.render()
    }
}

fn command() -> Timed {
    let now = Instant::now();
    Timed(Command::new(target(), now), now)
}

fn tool(call_id: &str, name: &str) -> Step {
    Step::ToolStarted {
        call_id: call_id.to_owned(),
        name: name.to_owned(),
    }
}

#[test]
fn a_command_without_tools_shows_only_typing() {
    let mut command = command();
    command.apply(Step::Working);
    command.apply(Step::Said("Here is the answer.".to_owned()));
    command.apply(Step::Finished);
    assert_eq!(command.render(), None);
}

#[test]
fn intermediate_messages_and_tool_calls_are_listed_until_the_command_finishes() {
    let mut command = command();
    command.apply(Step::Said("Searching the library first.".to_owned()));
    command.apply(tool("one", "plugin_search"));
    assert_eq!(
        command.render().as_deref(),
        Some("**Working…**\n💭 Searching the library first.\n🔧 `plugin_search` …")
    );

    command.apply(Step::ToolFinished {
        call_id: "one".to_owned(),
        is_error: false,
    });
    command.apply(tool("two", "plugin_manage"));
    command.apply(Step::ToolFinished {
        call_id: "two".to_owned(),
        is_error: true,
    });
    command.apply(Step::Said("The final answer.".to_owned()));
    assert_eq!(
        command.render().as_deref(),
        Some(
            "**Working…**\n💭 Searching the library first.\n🔧 `plugin_search` ✓\n🔧 `plugin_manage` ✗"
        )
    );
    command.apply(Step::Finished);
    assert_eq!(
        command.render(),
        None,
        "a finished command shows no progress"
    );
}

#[test]
fn long_progress_keeps_the_latest_steps_within_one_discord_message() {
    let mut command = command();
    for index in 0..200 {
        command.apply(Step::Said(format!("step {index} {}", "x".repeat(300))));
        command.apply(tool(&index.to_string(), "bash"));
    }
    let body = command.render().expect("progress");
    assert!(body.chars().count() <= 2000, "{}", body.len());
    assert!(body.contains("earlier steps"));
    assert!(body.ends_with("🔧 `bash` …"));
    assert!(body.contains("step 199 "));
}

#[test]
fn a_tool_record_is_named_by_the_plugin_tool_it_runs() {
    use renoa_control::TaskEventKind;
    use renoa_protocol::{
        CommandId, ExecutionEvent, ExecutionEventId, ExecutionEventKind, ExecutionId,
    };

    let kind = TaskEventKind::ExecutionEvent {
        command_id: CommandId::new(),
        event: ExecutionEvent {
            event_id: ExecutionEventId::new(),
            execution_id: ExecutionId::new(),
            sequence: 3,
            recorded_at_ms: 0,
            kind: ExecutionEventKind::ToolStarted {
                call_id: "one".to_owned(),
                name: "tool_execute".to_owned(),
                arguments: serde_json::json!({
                    "reference": format!("mcp:exa:{}:web_search_exa", "a".repeat(64)),
                }),
            },
        },
    };
    let (_, step) = Step::of(&kind);
    assert_eq!(step, tool("one", "exa.web_search_exa"));
}

#[test]
fn a_command_idle_too_long_expires_and_a_finished_one_expires_after_one_edit() {
    let start = Instant::now();
    let mut idle = Command::new(target(), start);
    idle.apply(tool("one", "bash"), start);
    assert!(!idle.expired(start + IDLE_LIMIT.saturating_sub(Duration::from_secs(1))));
    assert!(idle.expired(start + IDLE_LIMIT));

    let mut finished = Command::new(target(), start);
    finished.apply(Step::Finished, start);
    assert!(!finished.expired(start));
    assert!(finished.expired(start + EDIT_INTERVAL));
}

#[tokio::test(start_paused = true)]
async fn a_stalled_timer_ticks_once_instead_of_bursting() {
    let mut timer = ticker(Duration::from_secs(8));
    timer.tick().await;
    tokio::time::advance(Duration::from_secs(40)).await;
    timer.tick().await;
    assert!(
        timer.tick().now_or_never().is_none(),
        "missed ticks are not replayed"
    );
}

/// A Discord API that records each channel request and answers message
/// creation with id 900.
async fn discord() -> (DiscordApi, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("http port");
    let address = listener.local_addr().expect("http address");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&requests);
    tokio::spawn(async move { crate::live_test::serve_http(listener, 0, recorded).await });
    let api =
        DiscordApi::with_origin("token".to_owned(), format!("http://{address}")).expect("api");
    (api, requests)
}

fn deleted(requests: &Mutex<Vec<String>>) -> bool {
    requests
        .lock()
        .expect("requests")
        .iter()
        .any(|request| request.starts_with("DELETE /channels/202/messages/900"))
}

#[tokio::test]
async fn a_restarted_surface_deletes_the_progress_message_its_last_run_posted() {
    let directory = tempfile::tempdir().expect("temp directory");
    let (api, requests) = discord().await;
    let start = Instant::now();
    {
        let store = SurfaceStore::open(directory.path()).expect("store");
        let mut commands = HashMap::new();
        let mut command = Command::new(target(), start);
        command.apply(tool("one", "bash"), start);
        commands.insert(COMMAND.to_owned(), command);
        show(&api, &store, &mut commands, start).await;
        assert_eq!(
            store.shown_progress().expect("shown")[0].message_id,
            "900",
            "the posted message is recorded"
        );
    }

    // The command finished while the surface was down.
    let store = SurfaceStore::open(directory.path()).expect("reopen store");
    let mut commands = resume(&store, start).expect("resume");
    show(&api, &store, &mut commands, start + EDIT_INTERVAL).await;

    assert!(deleted(&requests));
    assert!(store.shown_progress().expect("shown").is_empty());
    assert!(commands.is_empty());
}

#[tokio::test]
async fn an_idle_command_has_its_progress_message_deleted() {
    let directory = tempfile::tempdir().expect("temp directory");
    let (api, requests) = discord().await;
    let store = SurfaceStore::open(directory.path()).expect("store");
    let start = Instant::now();
    let mut commands = HashMap::new();
    let mut command = Command::new(target(), start);
    command.apply(tool("one", "bash"), start);
    commands.insert(COMMAND.to_owned(), command);
    show(&api, &store, &mut commands, start).await;

    show(&api, &store, &mut commands, start + IDLE_LIMIT).await;

    assert!(deleted(&requests));
    assert!(store.shown_progress().expect("shown").is_empty());
    assert!(commands.is_empty(), "an idle command is forgotten");
}
