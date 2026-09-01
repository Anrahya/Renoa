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
    let target_count = config.targets.len();
    let device_id = config.credentials.device_id.to_string();
    let node = RenoaNode::open(
        config.endpoint,
        config.credentials,
        config.state_directory.join("node.sqlite"),
        config.host,
        config.targets,
    )?;
    let shutdown = CancellationToken::new();
    let signal = wait_for_shutdown();
    tokio::pin!(signal);
    log::event(
        "info",
        "service_started",
        &serde_json::json!({
            "device_id": device_id,
            "target_count": target_count,
        }),
    );
    let run = node.run(shutdown.clone());
    tokio::pin!(run);
    let result = tokio::select! {
        result = &mut run => result.map_err(ServiceError::from),
        result = &mut signal => {
            result?;
            shutdown.cancel();
            run.await.map_err(ServiceError::from)
        }
    };
    if result.is_ok() {
        log::event("info", "service_stopped", &serde_json::json!({}));
    }
    result
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
