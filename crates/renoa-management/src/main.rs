use std::{net::SocketAddr, path::PathBuf};

use renoa_management::ManagementApi;
use renoa_protocol::PrincipalId;
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    data_directory: PathBuf,
    assets_directory: PathBuf,
    host_id: Uuid,
    identity_address: SocketAddr,
    owner_principal_id: PrincipalId,
    public_origin: String,
    listen: SocketAddr,
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 1 {
        return Err("usage: renoa-management <config.json>".into());
    }
    let config: Config = serde_json::from_slice(&std::fs::read(&args[0])?)?;
    if !config.data_directory.is_absolute() || !config.assets_directory.is_absolute() {
        return Err("management storage and asset paths must be absolute".into());
    }
    let api = ManagementApi::open(
        &config.data_directory,
        config.host_id,
        config.identity_address,
        config.owner_principal_id,
        &config.public_origin,
    )?
    .with_assets(&config.assets_directory)?;
    let listener = TcpListener::bind(config.listen).await?;
    let shutdown = CancellationToken::new();
    let serving = api.serve(listener, shutdown.clone());
    tokio::pin!(serving);
    tokio::select! {
        result=&mut serving=>result?,
        signal=stop_signal()=> {
            signal?;
            shutdown.cancel();
            serving.await?;
        }
    }
    Ok(())
}

async fn stop_signal() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await
}
