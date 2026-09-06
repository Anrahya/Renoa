use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum Command {
    Prompt(String),
    Cancel,
    New,
    Status,
    Model(Option<String>),
    Reasoning(Option<String>),
    Compact,
    Help,
    Agent(Option<String>),
    Notice(String),
}

impl Command {
    pub(crate) fn parse(text: &str) -> Self {
        let mut words = text.split_whitespace();
        let first = words.next().unwrap_or("");
        let argument = words.next().map(str::to_owned);
        if words.next().is_some() {
            return Self::Prompt(text.to_owned());
        }
        match (first, argument) {
            ("!cancel", None) => Self::Cancel,
            ("!new", None) => Self::New,
            ("!status", None) => Self::Status,
            ("!compact", None) => Self::Compact,
            ("!help" | "", None) => Self::Help,
            ("!agent", value) => Self::Agent(value),
            ("!model", value) => Self::Model(value),
            ("!reasoning", value) => Self::Reasoning(value),
            _ => Self::Prompt(text.to_owned()),
        }
    }

    pub(crate) const fn executes_model(&self) -> bool {
        matches!(self, Self::Prompt(_) | Self::Compact)
    }
}
