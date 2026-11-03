//! What an item is: its kind, its action, and the bounds of its text.

use serde::Serialize;

/// The most characters an item's title holds.
pub(super) const TITLE_CHARS: usize = 120;
/// The most characters an item's body holds, and so an agent's question or
/// message to the owner.
pub(crate) const BODY_CHARS: usize = 240;
/// The most characters the Today sentence holds.
pub(crate) const TODAY_CHARS: usize = 140;
/// The fewest and most choices an ask offers, and the most characters each holds.
pub(crate) const CHOICES: std::ops::RangeInclusive<usize> = 2..=6;
pub(crate) const CHOICE_CHARS: usize = 40;

/// What an item reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedKind {
    /// An agent asks the owner something.
    Ask,
    /// An agent tells the owner something.
    Update,
    /// An automation's run finished in a task of its own.
    Result,
    /// An automation's run failed or was skipped.
    Problem,
    /// An agent turned an ability on for itself.
    Ability,
}

impl FeedKind {
    pub(super) const ALL: [Self; 5] = [
        Self::Ask,
        Self::Update,
        Self::Result,
        Self::Problem,
        Self::Ability,
    ];

    /// The stored name, the same as the serialized one.
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Update => "update",
            Self::Result => "result",
            Self::Problem => "problem",
            Self::Ability => "ability",
        }
    }

    /// Whether an open item of this kind waits on the owner. The one place
    /// this is decided: the Today line and its count read it through
    /// [`needing_owner`].
    pub(super) const fn needs_owner(self) -> bool {
        match self {
            Self::Ask | Self::Problem => true,
            Self::Update | Self::Result | Self::Ability => false,
        }
    }
}

/// The kinds that wait on the owner, as an SQL list.
pub(super) fn needing_owner() -> String {
    sql_list(
        FeedKind::ALL
            .into_iter()
            .filter(|kind| kind.needs_owner())
            .map(FeedKind::as_str),
    )
}

/// How soon the owner should see an item: an agent picks it for an ask or an
/// update, a problem is `Soon`, and every other kind is `Whenever`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Urgency {
    /// Nothing is lost if the owner sees it tomorrow.
    Whenever,
    /// The owner should see it within the hour.
    Soon,
    /// Waiting costs the owner something within minutes.
    Now,
}

impl Urgency {
    pub(crate) const ALL: [Self; 3] = [Self::Whenever, Self::Soon, Self::Now];

    /// Whether an open item of this urgency wakes the owner's phone. The one
    /// place this is decided: the owed wakes read it through [`waking`].
    pub(super) const fn wakes(self) -> bool {
        match self {
            Self::Whenever => false,
            Self::Soon | Self::Now => true,
        }
    }

    /// Whether the wake rings rather than buzzes.
    pub(super) const fn rings(self) -> bool {
        match self {
            Self::Whenever | Self::Soon => false,
            Self::Now => true,
        }
    }

    /// The stored name, the same as the serialized one.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Whenever => "whenever",
            Self::Soon => "soon",
            Self::Now => "now",
        }
    }
}

/// The urgencies that wake the owner's phone, as an SQL list.
pub(super) fn waking() -> String {
    sql_list(
        Urgency::ALL
            .into_iter()
            .filter(|urgency| urgency.wakes())
            .map(Urgency::as_str),
    )
}

/// What the owner can do about an item, through the operation that owns it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedAction {
    /// Answer in the agent's conversation.
    Reply,
    /// Read what it points at.
    Open,
    /// Run the automation now.
    Retry,
    /// Turn the ability off for that agent.
    TurnOff,
}

impl FeedAction {
    pub(super) const ALL: [Self; 4] = [Self::Reply, Self::Open, Self::Retry, Self::TurnOff];

    /// The stored name, the same as the serialized one.
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Reply => "reply",
            Self::Open => "open",
            Self::Retry => "retry",
            Self::TurnOff => "turn_off",
        }
    }
}

macro_rules! stored_name {
    ($type:ty, $what:literal) => {
        impl rusqlite::types::FromSql for $type {
            fn column_result(
                value: rusqlite::types::ValueRef<'_>,
            ) -> rusqlite::types::FromSqlResult<Self> {
                let stored = value.as_str()?;
                Self::ALL
                    .into_iter()
                    .find(|variant| variant.as_str() == stored)
                    .ok_or_else(|| {
                        rusqlite::types::FromSqlError::Other(
                            format!(concat!("unknown feed ", $what, " `{}`"), stored).into(),
                        )
                    })
            }
        }
    };
}
stored_name!(FeedKind, "kind");
stored_name!(FeedAction, "action");
stored_name!(Urgency, "urgency");

/// `'a','b'` from names that hold no quote.
pub(super) fn sql_list(names: impl IntoIterator<Item = &'static str>) -> String {
    names
        .into_iter()
        .map(|name| format!("'{name}'"))
        .collect::<Vec<_>>()
        .join(",")
}

/// `text` on one line within `limit` characters: runs of whitespace become
/// one space, and a longer text ends in an ellipsis.
pub(super) fn fit(text: &str, limit: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= limit {
        return line;
    }
    let mut cut: String = line.chars().take(limit.saturating_sub(1)).collect();
    cut.truncate(cut.trim_end().len());
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use super::{BODY_CHARS, FeedAction, FeedKind, TITLE_CHARS, Urgency, fit};

    #[test]
    fn a_kind_and_an_action_are_stored_under_their_serialized_names() {
        for kind in FeedKind::ALL {
            assert_eq!(serde_json::to_value(kind).expect("kind"), kind.as_str());
        }
        for action in FeedAction::ALL {
            assert_eq!(
                serde_json::to_value(action).expect("action"),
                action.as_str()
            );
        }
        for urgency in Urgency::ALL {
            assert_eq!(
                serde_json::to_value(urgency).expect("urgency"),
                urgency.as_str()
            );
        }
    }

    #[test]
    fn text_is_fitted_onto_one_line_within_its_bound() {
        assert_eq!(fit("  Morning\n\tdigest  ", TITLE_CHARS), "Morning digest");
        let long = "é".repeat(BODY_CHARS + 10);
        let fitted = fit(&long, BODY_CHARS);
        assert_eq!(fitted.chars().count(), BODY_CHARS);
        assert!(fitted.ends_with('…'));
        assert_eq!(
            fit(&"a".repeat(TITLE_CHARS), TITLE_CHARS).chars().count(),
            TITLE_CHARS
        );
    }
}
