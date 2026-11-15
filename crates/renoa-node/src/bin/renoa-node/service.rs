use std::path::Path;

use renoa_node::RenoaNode;
use tokio_util::sync::CancellationToken;

use crate::{config, error::ServiceError, log};

pub(crate) async fn serve(
    config_path: &Path,
    credentials_path: &Path,
    state_directory: &Path,
) -> Result<(), ServiceError> {
    let config = config::load(config_path, credentials_path, state_directory)?;
    // Bound before the node opens, so an address in use stops the start.
    let gateway =
        match config.site_gateway {
            Some(address) => Some(tokio::net::TcpListener::bind(address).await.map_err(
                |error| {
                    ServiceError::Configuration(format!(
                        "the site gateway cannot listen on {address}: {error}"
                    ))
                },
            )?),
            None => None,
        };
    let sites = renoa_local::Sites::open(config.host.home());
    let apps = renoa_local::Apps::open(config.host.home());
    // Nothing publishes yet, so what no site names is left from a crash.
    match sites.sweep().await {
        Ok(0) => {}
        Ok(removed) => log::event(
            "info",
            "site_snapshots_swept",
            &serde_json::json!({ "removed": removed }),
        ),
        Err(error) => log::event(
            "warn",
            "site_snapshots_sweep_failed",
            &serde_json::json!({ "error": error.to_string() }),
        ),
    }
    // Nothing starts an app yet, so a directory no app names is left from a crash.
    match apps.sweep_orphan_dirs() {
        Ok(0) => {}
        Ok(removed) => log::event(
            "info",
            "live_app_dirs_swept",
            &serde_json::json!({ "removed": removed }),
        ),
        Err(error) => log::event(
            "warn",
            "live_app_dirs_sweep_failed",
            &serde_json::json!({ "error": error.to_string() }),
        ),
    }
    let device_id = config.credentials.device_id.to_string();
    let home = config.host.home().path().display().to_string();
    let drain = CancellationToken::new();
    let node = RenoaNode::open(config.endpoint, config.credentials, config.host)?
        .with_automations(config.automation_credentials)?
        .with_drain(drain.clone());
    let shutdown = CancellationToken::new();
    spawn_apps_startup_reconcile(apps.clone(), shutdown.clone());
    let signal = wait_for_shutdown();
    tokio::pin!(signal);
    log::event(
        "info",
        "service_started",
        &serde_json::json!({
            "device_id": device_id,
            "home": home,
        }),
    );
    if let Some(listener) = gateway {
        let stop = shutdown.clone();
        tokio::spawn(async move {
            if let Err(error) = renoa_node::serve_site_gateway(listener, sites, apps, stop).await {
                log::event(
                    "error",
                    "site_gateway_stopped",
                    &serde_json::json!({ "error": error.to_string() }),
                );
            }
        });
    }
    let run = node.run(shutdown.clone());
    tokio::pin!(run);
    let result = tokio::select! {
        result = &mut run => result.map_err(ServiceError::from),
        result = &mut signal => {
            result?;
            // The first signal drains: the turns the node took finish and new
            // commands stay queued at the coordinator. A second stops at once.
            drain.cancel();
            log::event("info", "service_draining", &serde_json::json!({}));
            let again = wait_for_shutdown();
            tokio::pin!(again);
            tokio::select! {
                result = &mut run => result.map_err(ServiceError::from),
                result = &mut again => {
                    result?;
                    shutdown.cancel();
                    run.await.map_err(ServiceError::from)
                }
            }
        }
    };
    if result.is_ok() {
        log::event("info", "service_stopped", &serde_json::json!({}));
    }
    result
}

fn spawn_apps_startup_reconcile(apps: renoa_local::Apps, stop: CancellationToken) {
    tokio::spawn(async move {
        let mut delay = std::time::Duration::from_secs(1);
        loop {
            match apps.reconcile_on_startup().await {
                Ok(()) => return,
                Err(error) => {
                    log::event(
                        "warn",
                        "live_app_startup_reconcile_failed",
                        &serde_json::json!({
                            "error": error.to_string(),
                            "retry_secs": delay.as_secs(),
                        }),
                    );
                }
            }
            tokio::select! {
                () = stop.cancelled() => return,
                () = tokio::time::sleep(delay) => {}
            }
            delay = (delay * 2).min(std::time::Duration::from_secs(30));
        }
    });
}

async fn wait_for_shutdown() -> Result<(), ServiceError> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut terminate = signal(SignalKind::terminate()).map_err(ServiceError::Signal)?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result.map_err(ServiceError::Signal),
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await.map_err(ServiceError::Signal)
    }
}
