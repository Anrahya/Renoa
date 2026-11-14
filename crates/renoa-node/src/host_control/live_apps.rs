//! Owner Host control routes for live apps (`GET /v1/host/apps`,
//! `POST /v1/host/apps/{name}/stop`).

use renoa_local::{AppError, Apps, LiveApp};
use renoa_protocol::PrincipalId;
use serde::Serialize;
use serde_json::json;

use super::reply::{BAD_REQUEST, NOT_FOUND, Reply, UNAVAILABLE, log};

const FORBIDDEN: u16 = 403;

#[derive(Serialize)]
struct LiveAppView<'a> {
    #[serde(flatten)]
    app: &'a LiveApp,
    path: String,
}

fn app_view(app: &LiveApp) -> LiveAppView<'_> {
    LiveAppView {
        app,
        path: format!("/apps/{}/", app.name),
    }
}

pub(super) async fn list(apps: &Apps, principal: PrincipalId) -> Reply {
    match apps.list(Some(principal.as_uuid())).await {
        Ok(list) => {
            let views: Vec<_> = list.iter().map(app_view).collect();
            Reply::json(&json!({ "apps": views }))
        }
        Err(error) => failure(&error),
    }
}

pub(super) async fn stop(apps: &Apps, principal: PrincipalId, name: &str) -> Reply {
    match apps.stop_for(name, Some(principal.as_uuid())).await {
        Ok(app) => Reply::json(&app_view(&app)),
        Err(error) => failure(&error),
    }
}

fn failure(error: &AppError) -> Reply {
    match error {
        AppError::InvalidName | AppError::InvalidFolder(_) | AppError::InvalidCommand => {
            Reply::failure(BAD_REQUEST, "invalid_request", &error.to_string())
        }
        AppError::NotFound => Reply::failure(NOT_FOUND, "app_not_found", &error.to_string()),
        AppError::PermissionDenied => Reply::failure(FORBIDDEN, "forbidden", &error.to_string()),
        AppError::NoAvailablePort
        | AppError::SupervisorUnavailable(_)
        | AppError::StartupFailed { .. }
        | AppError::Failed { .. } => {
            log("apps", &error.to_string());
            Reply::failure(UNAVAILABLE, "apps_unavailable", &error.to_string())
        }
        _ => {
            log("apps", &error.to_string());
            Reply::failure(
                UNAVAILABLE,
                "apps_unavailable",
                "The Host could not update live apps.",
            )
        }
    }
}
