use std::{
    fmt::Write as _,
    io,
    num::{NonZeroU32, NonZeroU64},
    path::PathBuf,
};

use renoa_agent::{Model, ModelError, ModelEventStream, ModelRequest};
use renoa_harness::ContextSizer;
use serde::{Deserialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use crate::pi_catalog::PiReasoningLevel;

pub(crate) const OUTPUT_LIMIT: usize = 16 * 1_024 * 1_024;

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
    #[error("Pi model bridge returned an invalid model specification")]
    InvalidModelSpec,
    #[error("Pi model bridge returned an invalid model binding id")]
    InvalidModelBindingId,
    #[error("Pi model bridge returned an invalid model catalog")]
    InvalidModelCatalog,
}

/// A provider adapter that invokes Pi AI for one exact Renoa model request.
pub struct PiModel {
    config: PiBridgeConfig,
    binding_id: String,
    context_window_tokens: NonZeroU64,
    max_output_tokens: NonZeroU32,
    reasoning: PiReasoningLevel,
}

#[derive(Clone)]
pub(crate) struct PiBridgeConfig {
    bridge: PathBuf,
    provider: String,
    model: Option<String>,
    credential_store: PathBuf,
    model_spec: Option<String>,
    reasoning: Option<PiReasoningLevel>,
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
        reasoning: Option<PiReasoningLevel>,
        max_output_tokens: NonZeroU32,
    ) -> Result<Self, PiModelConfigError> {
        Self::load_with_spec(
            bridge,
            provider,
            model,
            credential_store,
            None,
            reasoning,
            max_output_tokens,
        )
        .await
    }

    pub(crate) async fn load_with_spec(
        bridge: impl Into<PathBuf>,
        provider: impl Into<String>,
        model: impl Into<String>,
        credential_store: impl Into<PathBuf>,
        model_spec: Option<String>,
        reasoning: Option<PiReasoningLevel>,
        max_output_tokens: NonZeroU32,
    ) -> Result<Self, PiModelConfigError> {
        let mut config = PiBridgeConfig::new(
            bridge,
            provider,
            model,
            credential_store,
            model_spec,
            reasoning,
        )?;
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
        serde_json::from_str::<serde_json::Value>(&description.model_spec)
            .ok()
            .filter(serde_json::Value::is_object)
            .ok_or(PiModelConfigError::InvalidModelSpec)?;
        if description.model_binding_id.len() != 64
            || !description
                .model_binding_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || description.model_binding_id != sha256_hex(description.model_spec.as_bytes())
        {
            return Err(PiModelConfigError::InvalidModelBindingId);
        }
        config.model_spec = Some(description.model_spec);
        config.reasoning = Some(description.reasoning_level);
        Ok(Self {
            config,
            binding_id: description.model_binding_id,
            context_window_tokens,
            max_output_tokens,
            reasoning: description.reasoning_level,
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

    #[must_use]
    pub fn binding_id(&self) -> &str {
        &self.binding_id
    }

    #[must_use]
    pub const fn reasoning(&self) -> PiReasoningLevel {
        self.reasoning
    }
}

impl PiBridgeConfig {
    fn new(
        bridge: impl Into<PathBuf>,
        provider: impl Into<String>,
        model: impl Into<String>,
        credential_store: impl Into<PathBuf>,
        model_spec: Option<String>,
        reasoning: Option<PiReasoningLevel>,
    ) -> Result<Self, PiModelConfigError> {
        let mut config = Self::for_provider(bridge, provider, credential_store)?;
        let model = model.into();
        if model.is_empty() {
            return Err(PiModelConfigError::EmptyModel);
        }
        config.model = Some(model);
        if let Some(model_spec) = model_spec {
            serde_json::from_str::<serde_json::Value>(&model_spec)
                .ok()
                .filter(serde_json::Value::is_object)
                .ok_or(PiModelConfigError::InvalidModelSpec)?;
            config.model_spec = Some(model_spec);
        }
        config.reasoning = reasoning;
        Ok(config)
    }

    pub(crate) fn for_provider(
        bridge: impl Into<PathBuf>,
        provider: impl Into<String>,
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
        Ok(Self {
            bridge,
            provider,
            model: None,
            credential_store,
            model_spec: None,
            reasoning: None,
        })
    }

    pub(crate) fn command(&self, action: &str, max_output_tokens: Option<NonZeroU32>) -> Command {
        let mut command = Command::new("node");
        command
            .arg("--dns-result-order=ipv4first")
            .arg(&self.bridge)
            .env("RENOA_PI_ACTION", action)
            .env("RENOA_PI_PROVIDER", &self.provider)
            .env("RENOA_PI_AUTH_STORE", &self.credential_store)
            .kill_on_drop(true)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        if let Some(model) = &self.model {
            command.env("RENOA_PI_MODEL", model);
        }
        if let Some(limit) = max_output_tokens {
            command.env("RENOA_PI_MAX_OUTPUT_TOKENS", limit.get().to_string());
        }
        if let Some(model_spec) = &self.model_spec {
            command.env("RENOA_PI_MODEL_SPEC", model_spec);
        }
        if let Some(reasoning) = self.reasoning {
            command.env("RENOA_PI_REASONING", reasoning.as_str());
        }
        command
    }
}

impl Model for PiModel {
    fn stream(
        &self,
        request: ModelRequest,
        cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        crate::pi_stream::stream_model(
            self.config.clone(),
            self.max_output_tokens,
            &request,
            cancellation,
        )
    }
}

impl ContextSizer for PiModel {
    fn estimate_input_tokens(&self, request: &ModelRequest) -> u64 {
        crate::pi_context::estimate_input_tokens(request)
    }
}

impl renoa_agent_loop::ContextSizer for PiModel {
    fn estimate_input_tokens(&self, request: &ModelRequest) -> u64 {
        crate::pi_context::estimate_input_tokens(request)
    }
}

async fn describe_bridge(config: PiBridgeConfig) -> Result<PiModelDescription, ModelError> {
    let output = run_bridge(
        config,
        "describe",
        None,
        Vec::new(),
        CancellationToken::new(),
    )
    .await?;
    decode_response(&output)
}

pub(crate) async fn run_bridge(
    config: PiBridgeConfig,
    action: &str,
    max_output_tokens: Option<NonZeroU32>,
    input: Vec<u8>,
    cancellation: CancellationToken,
) -> Result<Vec<u8>, ModelError> {
    let mut child = config
        .command(action, max_output_tokens)
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
    model_binding_id: String,
    model_spec: String,
    reasoning_level: PiReasoningLevel,
}

pub(crate) fn decode_response<T: DeserializeOwned>(encoded: &[u8]) -> Result<T, ModelError> {
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

enum ProcessExit {
    Cancelled,
    Finished(std::process::ExitStatus),
}

pub(crate) struct CapturedOutput {
    pub(crate) bytes: Vec<u8>,
    pub(crate) truncated: bool,
}

pub(crate) fn drain(
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

pub(crate) async fn join_output(
    output: JoinHandle<io::Result<CapturedOutput>>,
    name: &str,
) -> Result<CapturedOutput, ModelError> {
    output
        .await
        .map_err(|error| ModelError::new(format!("Pi {name} reader failed: {error}")))?
        .map_err(|error| model_error(&format!("read Pi model {name}"), error))
}

pub(crate) fn model_error(action: &str, error: impl std::fmt::Display) -> ModelError {
    ModelError::new(format!("cannot {action}: {error}"))
}

fn sha256_hex(value: &[u8]) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in Sha256::digest(value) {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}
