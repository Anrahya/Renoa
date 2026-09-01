use std::{env, ffi::OsString, path::PathBuf, process::ExitCode};

mod config;
mod enrollment;
mod error;
mod log;
mod private_file;
mod service;

use error::ServiceError;

const USAGE: &str = "usage:
  renoa-node serve <config-file> <device-credentials-file> <state-directory>
  renoa-node enroll <coordinator-endpoint> <enrollment-file> <device-credentials-output>";

enum Operation {
    Serve {
        config: PathBuf,
        credentials: PathBuf,
        state_directory: PathBuf,
    },
    Enroll {
        endpoint: String,
        enrollment: PathBuf,
        output: PathBuf,
    },
}

impl Operation {
    fn parse(mut arguments: impl Iterator<Item = OsString>) -> Result<Self, ServiceError> {
        let _program = arguments.next();
        let operation = arguments
            .next()
            .and_then(|value| value.into_string().ok())
            .ok_or_else(usage)?;
        let parsed = match operation.as_str() {
            "serve" => Self::Serve {
                config: path_argument(&mut arguments)?,
                credentials: path_argument(&mut arguments)?,
                state_directory: path_argument(&mut arguments)?,
            },
            "enroll" => Self::Enroll {
                endpoint: string_argument(&mut arguments)?,
                enrollment: path_argument(&mut arguments)?,
                output: path_argument(&mut arguments)?,
            },
            _ => return Err(usage()),
        };
        if arguments.next().is_some() {
            return Err(usage());
        }
        Ok(parsed)
    }
}

fn path_argument(arguments: &mut impl Iterator<Item = OsString>) -> Result<PathBuf, ServiceError> {
    arguments.next().map(PathBuf::from).ok_or_else(usage)
}

fn string_argument(arguments: &mut impl Iterator<Item = OsString>) -> Result<String, ServiceError> {
    arguments
        .next()
        .and_then(|value| value.into_string().ok())
        .filter(|value| !value.is_empty())
        .ok_or_else(usage)
}

fn usage() -> ServiceError {
    ServiceError::Configuration(USAGE.to_owned())
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            log::event(
                "error",
                "command_failed",
                &serde_json::json!({"error": error.to_string()}),
            );
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), ServiceError> {
    match Operation::parse(env::args_os())? {
        Operation::Serve {
            config,
            credentials,
            state_directory,
        } => service::serve(&config, &credentials, &state_directory).await,
        Operation::Enroll {
            endpoint,
            enrollment,
            output,
        } => {
            enrollment::enroll(&endpoint, &enrollment, &output).await?;
            println!("{{\"status\":\"enrolled\"}}");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_line_is_exact() {
        let serve = Operation::parse(
            ["renoa-node", "serve", "/a", "/b", "/c"]
                .into_iter()
                .map(OsString::from),
        )
        .expect("parse serve");
        assert!(matches!(serve, Operation::Serve { .. }));
        assert!(
            Operation::parse(
                ["renoa-node", "serve", "/a", "/b"]
                    .into_iter()
                    .map(OsString::from)
            )
            .is_err()
        );
        assert!(
            Operation::parse(
                [
                    "renoa-node",
                    "enroll",
                    "ws://localhost",
                    "/a",
                    "/b",
                    "extra"
                ]
                .into_iter()
                .map(OsString::from)
            )
            .is_err()
        );
    }
}
