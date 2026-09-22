//! Pinned, process-isolated Python evaluation for the MCP-only Code Mode tool.

mod value;

use std::{
    collections::HashSet,
    fmt::Write as _,
    fs::{self, File},
    io::{BufReader, Read as _},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use monty_pool::{Pool, PoolConfig, PoolError, ReplConfig, ResumeValue, TurnEvent, on_print_sync};
use monty_types::{
    CallArgs, DateTimeSource, ExcType, MontyException, MontyObject, NamedValues, OsPolicy,
    RandomSeed, RandomStart, ResourceLimits, SleepMode,
};
use renoa_agent_loop::{
    CodeMcpCall, CodeStep, CodeStepOutput, CodeStepRequest, MAX_CODE_CALLS_PER_WAVE,
    MAX_CODE_RESULT_BYTES, MAX_CODE_SNAPSHOT_BYTES,
};
use renoa_kernel::{
    EffectAdapter, EffectCompletion, EffectFuture, EffectInvocation, EffectOutcome,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::sync::OnceCell;

use self::value::{from_json, mcp_call, to_json};

const MONTY_VERSION: &str = "1.0.0-beta.2";
const MONTY_BINARY_SHA256: &str =
    "f596526655da1026bfbd928e4fa26bdbbe461e3130a351cea77208acd2bae140";

/// Preflights the exact worker without starting a process or opening Host state.
///
/// # Errors
///
/// Refuses a missing, linked, non-absolute, or differently hashed binary.
pub fn validate_code_mode_worker(path: &Path) -> Result<(), String> {
    MontyEvaluator::new(path).map(|_| ())
}

/// One exact worker binary and one lazily initialized, shared subprocess pool.
pub(crate) struct MontyEvaluator {
    binary: PathBuf,
    pool: OnceCell<Arc<Pool>>,
}

impl MontyEvaluator {
    pub(crate) fn new(binary: &Path) -> Result<Self, String> {
        if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            return Err("the pinned Monty worker supports Linux x86-64 only".to_owned());
        }
        if !binary.is_absolute() {
            return Err("Monty worker path must be absolute".to_owned());
        }
        let metadata = fs::symlink_metadata(binary)
            .map_err(|error| format!("Monty worker is unavailable: {error}"))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err("Monty worker must be a regular file, not a symlink".to_owned());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            if metadata.permissions().mode() & 0o111 == 0 {
                return Err("Monty worker is not executable".to_owned());
            }
        }
        let resolved = fs::canonicalize(binary)
            .map_err(|error| format!("Monty worker path is invalid: {error}"))?;
        let file = File::open(&resolved)
            .map_err(|error| format!("Monty worker cannot be read: {error}"))?;
        let mut reader = BufReader::new(file);
        let mut digest = Sha256::new();
        let mut buffer = [0_u8; 8 * 1024];
        loop {
            let count = reader
                .read(&mut buffer)
                .map_err(|error| format!("Monty worker hash failed: {error}"))?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        let mut actual = String::with_capacity(64);
        for byte in digest.finalize() {
            write!(&mut actual, "{byte:02x}").expect("writing to a String cannot fail");
        }
        if actual != MONTY_BINARY_SHA256 {
            return Err(format!(
                "Monty worker hash mismatch: expected {MONTY_BINARY_SHA256}, found {actual}"
            ));
        }
        Ok(Self {
            binary: resolved,
            pool: OnceCell::new(),
        })
    }

    pub(crate) fn revision() -> String {
        format!("monty-pool/{MONTY_VERSION}/binary-{MONTY_BINARY_SHA256}/limits-v1")
    }

    async fn pool(&self) -> Result<&Arc<Pool>, String> {
        self.pool
            .get_or_try_init(|| async {
                let mut config = PoolConfig::subprocess(self.binary.clone());
                config.min_processes = 0;
                config.max_processes = 2;
                config.checkout_timeout = Some(Duration::from_secs(15));
                config.request_timeout = Some(Duration::from_secs(5));
                config.max_checkouts_per_worker = Some(100);
                Pool::new(config)
                    .await
                    .map(Arc::new)
                    .map_err(|error| error.to_string())
            })
            .await
    }

    async fn evaluate(&self, request: CodeStepRequest) -> Result<CodeStepOutput, EvaluationError> {
        let pool = self.pool().await.map_err(EvaluationError::Infrastructure)?;
        let repl = ReplConfig {
            script_name: "code_mode.py".to_owned(),
            limits: Some(
                ResourceLimits::default()
                    .max_feed_duration(Duration::from_secs(2))
                    .max_turn_duration(Duration::from_secs(1))
                    .max_memory(64 * 1024 * 1024)
                    .max_recursion_depth(128)
                    .max_suspensions(256),
            ),
            os_policy: OsPolicy {
                datetime: DateTimeSource::Fixed {
                    unix_seconds: 0,
                    microsecond: 0,
                },
                sleep: SleepMode::Zero,
                random_start: RandomStart::Seed(RandomSeed::Str(request.run_id.clone())),
                ..OsPolicy::default()
            },
            ..ReplConfig::default()
        };
        let mut checkout = pool.checkout(&repl).await?;
        let mut on_print = on_print_sync(|_, _| {});
        let event = match request.step {
            CodeStep::Start { source } => {
                checkout
                    .feed(source, NamedValues::new(), Vec::new(), false, &mut on_print)
                    .await?
            }
            CodeStep::Resume { snapshot, results } => {
                if snapshot.is_empty() || snapshot.len() > MAX_CODE_SNAPSHOT_BYTES {
                    return Err(EvaluationError::Infrastructure(
                        "persisted Monty snapshot exceeds its limit".to_owned(),
                    ));
                }
                let state = BASE64.decode(snapshot.as_bytes()).map_err(|error| {
                    EvaluationError::Infrastructure(format!(
                        "persisted Monty snapshot is invalid: {error}"
                    ))
                })?;
                let (restored, _) = checkout.restore(state, Vec::new(), &mut on_print).await?;
                let Some(TurnEvent::ResolveFutures { pending_call_ids }) = restored else {
                    return Err(EvaluationError::Infrastructure(
                        "persisted Monty snapshot is not awaiting MCP futures".to_owned(),
                    ));
                };
                let pending: HashSet<u32> = pending_call_ids.into_iter().collect();
                let resolved = results
                    .into_iter()
                    .map(|(key, value)| {
                        let id = key.parse::<u32>().map_err(|_| {
                            EvaluationError::Infrastructure(
                                "persisted Monty call ID is invalid".to_owned(),
                            )
                        })?;
                        if id.to_string() != key {
                            return Err(EvaluationError::Infrastructure(
                                "persisted Monty call ID is not canonical".to_owned(),
                            ));
                        }
                        let object = from_json(&value, 0).map_err(EvaluationError::User)?;
                        Ok((id, ResumeValue::Return(object)))
                    })
                    .collect::<Result<Vec<_>, EvaluationError>>()?;
                if pending.len() != resolved.len()
                    || !resolved.iter().all(|(id, _)| pending.contains(id))
                {
                    return Err(EvaluationError::Infrastructure(
                        "Monty snapshot pending calls differ from durable MCP results".to_owned(),
                    ));
                }
                checkout.resume_futures(resolved, &mut on_print).await?
            }
        };
        let output = drive(&mut checkout, event, &mut on_print).await?;
        checkout.finish().await?;
        Ok(output)
    }
}

impl EffectAdapter for MontyEvaluator {
    fn invoke(&self, invocation: EffectInvocation) -> EffectFuture<'_> {
        Box::pin(async move {
            let request = match serde_json::from_value::<CodeStepRequest>(invocation.request) {
                Ok(request) => request,
                Err(error) => {
                    return EffectOutcome::Failure {
                        message: format!("invalid Code Mode step request: {error}"),
                    }
                    .into();
                }
            };
            let result = tokio::select! {
                biased;
                () = invocation.cancellation.cancelled() => Err(EvaluationError::Infrastructure("Code Mode evaluation was cancelled".to_owned())),
                result = self.evaluate(request) => result,
            };
            let output = match result {
                Ok(output) => output,
                Err(EvaluationError::User(message)) => CodeStepOutput::Completed {
                    result: json!({"error": message}),
                    is_error: true,
                },
                Err(EvaluationError::Retryable) => return EffectCompletion::OutcomeUnknown,
                Err(EvaluationError::Infrastructure(message)) => {
                    return EffectOutcome::Failure { message }.into();
                }
            };
            match serde_json::to_value(output) {
                Ok(value) => EffectOutcome::Success(value).into(),
                Err(error) => EffectOutcome::Failure {
                    message: format!("Code Mode output encoding failed: {error}"),
                }
                .into(),
            }
        })
    }
}

#[derive(Debug)]
enum EvaluationError {
    User(String),
    Retryable,
    Infrastructure(String),
}

impl From<PoolError> for EvaluationError {
    fn from(error: PoolError) -> Self {
        match error {
            PoolError::Runtime(_) | PoolError::Typing(_) => Self::User(error.to_string()),
            PoolError::Crashed { .. }
            | PoolError::Timeout { .. }
            | PoolError::Disconnected { .. }
            | PoolError::Shutdown { .. } => Self::Retryable,
            _ => Self::Infrastructure(error.to_string()),
        }
    }
}

async fn drive(
    checkout: &mut monty_pool::Checkout,
    mut event: TurnEvent,
    on_print: &mut (impl FnMut(monty_types::PrintStream, &str) -> monty_pool::PrintFuture + Send),
) -> Result<CodeStepOutput, EvaluationError> {
    let mut calls: Vec<CodeMcpCall> = Vec::new();
    loop {
        event = match event {
            TurnEvent::NameLookup {
                name,
                object_id: None,
            } if name == "mcp" => {
                checkout
                    .resume_name_lookup(
                        MontyObject::function(
                            "mcp",
                            Some(
                                "Call an attached MCP tool by exact reference and JSON arguments."
                                    .to_owned(),
                            ),
                        ),
                        on_print,
                    )
                    .await?
            }
            TurnEvent::NameLookup { .. } => {
                checkout
                    .resume_name_lookup(None::<MontyObject>, on_print)
                    .await?
            }
            TurnEvent::FunctionCall {
                function_name,
                args,
                call_id,
                object_id,
                ..
            } if function_name == "mcp" && object_id.is_none() => {
                resume_mcp_call(checkout, call_id, &args, &mut calls, on_print).await?
            }
            TurnEvent::FunctionCall { .. } => {
                checkout.resume(ResumeValue::NotFound, on_print).await?
            }
            TurnEvent::OsCall { .. } => checkout.resume(ResumeValue::NotHandled, on_print).await?,
            TurnEvent::ResolveFutures { pending_call_ids } => {
                let pending: HashSet<u32> = pending_call_ids.into_iter().collect();
                let known: HashSet<u32> = calls.iter().map(|call| call.call_id).collect();
                if pending.is_empty() || pending != known {
                    return Err(EvaluationError::Infrastructure(
                        "Monty futures differ from the declared MCP call wave".to_owned(),
                    ));
                }
                let snapshot = BASE64.encode(checkout.dump().await?);
                if snapshot.len() > MAX_CODE_SNAPSHOT_BYTES {
                    return Err(EvaluationError::User(
                        "Code Mode snapshot exceeds 2 MiB".to_owned(),
                    ));
                }
                return Ok(CodeStepOutput::Suspended { snapshot, calls });
            }
            TurnEvent::Complete(result) => {
                if !calls.is_empty() {
                    return Err(EvaluationError::User(
                        "Code Mode MCP calls must be awaited before Python returns".to_owned(),
                    ));
                }
                let result = to_json(result.as_ref(), 0).map_err(EvaluationError::User)?;
                let encoded = serde_json::to_vec(&result)
                    .map_err(|error| EvaluationError::Infrastructure(error.to_string()))?;
                if encoded.len() > MAX_CODE_RESULT_BYTES {
                    return Err(EvaluationError::User(
                        "Code Mode final result exceeds 1 MiB".to_owned(),
                    ));
                }
                return Ok(CodeStepOutput::Completed {
                    result,
                    is_error: false,
                });
            }
        };
    }
}

async fn resume_mcp_call(
    checkout: &mut monty_pool::Checkout,
    call_id: u32,
    args: &CallArgs,
    calls: &mut Vec<CodeMcpCall>,
    on_print: &mut (impl FnMut(monty_types::PrintStream, &str) -> monty_pool::PrintFuture + Send),
) -> Result<TurnEvent, EvaluationError> {
    let response = match mcp_call(call_id, args) {
        Ok(call)
            if calls.len() < MAX_CODE_CALLS_PER_WAVE
                && !calls.iter().any(|existing| existing.call_id == call_id) =>
        {
            calls.push(call);
            ResumeValue::Future
        }
        Ok(_) => ResumeValue::Error(MontyException::new(
            ExcType::RuntimeError,
            Some("Code Mode MCP wave limit or duplicate identity".to_owned()),
        )),
        Err(message) => ResumeValue::Error(MontyException::new(ExcType::TypeError, Some(message))),
    };
    checkout
        .resume(response, on_print)
        .await
        .map_err(Into::into)
}

#[cfg(test)]
mod tests;
