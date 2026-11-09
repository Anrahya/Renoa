//! The routines as the sheet holds them: the Host's automations as read,
//! and how a receipt or a later read changes what is shown.

use chrono::Local;
use serde::Deserialize;
use uuid::Uuid;

use crate::{sheets::schedule, stamp};

/// The parts of the Host's observation (`GET /v1/host`) this reads.
#[derive(Debug, Deserialize)]
pub(crate) struct Observation {
    pub(crate) automations: Vec<Automation>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Automation {
    pub(crate) id: String,
    pub(crate) agent_id: Uuid,
    pub(crate) name: String,
    pub(crate) schedule: schedule::Schedule,
    pub(crate) enabled: bool,
    pub(crate) revision: i64,
    pub(crate) next_due_ms: i64,
    #[serde(default)]
    pub(crate) last_run: Option<Run>,
    /// The Host plugin whose own trigger this is, such as a reminder.
    #[serde(default)]
    pub(crate) plugin: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Run {
    status: Outcome,
    due_ms: i64,
    #[serde(default)]
    finished_at_ms: Option<i64>,
}

/// How a run ended. The names are the Host's run statuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Outcome {
    /// The agent finished it, even if some of its tool calls failed.
    Succeeded,
    Failed,
    /// It came due too late to be worth running.
    Skipped,
}

/// The Host's receipt for turning a routine on or off.
#[derive(Debug, Deserialize)]
pub(crate) struct Enablement {
    pub(crate) revision: i64,
    pub(crate) enabled: bool,
    pub(crate) next_due_ms: i64,
}

/// Shows the Host's receipt on routine `id`, unless what is shown is
/// already newer: a read that landed after the switch.
pub(crate) fn take(automations: &mut [Automation], id: &str, receipt: &Enablement) {
    if let Some(automation) = automations
        .iter_mut()
        .find(|automation| automation.id == id)
        && automation.revision < receipt.revision
    {
        automation.revision = receipt.revision;
        automation.enabled = receipt.enabled;
        automation.next_due_ms = receipt.next_due_ms;
    }
}

/// What a read shows, given what is shown: a routine whose receipt is newer
/// than the read keeps the receipt, as a read sent before a switch can land
/// after it.
pub(crate) fn merge(shown: Option<&[Automation]>, read: Vec<Automation>) -> Vec<Automation> {
    read.into_iter()
        .map(|read| {
            shown
                .into_iter()
                .flatten()
                .find(|shown| shown.id == read.id && shown.revision > read.revision)
                .cloned()
                .unwrap_or(read)
        })
        .collect()
}

/// The routines an agent or the owner made: a plugin's own triggers, such as
/// reminders, are the Host's work, and Memory shows them.
pub(crate) fn own(automations: Vec<Automation>) -> Vec<Automation> {
    automations
        .into_iter()
        .filter(|automation| automation.plugin.is_none())
        .collect()
}

/// How a routine's last run went, and whether that needs the owner.
pub(crate) fn last_run(run: &Run) -> (String, bool) {
    let now = Local::now();
    let at = stamp::long(run.finished_at_ms.unwrap_or(run.due_ms), &now);
    match run.status {
        Outcome::Succeeded => (format!("Last ran {}", lower_first(&at)), false),
        Outcome::Failed => (
            format!("Last run didn't finish · {}", lower_first(&at)),
            true,
        ),
        Outcome::Skipped => (
            format!("Skipped {}: it came due too late", lower_first(&at)),
            false,
        ),
    }
}

/// "Today 6:12 PM" as it reads mid-sentence: "today 6:12 PM".
fn lower_first(text: &str) -> String {
    match text.split_once(' ') {
        Some((first @ ("Today" | "Yesterday"), rest)) => format!("{} {rest}", first.to_lowercase()),
        _ => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Enablement, Observation, Outcome, lower_first, merge, own, take};

    const OBSERVED: &str = r#"{"automations":[
        {"id":"a1","agent_id":"1b4e28ba-2fa1-11d2-883f-0016d3cca427","name":"Dinner ideas",
         "schedule":{"kind":"cron","expression":"30 18 * * *","timezone":"Asia/Kolkata"},
         "enabled":true,"revision":3,"next_due_ms":1762695000000,
         "last_run":{"status":"failed","due_ms":1762608600000,"finished_at_ms":1762608660000}},
        {"id":"r1","agent_id":"1b4e28ba-2fa1-11d2-883f-0016d3cca427","name":"Renew the gym",
         "schedule":{"kind":"once","at":"2026-11-12T03:30:00Z"},
         "enabled":true,"revision":1,"next_due_ms":1762918200000,"plugin":"renoa.memory"}
    ],"main_agent":null}"#;

    #[test]
    fn the_hosts_automations_read_and_its_plugins_triggers_are_left_to_memory() {
        let observed: Observation =
            serde_json::from_str(OBSERVED).expect("the Host's observation reads");
        let own = own(observed.automations);
        let [dinner] = &own[..] else {
            panic!("one routine: {own:?}")
        };
        assert_eq!(dinner.name, "Dinner ideas");
        assert_eq!(
            dinner.last_run.as_ref().map(|run| run.status),
            Some(Outcome::Failed)
        );
    }

    #[test]
    fn a_day_reads_mid_sentence() {
        assert_eq!(lower_first("Today 6:12 PM"), "today 6:12 PM");
        assert_eq!(lower_first("Yesterday 9:03 AM"), "yesterday 9:03 AM");
        assert_eq!(lower_first("Monday 6:12 PM"), "Monday 6:12 PM");
    }

    #[test]
    fn a_receipt_shows_unless_a_newer_read_came_first() {
        let observed: Observation =
            serde_json::from_str(OBSERVED).expect("the Host's observation reads");
        let mut shown = own(observed.automations);
        let paused = |revision| Enablement {
            revision,
            enabled: false,
            next_due_ms: 1,
        };
        take(&mut shown, "a1", &paused(3));
        assert!(shown[0].enabled, "a receipt no newer than the read is old");
        take(&mut shown, "a1", &paused(4));
        assert!(!shown[0].enabled);
        assert_eq!((shown[0].revision, shown[0].next_due_ms), (4, 1));
        take(&mut shown, "gone", &paused(9));
        assert_eq!(shown[0].revision, 4, "a receipt for another routine");
    }

    #[test]
    fn a_read_keeps_a_newer_receipt_and_takes_the_rest() {
        let read = || {
            own(serde_json::from_str::<Observation>(OBSERVED)
                .expect("the Host's observation reads")
                .automations)
        };
        let mut shown = read();
        take(
            &mut shown,
            "a1",
            &Enablement {
                revision: 4,
                enabled: false,
                next_due_ms: 1,
            },
        );
        let merged = merge(Some(&shown), read());
        assert_eq!((merged[0].revision, merged[0].enabled), (4, false));
        let mut older = read();
        older[0].revision = 2;
        let merged = merge(Some(&older), read());
        assert_eq!(
            (merged[0].revision, merged[0].enabled),
            (3, true),
            "an older one shown gives way to the read"
        );
        assert_eq!(merge(None, read()).len(), 1);
    }
}
