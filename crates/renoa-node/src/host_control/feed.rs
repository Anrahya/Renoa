//! The owner's feed: one page of what happened, the Today line, and the read
//! state every device shares.

use renoa_local::{FeedError, FeedKind, FeedPage, HostFeed, LocalHost, TurnObservation};
use serde::Deserialize;
use uuid::Uuid;

use super::reply::{BAD_REQUEST, NOT_FOUND, Reply, UNAVAILABLE, UNPROCESSABLE, log};
use crate::node_store::NodeStore;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Seen {
    through: u64,
}

pub(super) async fn list(host: &LocalHost, tasks: &NodeStore, before: Option<&str>) -> Reply {
    let Ok(before) = before.map(str::parse::<i64>).transpose() else {
        return Reply::failure(
            BAD_REQUEST,
            "invalid_request",
            "A page starts before a feed sequence, such as /v1/host/feed/before/120.",
        );
    };
    let feed = match open(host) {
        Ok(feed) => feed,
        Err(reply) => return reply,
    };
    let page = match feed.list(before).await {
        Ok(page) => page,
        Err(error) => return answer::<()>("feed list", Err(error)),
    };
    match name_tasks(tasks, page).await {
        Ok(page) => Reply::json(&page),
        Err(error) => {
            log("feed tasks", &error.to_string());
            Reply::failure(
                UNAVAILABLE,
                "host_unavailable",
                "The feed is temporarily unavailable. Retry the same request.",
            )
        }
    }
}

/// Names the task of each ask and update, which the Host knows only as its
/// session: a client answers an ask by submitting to that task. A session no
/// task of this node runs keeps its id.
async fn name_tasks(
    tasks: &NodeStore,
    mut page: FeedPage,
) -> Result<FeedPage, crate::node_store::NodeStoreError> {
    for item in &mut page.items {
        let names_session = match item.kind {
            FeedKind::Ask | FeedKind::Update => true,
            FeedKind::Result | FeedKind::Problem | FeedKind::Ability => false,
        };
        let Ok(session) = Uuid::parse_str(&item.subject_id) else {
            continue;
        };
        if names_session && let Some(task) = tasks.task_for_session(session).await? {
            item.subject_id = task.to_string();
        }
    }
    Ok(page)
}

pub(super) async fn see(host: &LocalHost, seen: Seen) -> Reply {
    match open(host) {
        Ok(feed) => answer("feed see", feed.see(seen.through).await),
        Err(reply) => reply,
    }
}

pub(super) async fn dismiss(host: &LocalHost, id: &str) -> Reply {
    let Ok(id) = Uuid::parse_str(id) else {
        return Reply::failure(
            BAD_REQUEST,
            "invalid_request",
            "A feed item id is a UUID, such as 0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0.",
        );
    };
    let now = match TurnObservation::now() {
        Ok(now) => now.unix_milliseconds(),
        Err(error) => {
            log("feed clock", &error.to_string());
            return Reply::failure(
                UNAVAILABLE,
                "clock_unavailable",
                "The Host clock is unavailable. Retry the same request.",
            );
        }
    };
    match open(host) {
        Ok(feed) => answer("feed dismiss", feed.dismiss(id, now).await),
        Err(reply) => reply,
    }
}

fn open(host: &LocalHost) -> Result<HostFeed, Reply> {
    HostFeed::open(host.home().path()).map_err(|error| unavailable("feed open", &error))
}

fn answer<T: serde::Serialize>(operation: &str, outcome: Result<T, FeedError>) -> Reply {
    match outcome {
        Ok(value) => Reply::json(&value),
        Err(error @ FeedError::NotFound(_)) => {
            Reply::failure(NOT_FOUND, "not_found", &error.to_string())
        }
        Err(error @ FeedError::AheadOfFeed { .. }) => {
            Reply::failure(UNPROCESSABLE, "ahead_of_feed", &error.to_string())
        }
        Err(error) => unavailable(operation, &error),
    }
}

fn unavailable(operation: &str, error: &FeedError) -> Reply {
    log(operation, &error.to_string());
    Reply::failure(
        UNAVAILABLE,
        "host_unavailable",
        "The feed is temporarily unavailable. Retry the same request.",
    )
}

#[cfg(test)]
mod tests {
    use renoa_kernel::AgentId;
    use renoa_local::{FeedAction, FeedAgent, FeedItem, FeedKind, FeedPage, Urgency};
    use uuid::Uuid;

    use super::name_tasks;
    use crate::node_store::NodeStore;

    fn item(kind: FeedKind, subject_id: String) -> FeedItem {
        FeedItem {
            id: Uuid::new_v4(),
            sequence: 1,
            kind,
            agent: FeedAgent {
                id: AgentId::new(),
                name: "Renoa".to_owned(),
            },
            title: "Title".to_owned(),
            body: String::new(),
            at_ms: 1,
            done: false,
            action: FeedAction::Reply,
            subject_id,
            urgency: Urgency::Soon,
            choices: None,
        }
    }

    /// A client answers an ask by submitting to its task, so an ask or an
    /// update names the task of its session; other kinds keep their subject,
    /// and a session no task runs keeps its id.
    #[tokio::test]
    async fn an_ask_names_the_task_of_its_session() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("node.sqlite3");
        let store = NodeStore::open(&path).expect("node store");
        let (session, task, unknown) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        rusqlite::Connection::open(&path)
            .expect("node database")
            .execute(
                "INSERT INTO host_node_tasks(task_id, target, agent_id, session_id, workspace)
                 VALUES (?1, 'target', 'agent', ?2, 'workspace')",
                [task.to_string(), session.to_string()],
            )
            .expect("task");
        let page = FeedPage {
            today: String::new(),
            unread: 0,
            seen_through: 0,
            items: vec![
                item(FeedKind::Ask, session.to_string()),
                item(FeedKind::Update, session.to_string()),
                item(FeedKind::Problem, session.to_string()),
                item(FeedKind::Ask, unknown.to_string()),
            ],
            next_before: None,
        };
        let subjects: Vec<String> = name_tasks(&store, page)
            .await
            .expect("named")
            .items
            .into_iter()
            .map(|item| item.subject_id)
            .collect();
        assert_eq!(
            subjects,
            [
                task.to_string(),
                task.to_string(),
                session.to_string(),
                unknown.to_string()
            ]
        );
    }
}
