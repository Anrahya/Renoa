use std::{
    io,
    num::{NonZeroU32, NonZeroU64},
    path::PathBuf,
};

use futures_util::{StreamExt, stream};
use renoa_agent::{Model, ModelError, ModelEvent, ModelEventStream, ModelRequest, ModelResponse};
use renoa_harness::ContextSizer;
use serde::{Deserialize, de::DeserializeOwned};
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

const OUTPUT_LIMIT: usize = 16 * 1_024 * 1_024;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PiModelConfigError {
    #[error("Pi model bridge cannot be resolved: {0}")]
    Bridge(#[source] io::Error),
    #[error("Pi credential store cannot be resolved: {0}")]
    CredentialStore(#[source] io::Error),
    #[error("Pi model bridge is not a file: {0}")]
    BridgeNotFile(PathBuf),
    #[error("Pi credential store is not a file: {0}")]
    CredentialStoreNotFile(PathBuf),
    #[error("unsupported Pi provider: {0}")]
    UnsupportedProvider(String),
    #[error("Pi model id must not be empty")]
    EmptyModel,
    #[error("Pi model bridge could not resolve the selected model: {0}")]
    ModelResolution(#[source] ModelError),
    #[error("Pi model reported a zero-token context window")]
    ZeroContextWindow,
    #[error("Pi model reported a zero-token output limit")]
    ZeroProviderOutputLimit,
}

/// A provider adapter that invokes Pi AI for one exact Renoa model request.
pub struct PiModel {
    config: PiBridgeConfig,
    context_window_tokens: NonZeroU64,
    max_output_tokens: NonZeroU32,
}

#[derive(Clone)]
struct PiBridgeConfig {
    bridge: PathBuf,
    provider: String,
    model: String,
    credential_store: PathBuf,
}

impl PiModel {
    /// Resolves and validates the selected Pi model before accepting work.
    ///
    /// # Errors
    ///
    /// Returns an error when configuration, credentials, or model limits are invalid.
    pub async fn load(
        bridge: impl Into<PathBuf>,
        provider: impl Into<String>,
        model: impl Into<String>,
        credential_store: impl Into<PathBuf>,
        max_output_tokens: NonZeroU32,
    ) -> Result<Self, PiModelConfigError> {
        let config = PiBridgeConfig::new(bridge, provider, model, credential_store)?;
        let description = describe_bridge(config.clone())
            .await
            .map_err(PiModelConfigError::ModelResolution)?;
        let context_window_tokens = NonZeroU64::new(description.context_window_tokens)
            .ok_or(PiModelConfigError::ZeroContextWindow)?;
        let provider_output_limit = NonZeroU64::new(description.max_output_tokens)
            .ok_or(PiModelConfigError::ZeroProviderOutputLimit)?;
        let max_output_tokens = NonZeroU32::try_from(provider_output_limit)
            .map_or(max_output_tokens, |provider_output_limit| {
                max_output_tokens.min(provider_output_limit)
            });
        Ok(Self {
            config,
            context_window_tokens,
            max_output_tokens,
        })
    }

    #[must_use]
    pub const fn context_window_tokens(&self) -> NonZeroU64 {
        self.context_window_tokens
    }

    #[must_use]
    pub const fn max_output_tokens(&self) -> NonZeroU32 {
        self.max_output_tokens
    }
}

impl PiBridgeConfig {
    fn new(
        bridge: impl Into<PathBuf>,
        provider: impl Into<String>,
        model: impl Into<String>,
        credential_store: impl Into<PathBuf>,
    ) -> Result<Self, PiModelConfigError> {
        let bridge = std::fs::canonicalize(bridge.into()).map_err(PiModelConfigError::Bridge)?;
        if !bridge.is_file() {
            return Err(PiModelConfigError::BridgeNotFile(bridge));
        }
        let credential_store = std::fs::canonicalize(credential_store.into())
            .map_err(PiModelConfigError::CredentialStore)?;
        if !credential_store.is_file() {
            return Err(PiModelConfigError::CredentialStoreNotFile(credential_store));
        }
        let provider = provider.into();
        if provider != "xai" && provider != "opencode-go" {
            return Err(PiModelConfigError::UnsupportedProvider(provider));
        }
        let model = model.into();
        if model.is_empty() {
            return Err(PiModelConfigError::EmptyModel);
        }
        Ok(Self {
            bridge,
            provider,
            model,
            credential_store,
        })
    }
}

impl Model for PiModel {
    fn stream(
        &self,
        request: ModelRequest,
        cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        let config = self.config.clone();
        let max_output_tokens = self.max_output_tokens;
        stream::once(async move {
            invoke_bridge(config, max_output_tokens, request, cancellation)
                .await
                .map(|response| ModelEvent::Completed { response })
        })
        .boxed()
    }
}

impl ContextSizer for PiModel {
    fn estimate_input_tokens(&self, request: &ModelRequest) -> u64 {
        crate::pi_context::estimate_input_tokens(request)
    }
}

async fn invoke_bridge(
    config: PiBridgeConfig,
    max_output_tokens: NonZeroU32,
    request: ModelRequest,
    cancellation: CancellationToken,
) -> Result<ModelResponse, ModelError> {
    let input = serde_json::to_vec(&request)
        .map_err(|error| model_error("encode Pi model request", error))?;
    let output = run_bridge(
        config,
        BridgeAction::Invoke,
        Some(max_output_tokens),
        input,
        cancellation,
    )
    .await?;
    decode_response(&output)
}

async fn describe_bridge(config: PiBridgeConfig) -> Result<PiModelDescription, ModelError> {
    let output = run_bridge(
        config,
        BridgeAction::Describe,
        None,
        Vec::new(),
        CancellationToken::new(),
    )
    .await?;
    decode_response(&output)
}

async fn run_bridge(
    config: PiBridgeConfig,
    action: BridgeAction,
    max_output_tokens: Option<NonZeroU32>,
    input: Vec<u8>,
    cancellation: CancellationToken,
) -> Result<Vec<u8>, ModelError> {
    let mut command = Command::new("node");
    command
        .arg(config.bridge)
        .env("RENOA_PI_ACTION", action.as_str())
        .env("RENOA_PI_PROVIDER", config.provider)
        .env("RENOA_PI_MODEL", config.model)
        .env("RENOA_PI_AUTH_STORE", config.credential_store)
        .kill_on_drop(true)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(limit) = max_output_tokens {
        command.env("RENOA_PI_MAX_OUTPUT_TOKENS", limit.get().to_string());
    }
    let mut child = command
        .spawn()
        .map_err(|error| model_error("start Pi model bridge", error))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| ModelError::new("Pi model bridge stdin is unavailable"))?;
    let writer = tokio::spawn(async move { stdin.write_all(&input).await });
    let stdout = drain(child.stdout.take().expect("piped stdout"));
    let stderr = drain(child.stderr.take().expect("piped stderr"));
    let exit = tokio::select! {
        biased;
        () = cancellation.cancelled() => {
            let _ = child.start_kill();
            child.wait().await.map_err(|error| model_error("reap Pi model bridge", error))?;
            ProcessExit::Cancelled
        }
        status = child.wait() => ProcessExit::Finished(
            status.map_err(|error| model_error("wait for Pi model bridge", error))?
        ),
    };
    let write_result = writer
        .await
        .map_err(|error| ModelError::new(format!("Pi request writer failed: {error}")))?;
    let stdout = join_output(stdout, "stdout").await?;
    let stderr = join_output(stderr, "stderr").await?;
    if matches!(exit, ProcessExit::Cancelled) {
        return Err(ModelError::new("Pi model request was cancelled"));
    }
    write_result.map_err(|error| model_error("write Pi model request", error))?;
    let ProcessExit::Finished(status) = exit else {
        unreachable!("cancelled process exits above")
    };
    if !status.success() {
        return Err(ModelError::new(format!(
            "Pi model bridge exited with {status}: {}",
            String::from_utf8_lossy(&stderr.bytes)
        )));
    }
    if stdout.truncated {
        return Err(ModelError::new("Pi model response exceeded 16 MiB"));
    }
    Ok(stdout.bytes)
}

#[derive(Deserialize)]
struct BridgeEnvelope<T> {
    ok: bool,
    response: Option<T>,
    error: Option<String>,
    error_kind: Option<BridgeErrorKind>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum BridgeErrorKind {
    ContextWindowExceeded,
}

#[derive(Deserialize)]
struct PiModelDescription {
    context_window_tokens: u64,
    max_output_tokens: u64,
}

fn decode_response<T: DeserializeOwned>(encoded: &[u8]) -> Result<T, ModelError> {
    let envelope: BridgeEnvelope<T> = serde_json::from_slice(encoded)
        .map_err(|error| model_error("decode Pi model response", error))?;
    match (
        envelope.ok,
        envelope.response,
        envelope.error,
        envelope.error_kind,
    ) {
        (true, Some(response), None, None) => Ok(response),
        (false, None, Some(error), None) => Err(ModelError::new(error)),
        (false, None, Some(error), Some(BridgeErrorKind::ContextWindowExceeded)) => {
            Err(ModelError::context_window_exceeded(error))
        }
        _ => Err(ModelError::new(
            "Pi model bridge returned an invalid envelope",
        )),
    }
}

enum BridgeAction {
    Describe,
    Invoke,
}

impl BridgeAction {
    const fn as_str(&self) -> &'static str {
        match self {
            Self::Describe => "describe",
            Self::Invoke => "invoke",
        }
    }
}

enum ProcessExit {
    Cancelled,
    Finished(std::process::ExitStatus),
}

struct CapturedOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

fn drain(
    mut reader: impl AsyncRead + Unpin + Send + 'static,
) -> JoinHandle<io::Result<CapturedOutput>> {
    tokio::spawn(async move {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 8_192];
        let mut truncated = false;
        loop {
            let read = reader.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            let remaining = OUTPUT_LIMIT.saturating_sub(bytes.len());
            let retained = read.min(remaining);
            bytes.extend_from_slice(&buffer[..retained]);
            truncated |= retained < read;
        }
        Ok(CapturedOutput { bytes, truncated })
    })
}

async fn join_output(
    output: JoinHandle<io::Result<CapturedOutput>>,
    name: &str,
) -> Result<CapturedOutput, ModelError> {
    output
        .await
        .map_err(|error| ModelError::new(format!("Pi {name} reader failed: {error}")))?
        .map_err(|error| model_error(&format!("read Pi model {name}"), error))
}

fn model_error(action: &str, error: impl std::fmt::Display) -> ModelError {
    ModelError::new(format!("cannot {action}: {error}"))
}
