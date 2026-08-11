use std::{
    env,
    ffi::OsString,
    io::{self, Write},
    net::Ipv4Addr,
    path::PathBuf,
    process::ExitCode,
    time::{Duration, SystemTime},
};

use renoa_control::{Coordinator, EnrollmentToken, PeerIdentity};
use renoa_protocol::{PrincipalId, SurfaceRef};
use serde::Serialize;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const USAGE: &str = "usage:
  renoa-coordinator serve <database-path> <port>
  renoa-coordinator enroll-surface <database-path> <principal-id> <surface>";
const ENROLLMENT_LIFETIME: Duration = Duration::from_mins(5);

enum Operation {
    Serve {
        database: PathBuf,
        port: u16,
    },
    EnrollSurface {
        database: PathBuf,
        principal_id: PrincipalId,
        surface: SurfaceRef,
    },
}

impl Operation {
    fn parse(mut arguments: impl Iterator<Item = OsString>) -> Result<Self, String> {
        let _program = arguments.next();
        let operation = arguments.next().ok_or_else(|| USAGE.to_owned())?;
        let database = arguments
            .next()
            .map(PathBuf::from)
            .ok_or_else(|| USAGE.to_owned())?;

        match operation.to_str() {
            Some("serve") => {
                let port = arguments
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .ok_or_else(|| USAGE.to_owned())?
                    .parse::<u16>()
                    .map_err(|_| "port must be an integer from 0 through 65535".to_owned())?;
                no_more_arguments(arguments)?;
                Ok(Self::Serve { database, port })
            }
            Some("enroll-surface") => {
                let principal_id = arguments
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .ok_or_else(|| USAGE.to_owned())?
                    .parse::<Uuid>()
                    .map(PrincipalId::from_uuid)
                    .map_err(|_| "principal id must be a UUID".to_owned())?;
                let surface = arguments
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .map(SurfaceRef::new)
                    .ok_or_else(|| USAGE.to_owned())?;
                no_more_arguments(arguments)?;
                Ok(Self::EnrollSurface {
                    database,
                    principal_id,
                    surface,
                })
            }
            _ => Err(USAGE.to_owned()),
        }
    }
}

fn no_more_arguments(mut arguments: impl Iterator<Item = OsString>) -> Result<(), String> {
    if arguments.next().is_some() {
        return Err(USAGE.to_owned());
    }
    Ok(())
}

#[derive(Serialize)]
struct Ready {
    endpoint: String,
}

#[derive(Serialize)]
struct EnrollmentCreated {
    token: EnrollmentToken,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("renoa-coordinator: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    match Operation::parse(env::args_os())? {
        Operation::Serve { database, port } => serve(database, port).await,
        Operation::EnrollSurface {
            database,
            principal_id,
            surface,
        } => create_surface_enrollment(database, principal_id, surface).await,
    }
}

async fn serve(database: PathBuf, port: u16) -> Result<(), String> {
    let coordinator = Coordinator::open(database).map_err(|error| error.to_string())?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
        .await
        .map_err(|error| format!("failed to bind loopback port {port}: {error}"))?;
    let address = listener
        .local_addr()
        .map_err(|error| format!("failed to read listener address: {error}"))?;
    write_json(&Ready {
        endpoint: format!("ws://{address}/connect"),
    })?;

    let shutdown = CancellationToken::new();
    let server = coordinator.serve(listener, shutdown.clone());
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

async fn create_surface_enrollment(
    database: PathBuf,
    principal_id: PrincipalId,
    surface: SurfaceRef,
) -> Result<(), String> {
    let coordinator = Coordinator::open(database).map_err(|error| error.to_string())?;
    let token = coordinator
        .create_enrollment(
            PeerIdentity::Surface {
                principal_id,
                surface,
            },
            SystemTime::now() + ENROLLMENT_LIFETIME,
        )
        .await
        .map_err(|error| error.to_string())?;
    write_json(&EnrollmentCreated { token })
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
