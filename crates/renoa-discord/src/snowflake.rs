use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::DiscordError;

/// A Discord snowflake stored as its canonical decimal text.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub(crate) struct Snowflake(String);

impl Snowflake {
    pub(crate) fn parse(value: &str) -> Result<Self, DiscordError> {
        Self::try_from(value.to_owned())
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Snowflake {
    type Error = DiscordError;

    fn try_from(value: String) -> Result<Self, DiscordError> {
        if value.is_empty()
            || value.len() > 20
            || value.starts_with('0')
            || !value.bytes().all(|byte| byte.is_ascii_digit())
            || value.parse::<u64>().is_err()
        {
            return Err(DiscordError::Invalid(
                "Discord snowflake must be a positive decimal integer".to_owned(),
            ));
        }
        Ok(Self(value))
    }
}

impl From<Snowflake> for String {
    fn from(value: Snowflake) -> Self {
        value.0
    }
}

/// Canonical decimal text has no leading zeros, so length then digits is
/// numeric order.
impl Ord for Snowflake {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .len()
            .cmp(&other.0.len())
            .then_with(|| self.0.cmp(&other.0))
    }
}

impl PartialOrd for Snowflake {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::Snowflake;

    #[test]
    fn order_is_numeric_not_lexicographic() {
        let small = Snowflake::parse("99").unwrap();
        let large = Snowflake::parse("100").unwrap();
        assert!(small < large);
        assert!(Snowflake::parse("18446744073709551615").unwrap() > large);
        assert!(serde_json::from_str::<Snowflake>("\"012\"").is_err());
        assert_eq!(serde_json::to_string(&large).unwrap(), "\"100\"");
    }
}
