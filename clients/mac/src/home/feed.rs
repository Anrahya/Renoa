//! The owner's feed on the Host (`GET /v1/host/feed`): the Today line, the
//! questions agents asked, and the notes they left, as the phone reads it
//! (`agents/Feed.kt`).

use std::collections::HashSet;

use serde::Deserialize;
use uuid::Uuid;

/// What an item is. The names are the Host's feed kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// An agent asks the owner something.
    Ask,
    /// An agent tells the owner something.
    Update,
    /// A routine ran in a conversation of its own.
    Result,
    /// A routine failed or was skipped; it can run again.
    Problem,
    /// An agent turned an ability on for itself.
    Ability,
    /// A kind this app does not know yet; it is not shown.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FeedAgent {
    pub id: Uuid,
    pub name: String,
}

/// An item of the feed, with the fields the app reads.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Item {
    pub id: Uuid,
    pub kind: Kind,
    pub agent: FeedAgent,
    pub title: String,
    pub body: String,
    pub at_ms: i64,
    pub done: bool,
    /// For an ask or an update, the task it was made in, or the Host
    /// session when no task runs it; for a problem, its routine.
    pub subject_id: String,
    #[serde(default)]
    pub choices: Option<Vec<String>>,
}

impl Item {
    /// What Home shows of it: an update's own words, else its title, which
    /// names the routine that ran or failed or the ability turned on; their
    /// bodies are the Host's stock words.
    pub fn words(&self) -> &str {
        if self.kind == Kind::Update {
            &self.body
        } else {
            &self.title
        }
    }
}

/// One page of the feed, newest first; `next_before` starts the next, older
/// page, or is `None` on the last.
#[derive(Debug, Clone, Deserialize)]
pub struct Page {
    pub today: String,
    pub items: Vec<Item>,
    #[serde(default)]
    pub next_before: Option<i64>,
}

/// The path of the page before `before`, or of the newest page.
pub fn page_path(before: Option<i64>) -> String {
    before.map_or_else(
        || "/v1/host/feed".to_owned(),
        |before| format!("/v1/host/feed/before/{before}"),
    )
}

/// Where the next older page starts after one that started at `at`: a
/// cursor that does not move back ends the read.
pub fn next(at: Option<i64>, page: &Page) -> Option<i64> {
    page.next_before
        .filter(|before| at.is_none_or(|at| *before < at))
}

/// The feed as the Host last listed it, less what the owner dealt with here.
#[derive(Debug, Clone, Default)]
pub struct Held {
    pub today: String,
    items: Vec<Item>,
    /// Answered, dismissed or run again here, before the Host says so.
    dealt: HashSet<Uuid>,
}

impl Held {
    /// The feed as read, keeping hidden what was dealt with here and is
    /// still open on the Host.
    pub fn read(&mut self, today: String, items: Vec<Item>) {
        self.dealt
            .retain(|id| items.iter().any(|item| item.id == *id && !item.done));
        self.today = today;
        self.items = items;
    }

    fn open(&self) -> impl Iterator<Item = &Item> {
        self.items
            .iter()
            .filter(|item| !item.done && !self.dealt.contains(&item.id))
    }

    /// The newest open question of each agent, newest first.
    pub fn asks(&self) -> Vec<&Item> {
        let mut asked = HashSet::new();
        self.open()
            .filter(|item| item.kind == Kind::Ask && asked.insert(item.agent.id))
            .collect()
    }

    /// The notes left for the owner, newest first.
    pub fn notes(&self) -> Vec<&Item> {
        self.open()
            .filter(|item| !matches!(item.kind, Kind::Ask | Kind::Unknown))
            .collect()
    }

    pub fn item(&self, id: Uuid) -> Option<&Item> {
        self.open().find(|item| item.id == id)
    }

    /// How many things wait on the owner: the questions of agents
    /// `answerable` says this Mac can answer, and routines that failed.
    pub fn needing_you(&self, answerable: impl Fn(Uuid) -> bool) -> usize {
        self.asks()
            .iter()
            .filter(|ask| answerable(ask.agent.id))
            .count()
            + self
                .open()
                .filter(|item| item.kind == Kind::Problem)
                .count()
    }

    /// Hides `id` here, as answered, dismissed or run again.
    pub fn deal(&mut self, id: Uuid) {
        self.dealt.insert(id);
    }

    /// Shows `id` again, as when what dealt with it was refused.
    pub fn restore(&mut self, id: Uuid) {
        self.dealt.remove(&id);
    }
}

/// How the owner's answer to a question asked in another conversation reads
/// in the agent's chat: the chat did not see the question, so the answer
/// names it.
pub fn from_elsewhere(question: &str, answer: &str) -> String {
    format!("About your question “{question}”: {answer}")
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{Held, Kind, Page, from_elsewhere, next, page_path};

    const PAGE: &str = r#"{"today":"Chef has dinner ideas at 6:30 PM.","unread":3,"seen_through":9,
        "items":[
          {"id":"00000000-0000-0000-0000-000000000005","sequence":9,"kind":"ask",
           "agent":{"id":"00000000-0000-0000-0000-0000000000a1","name":"Deals"},
           "title":"Deals asks","body":"Add two cartons?","at_ms":5,"done":false,
           "action":"answer","subject_id":"t1","urgency":"normal","choices":["Add two","Not now"]},
          {"id":"00000000-0000-0000-0000-000000000004","sequence":8,"kind":"ask",
           "agent":{"id":"00000000-0000-0000-0000-0000000000a1","name":"Deals"},
           "title":"Deals asks","body":"Older question","at_ms":4,"done":false,
           "action":"answer","subject_id":"t1","urgency":"normal","choices":null},
          {"id":"00000000-0000-0000-0000-000000000003","sequence":7,"kind":"problem",
           "agent":{"id":"00000000-0000-0000-0000-0000000000a2","name":"Chef"},
           "title":"Dinner ideas didn't finish","body":"It can run again.","at_ms":3,"done":false,
           "action":"retry","subject_id":"r1","urgency":"normal","choices":null},
          {"id":"00000000-0000-0000-0000-000000000002","sequence":6,"kind":"update",
           "agent":{"id":"00000000-0000-0000-0000-0000000000a2","name":"Chef"},
           "title":"Chef","body":"Paneer tonight.","at_ms":2,"done":false,
           "action":"open","subject_id":"t2","urgency":"normal","choices":null},
          {"id":"00000000-0000-0000-0000-000000000001","sequence":5,"kind":"update",
           "agent":{"id":"00000000-0000-0000-0000-0000000000a2","name":"Chef"},
           "title":"Chef","body":"Done already.","at_ms":1,"done":true,
           "action":"open","subject_id":"t2","urgency":"normal","choices":null},
          {"id":"00000000-0000-0000-0000-000000000000","sequence":4,"kind":"weather",
           "agent":{"id":"00000000-0000-0000-0000-0000000000a2","name":"Chef"},
           "title":"?","body":"?","at_ms":0,"done":false,
           "action":"open","subject_id":"t2","urgency":"normal","choices":null}
        ],"next_before":4}"#;

    fn held() -> Held {
        let page: Page = serde_json::from_str(PAGE).expect("the Host's page reads");
        let mut held = Held::default();
        held.read(page.today, page.items);
        held
    }

    #[test]
    fn home_shows_each_agents_newest_question_and_the_open_notes() {
        let held = held();
        let asks: Vec<&str> = held.asks().iter().map(|ask| ask.body.as_str()).collect();
        assert_eq!(asks, ["Add two cartons?"]);
        let notes: Vec<(Kind, &str)> = held
            .notes()
            .iter()
            .map(|note| (note.kind, note.words()))
            .collect();
        assert_eq!(
            notes,
            [
                (Kind::Problem, "Dinner ideas didn't finish"),
                (Kind::Update, "Paneer tonight.")
            ],
            "done items and kinds this app does not know are left out"
        );
        assert_eq!(
            held.needing_you(|_| true),
            2,
            "a question and a failed routine"
        );
        assert_eq!(
            held.needing_you(|_| false),
            1,
            "a question this computer cannot answer"
        );
        let mut held = held;
        held.deal(Uuid::from_u128(3));
        assert_eq!(held.needing_you(|_| true), 1, "the failed routine put away");
    }

    #[test]
    fn what_was_dealt_with_here_stays_hidden_across_reads_until_restored() {
        let mut held = held();
        let newest = Uuid::from_u128(5);
        held.deal(newest);
        assert_eq!(
            held.asks()
                .iter()
                .map(|ask| ask.body.as_str())
                .collect::<Vec<_>>(),
            ["Older question"],
            "the agent's next question shows"
        );
        let again = held.clone();
        held.read("Later".to_owned(), again.items.clone());
        assert!(held.item(newest).is_none());
        held.restore(newest);
        assert!(held.item(newest).is_some());
        held.deal(newest);
        let closed: Vec<_> = again
            .items
            .iter()
            .filter(|item| item.id != newest)
            .cloned()
            .collect();
        held.read("Later".to_owned(), closed);
        held.read("Later".to_owned(), again.items.clone());
        assert!(
            held.item(newest).is_some(),
            "once the Host no longer lists it, it is no longer held hidden"
        );
    }

    #[test]
    fn the_pages_are_read_back_until_the_cursor_stops_moving() {
        let page: Page = serde_json::from_str(PAGE).expect("the Host's page reads");
        assert_eq!(page_path(None), "/v1/host/feed");
        assert_eq!(page_path(Some(4)), "/v1/host/feed/before/4");
        assert_eq!(next(None, &page), Some(4));
        assert_eq!(next(Some(9), &page), Some(4));
        assert_eq!(
            next(Some(4), &page),
            None,
            "a cursor that does not move back"
        );
    }

    #[test]
    fn an_answer_from_elsewhere_names_its_question() {
        assert_eq!(
            from_elsewhere("Add two cartons?", "Add two"),
            "About your question “Add two cartons?”: Add two"
        );
    }
}
