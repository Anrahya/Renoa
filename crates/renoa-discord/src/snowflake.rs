use crate::DiscordError;

/// A Discord snowflake stored as its decimal text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Snowflake(String);

impl Snowflake {
    pub(crate) fn parse(value: &str) -> Result<Self, DiscordError> {
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
        Ok(Self(value.to_owned()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}
