//! The signed-in owner's own `USER.md`, which agent creation shows so the
//! owner can decide whether a new agent reads it.

use renoa_local::{LocalHost, ProfileEditError};
use renoa_protocol::PrincipalId;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use super::reply::{CONFLICT, Reply, UNAVAILABLE, UNPROCESSABLE, log};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProfileEdit {
    expected_revision: String,
    content: String,
}

pub(super) async fn read(host: &LocalHost, principal: PrincipalId) -> Reply {
    match host.user_profile(principal.as_uuid()).await {
        Ok(profile) => Reply::json(&profile),
        Err(error) => {
            log("profile read", &error.to_string());
            Reply::failure(
                UNAVAILABLE,
                "profile_unavailable",
                "Your profile could not be read. Retry in a moment.",
            )
        }
    }
}

pub(super) async fn replace(host: &LocalHost, principal: PrincipalId, edit: ProfileEdit) -> Reply {
    match host
        .replace_user_profile(
            principal.as_uuid(),
            &edit.expected_revision,
            edit.content,
            &CancellationToken::new(),
        )
        .await
    {
        Ok(profile) => Reply::json(&profile),
        Err(ProfileEditError::Stale) => Reply::failure(
            CONFLICT,
            "revision_conflict",
            "Your profile changed since you opened it. Reload it before saving.",
        ),
        Err(ProfileEditError::Invalid(reason)) => {
            Reply::failure(UNPROCESSABLE, "invalid_profile", &reason)
        }
        Err(ProfileEditError::Unavailable(reason)) => {
            log("profile edit", &reason);
            Reply::failure(
                UNAVAILABLE,
                "profile_unavailable",
                "The change could not be confirmed. Retry the same save to recover its outcome.",
            )
        }
    }
}
