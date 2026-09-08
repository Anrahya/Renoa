//! The GitHub surface receives events and supervises disposable Host workers.
use renoa_local::{InspectionSandboxConfig, LocalHost};
use serde::Deserialize;
use std::{
    error::Error,
    fs::OpenOptions,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio_util::sync::CancellationToken;

#[path = "github_service/auth.rs"]
mod auth;
#[path = "github_service/dispatcher.rs"]
mod dispatcher;
#[path = "github_service/http.rs"]
mod http;
#[path = "github_service/recovery.rs"]
mod recovery;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    listen: SocketAddr,
    host_config: PathBuf,
    app_client_id: String,
    app_key_der: PathBuf,
    bot_login: String,
    webhook_secret: PathBuf,
    workspace: InspectionSandboxConfig,
}

pub async fn run(host: &LocalHost, data: &Path, path: &Path) -> Result<(), Box<dyn Error>> {
    let config: Settings = serde_json::from_slice(&tokio::fs::read(path).await?)?;
    if !config.listen.ip().is_loopback() {
        return Err("GitHub receiver must listen on loopback behind the HTTPS ingress".into());
    }
    // These paths are interpolated into systemd ExecStopPost, whose syntax is
    // deliberately narrower than arbitrary shell commands.
    for path in [&config.host_config, data] {
        if !path.is_absolute()
            || !path
                .as_os_str()
                .as_encoded_bytes()
                .iter()
                .all(|c| c.is_ascii_alphanumeric() || b"/._-".contains(c))
        {
            return Err(
                "service paths must be absolute ASCII paths without shell/systemd metacharacters"
                    .into(),
            );
        }
    }
    verify_worker_host(data, &config.host_config).await?;
    let owner = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(data.join(".github-service.lock"))?;
    owner.try_lock()?;
    let auth = auth::AppAuth::load(&config.app_client_id, &config.app_key_der)?;
    let secret = super::github_review::private_credential(&config.webhook_secret, 4096)?;
    let stop = CancellationToken::new();
    let state = Arc::new(http::State {
        host: host.clone(),
        secret,
        stop: stop.clone(),
    });
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    let server = axum::serve(listener, http::router(state))
        .with_graceful_shutdown(stop.clone().cancelled_owned());
    let server = std::future::IntoFuture::into_future(server);
    let dispatch = async {
        tokio::try_join!(
            dispatcher::run(host, data, &config, &auth, &stop),
            recovery::run(host, data, &auth, &stop),
        )?;
        Ok::<_, Box<dyn Error>>(())
    };
    tokio::pin!(server, dispatch);
    let mut termination =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    eprintln!("GitHub surface listening on {}", config.listen);
    let result = tokio::select! {
        result=&mut server=>{ stop.cancel(); dispatch.await?; result.map_err(Into::into) },
        result=&mut dispatch=>{ stop.cancel(); server.await?; result },
        result=tokio::signal::ctrl_c()=>{ result?; stop.cancel(); server.await?; dispatch.await },
        _=termination.recv()=>{ stop.cancel(); server.await?; dispatch.await },
    };
    owner.unlock()?;
    result
}

async fn verify_worker_host(data: &Path, config: &Path) -> Result<(), Box<dyn Error>> {
    let worker: super::Config = serde_json::from_slice(&tokio::fs::read(config).await?)?;
    if !worker.data_directory.is_absolute() {
        return Err("worker Host data directory must be absolute".into());
    }
    let supervisor = tokio::fs::canonicalize(data.join("host.sqlite3")).await?;
    let worker = tokio::fs::canonicalize(worker.data_directory.join("host.sqlite3")).await?;
    if supervisor != worker {
        return Err("worker Host database differs from the supervising Host database".into());
    }
    Ok(())
}
