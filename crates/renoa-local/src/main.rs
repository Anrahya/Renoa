use std::{env, error::Error, io, path::PathBuf};

use renoa_agent::{AssistantContent, Message};
use renoa_agent_loop::{AgentCommand, MESSAGE_EVENT_KIND};
use renoa_kernel::{
    AgentId, CancellationId, Command, CommandId, DriveResult, EventCursor, Kernel, OperationId,
    OperationOutcome, SessionId,
};
use renoa_local::{LocalRuntimeConfig, LocalWorkspace, build_local_runtime};
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
    let prompt = arguments[3..].join(" ");
    let runtime = build_local_runtime(
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
    let kernel = Kernel::open(database)?;
    let session_id = open_session(&kernel, &arguments[2])?;
    let command = serde_json::to_value(AgentCommand::text(prompt))?;
    let admission = kernel.submit(session_id, Command::new(CommandId::new(), command))?;
    let execution = kernel.drive(session_id, &runtime);
    tokio::pin!(execution);
    let result = tokio::select! {
        biased;
        result = &mut execution => result?,
        signal = tokio::signal::ctrl_c() => {
            signal?;
            kernel.request_cancellation(
                session_id,
                admission.operation_id,
                CancellationId::new(),
            )?;
            execution.await?
        }
    };

    println!("session_id={session_id}");
    report(&kernel, session_id, result)
}

fn report(
    kernel: &Kernel,
    session_id: SessionId,
    result: DriveResult,
) -> Result<(), Box<dyn Error>> {
    match result {
        DriveResult::Finished {
            operation_id,
            outcome: OperationOutcome::Completed,
        } => {
            println!("{}", completed_output(kernel, session_id, operation_id)?);
            Ok(())
        }
        DriveResult::Finished {
            outcome: OperationOutcome::Cancelled,
            ..
        } => Err(io::Error::other("operation was cancelled").into()),
        DriveResult::Finished {
            outcome: OperationOutcome::Failed { reason },
            ..
        } => Err(io::Error::other(reason).into()),
        DriveResult::Finished {
            outcome: OperationOutcome::WaitingForInput,
            ..
        } => Err(io::Error::other("operation is waiting for more input").into()),
        DriveResult::Blocked { operation_id } => Err(io::Error::other(format!(
            "operation {operation_id} is blocked on an uncertain tool outcome"
        ))
        .into()),
        DriveResult::Idle => Err(io::Error::other("the session had no runnable operation").into()),
        _ => Err(io::Error::other("the kernel returned an unsupported outcome").into()),
    }
}

fn open_session(kernel: &Kernel, value: &str) -> Result<SessionId, Box<dyn Error>> {
    if value == "new" {
        let agent_id = AgentId::new();
        let session_id = SessionId::new();
        kernel.create_agent(agent_id)?;
        kernel.create_session(session_id, agent_id)?;
        return Ok(session_id);
    }
    let session_id = SessionId::from_uuid(Uuid::parse_str(value)?);
    kernel.inspect(session_id)?;
    Ok(session_id)
}

fn completed_output(
    kernel: &Kernel,
    session_id: SessionId,
    operation_id: OperationId,
) -> Result<String, Box<dyn Error>> {
    let events = kernel.events_after(session_id, EventCursor::START)?.events;
    let event = events
        .iter()
        .rev()
        .find(|event| event.operation_id == operation_id && event.kind == MESSAGE_EVENT_KIND)
        .ok_or_else(|| io::Error::other("completed operation has no message event"))?;
    let message = serde_json::from_value::<Message>(event.payload.clone())?;
    let Message::Assistant { content, .. } = message else {
        return Err(io::Error::other("completed operation has no assistant message").into());
    };
    Ok(content
        .into_iter()
        .filter_map(|block| match block {
            AssistantContent::Text { text, .. } => Some(text),
            AssistantContent::Reasoning { .. } | AssistantContent::ToolCall { .. } => None,
        })
        .collect())
}

fn required_environment(name: &str) -> Result<String, Box<dyn Error>> {
    env::var(name)
        .map_err(|_| io::Error::other(format!("{name} must be set")))
        .map_err(Into::into)
}
