use std::{env, error::Error, io, path::Path};

use renoa_agent::ContentBlock;
use renoa_kernel::{AgentId, CommandId, SessionId};
use renoa_local::{
    LocalRuntimeConfig, LocalSession, LocalTurnOutcome, LocalWorkspace, ReasoningLevel,
    build_local_runtime,
};
use tokio_util::sync::CancellationToken;
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
    let database = Path::new(&arguments[0]);
    let workspace = LocalWorkspace::open(&arguments[1])?;
    let prompt = arguments[3..].join(" ");
    let mut runtime_config = LocalRuntimeConfig::for_alpha(
        required_environment("RENOA_MODEL_BRIDGE")?,
        required_environment("RENOA_MODEL_PROVIDER")?,
        required_environment("RENOA_MODEL")?,
        required_environment("RENOA_MODEL_AUTH_STORE")?,
        &workspace,
    )?;
    if let Some(reasoning) = optional_reasoning()? {
        runtime_config = runtime_config.with_reasoning(reasoning);
    }
    let runtime = build_local_runtime(runtime_config, &workspace).await?;
    let session = open_session(database, &arguments[2])?;
    let cancellation = CancellationToken::new();
    let execution = session.execute_turn(
        CommandId::new(),
        vec![ContentBlock::text(prompt)],
        &runtime,
        cancellation.clone(),
    );
    tokio::pin!(execution);
    let outcome = tokio::select! {
        biased;
        result = &mut execution => result?,
        signal = tokio::signal::ctrl_c() => {
            signal?;
            cancellation.cancel();
            execution.await?
        }
    };

    println!("session_id={}", session.session_id());
    report(outcome)
}

fn report(outcome: LocalTurnOutcome) -> Result<(), Box<dyn Error>> {
    match outcome {
        LocalTurnOutcome::Completed { output, .. } => {
            println!("{output}");
            Ok(())
        }
        LocalTurnOutcome::Cancelled => Err(io::Error::other("operation was cancelled").into()),
        LocalTurnOutcome::Failed { reason } => Err(io::Error::other(reason).into()),
        LocalTurnOutcome::WaitingForInput => {
            Err(io::Error::other("operation is waiting for more input").into())
        }
        _ => Err(io::Error::other("the local Host returned an unsupported outcome").into()),
    }
}

fn open_session(database: &Path, value: &str) -> Result<LocalSession, Box<dyn Error>> {
    if value == "new" {
        let agent_id = AgentId::new();
        let session_id = SessionId::new();
        return Ok(LocalSession::create(database, agent_id, session_id)?);
    }
    let session_id = SessionId::from_uuid(Uuid::parse_str(value)?);
    Ok(LocalSession::load(database, session_id)?)
}

fn required_environment(name: &str) -> Result<String, Box<dyn Error>> {
    env::var(name)
        .map_err(|_| io::Error::other(format!("{name} must be set")))
        .map_err(Into::into)
}

fn optional_reasoning() -> Result<Option<ReasoningLevel>, Box<dyn Error>> {
    match env::var("RENOA_MODEL_REASONING") {
        Ok(value) => ReasoningLevel::from_id(&value).map(Some).ok_or_else(|| {
            io::Error::other(
                "RENOA_MODEL_REASONING must be off, minimal, low, medium, high, xhigh, or max",
            )
            .into()
        }),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => {
            Err(io::Error::other("RENOA_MODEL_REASONING must be valid UTF-8").into())
        }
    }
}
