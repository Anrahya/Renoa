use std::{env, error::Error, io, path::PathBuf, sync::Arc};

use renoa_agent::ContentBlock;
use renoa_harness::{
    CancellationId, Harness, OperationOutcome, OperationRequest, RequestId, RunNext, SessionId,
};
use renoa_local::{LocalRuntimeConfig, LocalWorkspace, build_local_profile};
use uuid::Uuid;

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    if arguments.len() < 4 {
        return Err(io::Error::other(
            "usage: renoa-local <database> <workspace> <new|session-id> <prompt>",
        )
        .into());
    }
    let database = PathBuf::from(&arguments[0]);
    let workspace = LocalWorkspace::open(&arguments[1])?;
    let session_id = session_id(&arguments[2])?;
    let prompt = arguments[3..].join(" ");
    let profile = build_local_profile(
        LocalRuntimeConfig::new(
            required_environment("RENOA_PI_BRIDGE")?,
            required_environment("RENOA_PI_PROVIDER")?,
            required_environment("RENOA_PI_MODEL")?,
            required_environment("RENOA_PI_AUTH_STORE")?,
            required_environment("RENOA_PI_INSTRUCTIONS")?,
        ),
        &workspace,
    )
    .await?;
    let harness = Arc::new(Harness::open(database)?);
    harness.create_standalone_session(session_id).await?;
    let admission = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text(prompt)]),
        )
        .await?;
    let execution = harness.run_next(session_id, &profile);
    tokio::pin!(execution);
    let result = tokio::select! {
        biased;
        result = &mut execution => result?,
        signal = tokio::signal::ctrl_c() => {
            signal?;
            harness.request_standalone_cancellation(
                session_id,
                admission.operation_id,
                CancellationId::new(),
            ).await?;
            execution.await?
        }
    };

    println!("session_id={session_id}");
    report(result)
}

fn report(result: RunNext) -> Result<(), Box<dyn Error>> {
    match result {
        RunNext::Finished {
            outcome:
                OperationOutcome::Completed {
                    output,
                    stop_reason: _,
                    usage: _,
                },
            ..
        } => {
            println!("{output}");
            Ok(())
        }
        RunNext::Finished {
            outcome: OperationOutcome::Cancelled { message },
            ..
        }
        | RunNext::Finished {
            outcome: OperationOutcome::Failed { message },
            ..
        } => Err(io::Error::other(message).into()),
        RunNext::Blocked { operation_id } => Err(io::Error::other(format!(
            "operation {operation_id} is blocked on an uncertain tool outcome"
        ))
        .into()),
        RunNext::Idle => Err(io::Error::other("the session had no runnable operation").into()),
        _ => Err(io::Error::other("the harness returned an unsupported outcome").into()),
    }
}

fn session_id(value: &str) -> Result<SessionId, Box<dyn Error>> {
    if value == "new" {
        return Ok(SessionId::new());
    }
    Ok(SessionId::from_uuid(Uuid::parse_str(value)?))
}

fn required_environment(name: &str) -> Result<String, Box<dyn Error>> {
    env::var(name)
        .map_err(|_| io::Error::other(format!("{name} must be set")))
        .map_err(Into::into)
}
