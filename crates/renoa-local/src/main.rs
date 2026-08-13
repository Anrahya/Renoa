use std::{
    env,
    error::Error,
    io,
    num::{NonZeroU32, NonZeroU64},
    path::PathBuf,
    sync::Arc,
};

use renoa_agent::ContentBlock;
use renoa_harness::{
    CancellationId, CompactionPolicy, Harness, OperationOutcome, OperationRequest, RequestId,
    RunNext, RuntimeProfile, SessionId,
};
use renoa_local::{LocalWorkspace, PiModel};
use uuid::Uuid;

const MODEL_ATTEMPT_LIMIT: u32 = 32;
const TOOL_CALL_LIMIT: u32 = 16;
const MAX_OUTPUT_TOKENS: u32 = 32_768;
const COMPACTION_ATTEMPT_LIMIT: u32 = 2;
const MAX_CHECKPOINT_TOKENS: u64 = 16_384;
const MIN_CONTEXT_SAFETY_TOKENS: u64 = 8_192;

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
    let provider = required_environment("RENOA_PI_PROVIDER")?;
    let model_id = required_environment("RENOA_PI_MODEL")?;
    let model = Arc::new(
        PiModel::load(
            required_environment("RENOA_PI_BRIDGE")?,
            &provider,
            &model_id,
            required_environment("RENOA_PI_AUTH_STORE")?,
            NonZeroU32::new(MAX_OUTPUT_TOKENS).expect("output limit is non-zero"),
        )
        .await?,
    );
    let compaction = compaction_policy(&model)?;
    let profile = RuntimeProfile::new(
        format!(
            "pi/{provider}/{model_id}/{}/local-tools-compaction-v1",
            model.binding_id()
        ),
        model.clone(),
        required_environment("RENOA_PI_INSTRUCTIONS")?,
        NonZeroU32::new(MODEL_ATTEMPT_LIMIT).expect("model limit is non-zero"),
    )
    .with_tools(
        workspace.tool_bindings(),
        NonZeroU32::new(TOOL_CALL_LIMIT).expect("tool limit is non-zero"),
    )?
    .with_compaction(compaction, model);
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

fn compaction_policy(model: &PiModel) -> Result<CompactionPolicy, Box<dyn Error>> {
    let context = model.context_window_tokens();
    let safety = (context.get() / 50).max(MIN_CONTEXT_SAFETY_TOKENS);
    let reserved = u64::from(model.max_output_tokens().get())
        .checked_add(safety)
        .ok_or_else(|| io::Error::other("context reserve overflowed u64"))?;
    let dispatch = context
        .get()
        .checked_sub(reserved)
        .ok_or_else(|| io::Error::other("model context is smaller than its output reserve"))?;
    let target = dispatch
        .checked_mul(3)
        .and_then(|value| value.checked_div(5))
        .and_then(NonZeroU64::new)
        .ok_or_else(|| io::Error::other("post-compaction target is zero"))?;
    let max_summary = NonZeroU64::new(MAX_CHECKPOINT_TOKENS.min(target.get() / 4))
        .ok_or_else(|| io::Error::other("checkpoint budget is zero"))?;
    Ok(CompactionPolicy::new(
        context,
        reserved,
        target,
        max_summary,
        NonZeroU32::new(COMPACTION_ATTEMPT_LIMIT).expect("compaction attempt limit is non-zero"),
    )?)
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
