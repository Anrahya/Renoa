use std::{env, error::Error, io, path::Path, sync::Arc};

use renoa_agent::{AgentEvent, AgentEventSink, BoxFuture, ContentBlock};
use renoa_kernel::AgentId;
use renoa_local::{
    LocalHost, LocalHostAdapters, LocalModelConfiguration, LocalTurnOutcome, ModelProvider,
    ReasoningLevel,
};
use uuid::Uuid;

struct Quiet;

impl AgentEventSink for Quiet {
    fn emit(&self, _: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<String> = env::args().skip(1).collect();
    if arguments.len() < 4 {
        return Err(io::Error::other(
            "usage: renoa-local <host-data-directory> <workspace> <new|session-id> <prompt>",
        )
        .into());
    }
    let data_directory = Path::new(&arguments[0]);
    let workspace = Path::new(&arguments[1]);
    let prompt = arguments[3..].join(" ");
    let provider = ModelProvider::from_id(&required_environment("RENOA_MODEL_PROVIDER")?)
        .ok_or_else(|| io::Error::other("RENOA_MODEL_PROVIDER must be xai or opencode-go"))?;
    let models = LocalModelConfiguration::new(
        required_environment("RENOA_MODEL_BRIDGE")?,
        vec![provider],
        provider,
        required_environment("RENOA_MODEL")?,
        required_environment("RENOA_MODEL_AUTH_STORE")?,
    );
    let host = LocalHost::new(data_directory, models, LocalHostAdapters::new(None))?;
    let agent = AgentId::from_uuid(Uuid::parse_str(&required_environment("RENOA_AGENT_ID")?)?);
    if host.agent_definition(agent).await?.is_none() {
        return Err(io::Error::other(format!(
            "agent {agent} is not provisioned in this Host; provision it with `renoa-host <config.json> provision <provision.json>`"
        ))
        .into());
    }
    let session_uuid = match arguments[2].as_str() {
        "new" => Uuid::new_v4(),
        value => Uuid::parse_str(value)?,
    };
    let session = host
        .ensure_agent_session(agent, workspace, session_uuid)
        .await?;
    if let Some(reasoning) = optional_reasoning()? {
        session.set_reasoning(reasoning).await?;
    }
    let execution = session.execute_turn(
        Uuid::new_v4(),
        vec![ContentBlock::text(prompt)],
        Arc::new(Quiet),
    );
    tokio::pin!(execution);
    let outcome = tokio::select! {
        biased;
        result = &mut execution => result?,
        signal = tokio::signal::ctrl_c() => {
            signal?;
            session.cancel_active_turn()?;
            execution.await?
        }
    };

    println!("session_id={}", session.id());
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
