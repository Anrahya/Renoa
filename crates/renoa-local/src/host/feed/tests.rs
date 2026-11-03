//! The owner's reading of the feed, its Today line, retention and schema,
//! against a real catalog. Items are written here as the producers write
//! them; the producers have their own tests beside the facts they report.

use std::path::PathBuf;

use rusqlite::{Connection, params};
use uuid::Uuid;

use super::{FEED_PAGE, FeedError, FeedKind, HostFeed, produce, retention};
use crate::{host::catalog, plugins::host::HostPluginId};

const DAY: i64 = 24 * 3_600_000;

struct Store {
    _directory: tempfile::TempDir,
    root: PathBuf,
    agent: String,
}

impl Store {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("directory");
        let root = directory.path().to_path_buf();
        std::fs::create_dir_all(root.join("state")).expect("state directory");
        catalog::initialize(&root.join(catalog::HOST_DATABASE)).expect("catalog");
        let agent = Uuid::new_v4().to_string();
        crate::test_agents::insert_agent(&root.join(catalog::HOST_DATABASE), &agent);
        Self {
            _directory: directory,
            root,
            agent,
        }
    }

    fn database(&self) -> PathBuf {
        self.root.join(catalog::HOST_DATABASE)
    }

    fn db(&self) -> Connection {
        Connection::open(self.database()).expect("catalog")
    }

    fn feed(&self) -> HostFeed {
        HostFeed::open(&self.root).expect("feed")
    }

    /// Writes one open item of `kind` created at `at_ms`, and returns its
    /// sequence and id.
    fn item(&self, kind: FeedKind, at_ms: i64) -> (i64, Uuid) {
        let id = Uuid::new_v4();
        let sequence = self
            .db()
            .query_row(
                "INSERT INTO host_feed_items(id, kind, agent_id, title, body, action, subject_id, created_at_ms)
                 VALUES (?1, ?2, ?3, 'Title', 'Body', 'open', 'subject', ?4) RETURNING sequence",
                params![id.to_string(), kind.as_str(), self.agent, at_ms],
                |row| row.get(0),
            )
            .expect("item");
        (sequence, id)
    }

    fn today(&self, sentence: &str, through: i64) -> rusqlite::Result<usize> {
        self.db().execute(
            "INSERT INTO host_today(singleton, sentence, through_sequence) VALUES (1, ?1, ?2)
             ON CONFLICT(singleton) DO UPDATE SET sentence = excluded.sentence,
                 through_sequence = excluded.through_sequence",
            params![sentence, through],
        )
    }

    fn sequences(&self) -> Vec<i64> {
        self.db()
            .prepare("SELECT sequence FROM host_feed_items ORDER BY sequence")
            .expect("prepare")
            .query_map([], |row| row.get(0))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("sequences")
    }
}

#[tokio::test]
async fn seen_through_only_rises_and_two_devices_converge() {
    let store = Store::new();
    for _ in 0..3 {
        store.item(FeedKind::Update, 0);
    }
    let feed = store.feed();
    let seen = feed.see(2).await.expect("see");
    assert_eq!((seen.seen_through, seen.unread), (2, 1));
    let back = feed.see(1).await.expect("an older value");
    assert_eq!(
        (back.seen_through, back.unread),
        (2, 1),
        "it never moves back"
    );
    assert_eq!(feed.see(2).await.expect("a repeat"), seen);

    let (phone, room) = (store.feed(), store.feed());
    let (phone, room) = tokio::join!(phone.see(3), room.see(1));
    phone.expect("phone");
    room.expect("Control Room");
    let page = feed.list(None).await.expect("list");
    assert_eq!((page.seen_through, page.unread), (3, 0));
}

#[tokio::test]
async fn seeing_past_the_newest_item_is_refused_naming_both() {
    let store = Store::new();
    for _ in 0..3 {
        store.item(FeedKind::Update, 0);
    }
    let refused = store.feed().see(4).await.expect_err("ahead of the feed");
    assert!(matches!(
        refused,
        FeedError::AheadOfFeed {
            through: 4,
            newest: 3
        }
    ));
    let message = refused.to_string();
    assert!(message.contains("seen_through 4") && message.contains("newest feed item, 3"));
    assert_eq!(store.feed().list(None).await.expect("list").seen_through, 0);

    store
        .db()
        .execute("DELETE FROM host_feed_items", [])
        .expect("prune every item");
    assert_eq!(
        store
            .feed()
            .see(3)
            .await
            .expect("a pruned sequence")
            .seen_through,
        3,
        "the newest sequence handed out stays seeable after it is pruned"
    );
}

#[tokio::test]
async fn unread_counts_open_items_above_seen_through() {
    let store = Store::new();
    let (_, below) = store.item(FeedKind::Problem, 0);
    store.item(FeedKind::Result, 0);
    let (_, above) = store.item(FeedKind::Ask, 0);
    store.item(FeedKind::Ability, 0);
    let feed = store.feed();
    assert_eq!(feed.see(2).await.expect("see").unread, 2);
    feed.dismiss(above, 10).await.expect("dismiss");
    feed.dismiss(below, 10).await.expect("dismiss");
    let page = feed.list(None).await.expect("list");
    assert_eq!(page.unread, 1, "seeing does not resolve, dismissing does");
}

#[tokio::test]
async fn dismissing_marks_an_item_done_once_and_an_unknown_id_is_refused_naming_it() {
    let store = Store::new();
    let (_, id) = store.item(FeedKind::Problem, 0);
    let feed = store.feed();
    let dismissed = feed.dismiss(id, 10).await.expect("dismiss");
    assert!(dismissed.done);
    assert_eq!(dismissed.id, id);
    assert_eq!(feed.dismiss(id, 20).await.expect("a repeat"), dismissed);
    let done_at: i64 = store
        .db()
        .query_row(
            "SELECT done_at_ms FROM host_feed_items WHERE id = ?1",
            [id.to_string()],
            |row| row.get(0),
        )
        .expect("done");
    assert_eq!(done_at, 10, "the first dismissal's time stays");

    let unknown = Uuid::new_v4();
    let refused = feed.dismiss(unknown, 30).await.expect_err("unknown");
    assert!(matches!(refused, FeedError::NotFound(id) if id == unknown));
    assert!(refused.to_string().contains(&unknown.to_string()));
}

#[tokio::test]
async fn the_today_line_never_says_quiet_above_an_open_ask_or_problem() {
    let store = Store::new();
    let feed = store.feed();
    assert_eq!(feed.list(None).await.expect("list").today, "All quiet.");
    store.item(FeedKind::Result, 0);
    store.item(FeedKind::Update, 0);
    store.item(FeedKind::Ability, 0);
    assert_eq!(feed.list(None).await.expect("list").today, "All quiet.");
    let (problem, problem_id) = store.item(FeedKind::Problem, 0);
    assert_eq!(
        feed.list(None).await.expect("list").today,
        "1 thing needs you."
    );

    store
        .today("Your digest went out.", problem)
        .expect("sentence");
    assert_eq!(
        feed.list(None).await.expect("list").today,
        "Your digest went out.",
        "the sentence stands while nothing newer needs the owner"
    );
    let (_, ask) = store.item(FeedKind::Ask, 0);
    assert_eq!(
        feed.list(None).await.expect("list").today,
        "2 things need you.",
        "an open ask newer than the sentence replaces it"
    );
    feed.dismiss(ask, 1).await.expect("answer the ask");
    assert_eq!(
        feed.list(None).await.expect("list").today,
        "Your digest went out."
    );
    store.item(FeedKind::Update, 0);
    assert_eq!(
        feed.list(None).await.expect("list").today,
        "Your digest went out.",
        "a newer item that needs nobody leaves the sentence"
    );
    feed.dismiss(problem_id, 2).await.expect("dismiss");
    store.item(FeedKind::Problem, 0);
    assert_eq!(
        feed.list(None).await.expect("list").today,
        "1 thing needs you."
    );
}

#[test]
fn the_today_sentence_is_stored_only_as_one_line_of_1_to_140_characters() {
    let store = Store::new();
    for refused in [
        String::new(),
        "a".repeat(141),
        "two\nlines".to_owned(),
        "a\rb".to_owned(),
    ] {
        assert!(store.today(&refused, 0).is_err(), "{refused:?}");
    }
    store.today(&"é".repeat(140), 0).expect("140 characters");
}

#[tokio::test]
async fn a_page_is_newest_first_and_names_the_next_older_page() {
    let store = Store::new();
    let total = i64::from(FEED_PAGE) + 2;
    for _ in 0..total {
        store.item(FeedKind::Update, 0);
    }
    let first = store.feed().list(None).await.expect("first page");
    assert_eq!(first.items.len(), usize::from(FEED_PAGE));
    assert_eq!(first.items[0].sequence, total);
    assert_eq!(first.next_before, Some(3));
    let last = store
        .feed()
        .list(first.next_before)
        .await
        .expect("last page");
    assert_eq!(
        last.items
            .iter()
            .map(|item| item.sequence)
            .collect::<Vec<_>>(),
        [2, 1]
    );
    assert_eq!(last.next_before, None);
}

#[tokio::test]
async fn an_item_shows_its_agents_current_name() {
    let store = Store::new();
    store.item(FeedKind::Update, 0);
    store
        .db()
        .execute(
            "UPDATE host_agents SET name = 'Renamed' WHERE agent_id = ?1",
            [&store.agent],
        )
        .expect("rename");
    let page = store.feed().list(None).await.expect("list");
    assert_eq!(page.items[0].agent.name, "Renamed");
    assert_eq!(page.items[0].agent.id.to_string(), store.agent);
}

/// The removals' telemetry, which holds counts and no text.
fn reported(removed: &[crate::host::automations::Removed]) -> Vec<(&str, serde_json::Value)> {
    removed
        .iter()
        .map(|removed| {
            let (name, fields) = removed.parts();
            (name, fields.clone())
        })
        .collect()
}

#[test]
fn retention_keeps_the_newest_200_items() {
    let store = Store::new();
    for _ in 0..205 {
        store.item(FeedKind::Update, 0);
    }
    let removed = retention::expire(&store.db(), 0).expect("expire");
    assert_eq!(
        reported(&removed),
        [(
            "feed_items_pruned",
            serde_json::json!({ "items": 5, "reason": "count" })
        )]
    );
    assert_eq!(store.sequences(), (6..=205).collect::<Vec<_>>());
    assert!(retention::expire(&store.db(), 0).expect("again").is_empty());
}

#[test]
fn retention_removes_an_item_30_days_after_its_creation() {
    let store = Store::new();
    store.item(FeedKind::Update, 0);
    let (kept, _) = store.item(FeedKind::Result, 1);
    let removed = retention::expire(&store.db(), 30 * DAY + 1).expect("expire");
    assert_eq!(
        reported(&removed),
        [(
            "feed_items_pruned",
            serde_json::json!({ "items": 1, "reason": "age" })
        )]
    );
    assert_eq!(store.sequences(), [kept]);
}

#[test]
fn an_open_item_that_needs_the_owner_outlives_both_retention_rules() {
    let store = Store::new();
    let (ask, _) = store.item(FeedKind::Ask, 0);
    let (problem, _) = store.item(FeedKind::Problem, 0);
    for kind in [FeedKind::Ask, FeedKind::Problem] {
        let (_, done) = store.item(kind, 0);
        store
            .db()
            .execute(
                "UPDATE host_feed_items SET done_at_ms = 1 WHERE id = ?1",
                [done.to_string()],
            )
            .expect("resolve one of each");
    }
    for kind in [FeedKind::Update, FeedKind::Result, FeedKind::Ability] {
        store.item(kind, 0);
    }
    for _ in 0..200 {
        store.item(FeedKind::Update, 40 * DAY);
    }
    let removed = retention::expire(&store.db(), 40 * DAY).expect("expire");
    assert_eq!(
        reported(&removed),
        [(
            "feed_items_pruned",
            serde_json::json!({ "items": 5, "reason": "age" })
        )]
    );
    let sequences = store.sequences();
    assert_eq!(sequences.len(), 202);
    assert_eq!(
        sequences[..2],
        [ask, problem],
        "the open ask and problem are past both limits and stay"
    );
}

#[tokio::test]
async fn a_replayed_item_takes_no_sequence() {
    let store = Store::new();
    let db = store.db();
    for _ in 0..3 {
        produce::ability_turned_on(
            &db,
            &store.agent,
            produce::Ability::Compiled(HostPluginId::Memory),
            "on",
        )
        .expect("an item, then its replays");
    }
    assert!(matches!(
        store.feed().see(2).await,
        Err(FeedError::AheadOfFeed {
            through: 2,
            newest: 1
        })
    ));
    let (next, _) = store.item(FeedKind::Update, 0);
    assert_eq!(next, 2, "the next item takes the next sequence");
}

#[test]
fn schema_46_adds_the_three_feed_tables_empty_to_a_schema_45_catalog() {
    let store = Store::new();
    let database = store.database();
    let automation = Uuid::new_v4().to_string();
    let db = store.db();
    catalog::restore_schema_45(&db);
    db.execute_batch(&format!(
        "UPDATE host_metadata SET schema_version = 45 WHERE singleton = 1;
         PRAGMA user_version = 45;
         INSERT INTO host_automations(id, agent_id, name, prompt, schedule_json, enabled, revision, next_due_ms)
             VALUES ('{automation}', '{agent}', 'Digest', 'Write it.', '{{}}', 1, 1, 0);
         INSERT INTO host_automation_runs(id, automation_id, agent_id, due_ms, admitted_at_ms, submission, output, status, finished_at_ms)
             VALUES ('{run}', '{automation}', '{agent}', 0, 0, 'Write it.', 'Scheduled run failed: down', 'failed', 0);",
        agent = store.agent,
        run = Uuid::new_v4(),
    ))
    .expect("a schema 45 catalog with a failed run");
    drop(db);
    catalog::initialize(&database).expect("upgrade schema 45");
    catalog::initialize(&database).expect("reopen the upgraded catalog");
    let db = store.db();
    for table in ["host_feed_items", "host_feed_state", "host_today"] {
        let rows: i64 = db
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .expect("the table exists");
        assert_eq!(rows, 0, "{table} starts empty; nothing is backfilled");
    }
}

/// The owner's next message on a session answers the asks made there before
/// it, and nothing else: not an ask made after it or in another session, and
/// not when the message is an automation's run.
#[test]
fn a_message_answers_the_asks_before_it_in_its_own_session() {
    use renoa_kernel::{AgentId, CommandId, SessionId};
    let store = Store::new();
    let agent = AgentId::from_uuid(Uuid::parse_str(&store.agent).expect("agent"));
    let (session, other) = (SessionId::new(), SessionId::new());
    let ask = |session: SessionId, call: &str, at_ms: i64| {
        let db = store.db();
        produce::owner_asked(
            &db,
            &produce::ForOwner {
                agent,
                session,
                command: Some(CommandId::new()),
                call,
                text: "Book it?",
                urgency: super::Urgency::Soon,
            },
            None,
        )
        .expect("ask");
        db.execute(
            "UPDATE host_feed_items SET created_at_ms = ?1 WHERE sequence = (SELECT max(sequence) FROM host_feed_items)",
            [at_ms],
        )
        .expect("time");
    };
    ask(session, "before", 100);
    ask(session, "after", 300);
    ask(other, "elsewhere", 100);
    let run = Uuid::new_v4();
    let db = store.db();
    db.execute_batch(&format!(
        "INSERT INTO host_automations(id, agent_id, name, prompt, schedule_json, enabled, revision, next_due_ms)
         VALUES ('{automation}', '{agent}', 'Check', 'Check.', '{{\"kind\":\"once\",\"at\":\"1970-01-01T00:00:00Z\"}}', 1, 1, 100);
         INSERT INTO host_automation_runs(id, automation_id, agent_id, due_ms, admitted_at_ms, submission)
         VALUES ('{run}', '{automation}', '{agent}', 1, 1, '{{}}');",
        automation = Uuid::new_v4(),
        agent = store.agent,
    ))
    .expect("an automation run");

    let answered =
        |command: CommandId| produce::asks_answered(&db, session, command, 200).expect("answer");
    assert_eq!(
        answered(CommandId::from_uuid(run)),
        0,
        "a run answers nothing"
    );
    assert_eq!(answered(CommandId::new()), 1);
    let open: Vec<String> = db
        .prepare("SELECT body FROM host_feed_items WHERE done_at_ms IS NULL ORDER BY sequence")
        .expect("prepare")
        .query_map([], |row| row.get(0))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("open");
    assert_eq!(
        open.len(),
        2,
        "the later ask and the other session's stay open"
    );
    assert_eq!(
        answered(CommandId::new()),
        0,
        "answering again changes nothing"
    );
}
