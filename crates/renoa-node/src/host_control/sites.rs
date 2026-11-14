//! Published static sites listed and unpublished through the owner's Host
//! routes; the site gateway serves them.

use renoa_local::{SiteError, Sites};
use renoa_protocol::PrincipalId;
use serde_json::json;

use super::reply::{BAD_REQUEST, NOT_FOUND, Reply, UNAVAILABLE, log};

pub(super) async fn list(sites: &Sites, principal: PrincipalId) -> Reply {
    match sites.list(principal.as_uuid()).await {
        Ok(list) => Reply::json(&json!({ "sites": list })),
        Err(error) => failure(&error),
    }
}

pub(super) async fn unpublish(sites: &Sites, principal: PrincipalId, name: &str) -> Reply {
    match sites.unpublish(principal.as_uuid(), name).await {
        Ok(unpublished) => Reply::json(&json!({ "name": name, "unpublished": unpublished })),
        Err(error) => failure(&error),
    }
}

fn failure(error: &SiteError) -> Reply {
    match error {
        SiteError::InvalidName => {
            Reply::failure(BAD_REQUEST, "invalid_request", &error.to_string())
        }
        SiteError::NotFound => Reply::failure(NOT_FOUND, "site_not_found", &error.to_string()),
        _ => {
            log("sites", &error.to_string());
            Reply::failure(
                UNAVAILABLE,
                "host_unavailable",
                "The published site could not be read. Try again in a moment.",
            )
        }
    }
}
