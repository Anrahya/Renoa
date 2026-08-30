use std::{
    env,
    ffi::OsString,
    io::{self, Write as _},
    net::Ipv4Addr,
    path::PathBuf,
    process::ExitCode,
};

use renoa_registry::Registry;
use serde::Serialize;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

const USAGE: &str = "usage: renoa-registry serve <state-directory> <port>";

#[derive(Serialize)]
struct Ready {
    endpoint: String,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("renoa-registry: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let (state, port) = parse(env::args_os())?;
    let registry = Registry::open(state).map_err(|error| error.to_string())?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
        .await
        .map_err(|error| format!("failed to bind loopback port {port}: {error}"))?;
    let address = listener
        .local_addr()
        .map_err(|error| format!("failed to read listener address: {error}"))?;
    write_json(&Ready {
        endpoint: format!("http://{address}/v1"),
    })?;
    let shutdown = CancellationToken::new();
    let server = registry.serve(listener, shutdown.clone());
    tokio::pin!(server);
    tokio::select! {
        result = &mut server => result.map_err(|error| error.to_string()),
        result = shutdown_signal() => {
            result.map_err(|error| format!("failed to listen for shutdown: {error}"))?;
            shutdown.cancel();
            server.await.map_err(|error| error.to_string())
        }
    }
}

fn parse(mut arguments: impl Iterator<Item = OsString>) -> Result<(PathBuf, u16), String> {
    let _program = arguments.next();
    if arguments.next().as_deref() != Some(std::ffi::OsStr::new("serve")) {
        return Err(USAGE.to_owned());
    }
    let state = arguments
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| USAGE.to_owned())?;
    let port = arguments
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or_else(|| USAGE.to_owned())?
        .parse::<u16>()
        .map_err(|_| "port must be an integer from 0 through 65535".to_owned())?;
    if arguments.next().is_some() {
        return Err(USAGE.to_owned());
    }
    Ok((state, port))
}

fn write_json(value: &impl Serialize) -> Result<(), String> {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, value)
        .map_err(|error| format!("failed to serialize output: {error}"))?;
    writeln!(stdout).map_err(|error| format!("failed to write output: {error}"))?;
    stdout
        .flush()
        .map_err(|error| format!("failed to flush output: {error}"))
}

#[cfg(unix)]
async fn shutdown_signal() -> io::Result<()> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result,
        _ = terminate.recv() => Ok(()),
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() -> io::Result<()> {
    tokio::signal::ctrl_c().await
}
