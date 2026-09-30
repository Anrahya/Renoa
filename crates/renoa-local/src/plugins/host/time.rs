//! `renoa.time`: the current time in the agent's zone and the time since the
//! previous user message, admitted with each message.
//!
//! The zone is the agent's `timezone` setting, or else the Host's system zone,
//! which an operator sets with `TZ`. Without an IANA name for the system zone,
//! the time is shown in UTC.

use std::fmt::Write as _;

use jiff::{SignedDuration, Timestamp, fmt::strtime, tz::TimeZone};
use renoa_agent_loop::ContextContribution;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::plugins::PluginError;

pub(crate) const PLUGIN_ID: &str = "renoa.time";
pub(crate) const DESCRIPTION: &str = "Show the current time in this agent's time zone and the time since the previous user message with each message. Set the zone with configure_plugin settings {\"timezone\": \"<IANA name such as Asia/Kolkata>\"}; settings {} returns to the Host zone.";
const DISPLAY_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%:z[%Q]";

/// The settings `renoa.time` accepts. An absent zone means the Host zone.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct TimeSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timezone: Option<String>,
}

impl TimeSettings {
    /// Validates settings from a caller, before anything is stored.
    pub(crate) fn validate(value: Value) -> Result<Self, PluginError> {
        if !value.is_object() {
            return Err(PluginError::Invalid(
                "renoa.time settings must be an object".to_owned(),
            ));
        }
        let settings: Self = serde_json::from_value(value).map_err(|error| {
            PluginError::Invalid(format!("renoa.time settings are invalid: {error}"))
        })?;
        settings.zone()?;
        Ok(settings)
    }

    fn zone(&self) -> Result<TimeZone, PluginError> {
        self.timezone.as_deref().map_or_else(
            || Ok(TimeZone::try_system().unwrap_or(TimeZone::UTC)),
            |name| {
                TimeZone::get(name).map_err(|_| {
                    PluginError::Invalid(format!(
                        "`{name}` is not an IANA time zone name known to this Host"
                    ))
                })
            },
        )
    }

    /// The entry for a message admitted at `observed_at_ms`.
    pub(crate) fn contribution(
        &self,
        observed_at_ms: i64,
        previous_ms: Option<i64>,
    ) -> Result<ContextContribution, PluginError> {
        let invalid = |error: jiff::Error| PluginError::Invalid(error.to_string());
        let now = Timestamp::from_millisecond(observed_at_ms).map_err(invalid)?;
        let mut text = format!(
            "current_time: {}",
            strtime::format(DISPLAY_FORMAT, &now.to_zoned(self.zone()?)).map_err(invalid)?
        );
        // A clock that moved backward has no honest elapsed time.
        if let Some(elapsed) = previous_ms
            .and_then(|previous| observed_at_ms.checked_sub(previous))
            .filter(|elapsed| *elapsed >= 0)
        {
            let _ = write!(
                text,
                "\nelapsed_since_previous_user_message: {:#}",
                SignedDuration::from_millis(elapsed)
            );
        }
        ContextContribution::plugin(PLUGIN_ID, text)
            .map_err(|error| PluginError::Invalid(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::TimeSettings;

    #[test]
    fn the_entry_shows_the_configured_zone_and_elapsed_time() {
        let settings = TimeSettings::validate(json!({"timezone": "Asia/Kolkata"})).expect("zone");

        let entry = settings
            .contribution(1_788_199_445_000, Some(1_788_195_845_000 - 4_321))
            .expect("entry");

        assert_eq!(entry.source(), "plugin:renoa.time");
        assert_eq!(
            entry.text(),
            "current_time: 2026-08-31T23:34:05+05:30[Asia/Kolkata]\nelapsed_since_previous_user_message: 1h 4s 321ms"
        );
        let seoul = TimeSettings::validate(json!({"timezone": "Asia/Seoul"})).expect("zone");
        assert!(
            seoul
                .contribution(1_788_199_445_000, None)
                .expect("entry")
                .text()
                .ends_with("2026-09-01T03:04:05+09:00[Asia/Seoul]")
        );
    }

    #[test]
    fn a_first_message_or_a_backward_clock_has_no_elapsed_line() {
        let settings = TimeSettings::validate(json!({"timezone": "UTC"})).expect("zone");
        for previous in [None, Some(2_000)] {
            let entry = settings.contribution(1_000, previous).expect("entry");
            assert_eq!(entry.text(), "current_time: 1970-01-01T00:00:01+00:00[UTC]");
        }
    }

    #[test]
    fn invalid_settings_are_refused() {
        for invalid in [
            json!({"timezone": "Mars/Olympus"}),
            json!({"timezone": 5}),
            json!({"zone": "UTC"}),
            json!([]),
        ] {
            assert!(
                TimeSettings::validate(invalid.clone()).is_err(),
                "{invalid}"
            );
        }
        assert_eq!(
            TimeSettings::validate(json!({})).expect("empty"),
            TimeSettings::default()
        );
    }
}
