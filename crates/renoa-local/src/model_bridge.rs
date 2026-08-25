use std::{
    fmt::Write as _,
    io,
    num::{NonZeroU32, NonZeroU64},
    path::PathBuf,
    time::Duration,
};

use renoa_agent::{
    InferenceOutcome, Model, ModelError, ModelErrorKind, ModelEventStream, ModelFailureDiagnostic,
    ModelRequest,
};
use serde::{Deserialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use crate::model_catalog::ReasoningLevel;
use crate::process::{child_pid_raw, configure_process_group, stop_process_group_raw};

pub(crate) const OUTPUT_LIMIT: usize = 16 * 1_024 * 1_024;
const CONTROL_ACTION_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ModelBridgeError {
    #[error("model adapter cannot be resolved: {0}")]
    Bridge(#[source] io::Error),
    #[error("credential store cannot be resolved: {0}")]
    CredentialStore(#[source] io::Error),
    #[error("model adapter is not a file: {0}")]
    BridgeNotFile(PathBuf),
    #[error("credential store is not a file: {0}")]
    CredentialStoreNotFile(PathBuf),
    #[error("unsupported model provider: {0}")]
    UnsupportedProvider(String),
    #[error("model id must not be empty")]
    EmptyModel,
    #[error("model adapter could not resolve the selected model: {0}")]
    ModelResolution(#[source] ModelError),
    #[error("model adapter reported a zero-token context window")]
    ZeroContextWindow,
    #[error("model adapter reported a zero-token output limit")]
    ZeroProviderOutputLimit,
    #[error("model adapter returned an invalid model specification")]
    InvalidModelSpec,
    #[error("model adapter returned an invalid model binding id")]
    InvalidModelBindingId,
    #[error("model adapter returned an invalid model catalog")]
    InvalidModelCatalog,
}

/// A process-boundary provider adapter for one exact Renoa model request.
pub struct BridgeModel {
    config: ModelBridgeConfig,
    binding_id: String,
    context_window_tokens: NonZeroU64,
    max_output_tokens: NonZeroU32,
    reasoning: ReasoningLevel,
}

#[derive(Clone)]
pub(crate) struct ModelBridgeConfig {
    bridge: PathBuf,
    provider: String,
    model: Option<String>,
    credential_store: PathBuf,
    model_spec: Option<String>,
    reasoning: Option<ReasoningLevel>,
}

impl BridgeModel {
    /// Resolves and validates the selected model before accepting work.
    ///
    /// # Errors
    ///
    /// Returns an error when configuration, credentials, or model limits are invalid.
    pub async fn load(
        bridge: impl Into<PathBuf>,
        provider: impl Into<String>,
        model: impl Into<String>,
        credential_store: impl Into<PathBuf>,
        reasoning: Option<ReasoningLevel>,
        max_output_tokens: NonZeroU32,
    ) -> Result<Self, ModelBridgeError> {
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
        reasoning: Option<ReasoningLevel>,
        max_output_tokens: NonZeroU32,
    ) -> Result<Self, ModelBridgeError> {
        let mut config = ModelBridgeConfig::new(
            bridge,
            provider,
            model,
            credential_store,
            model_spec,
            reasoning,
        )?;
        let description = describe_bridge(config.clone())
            .await
            .map_err(ModelBridgeError::ModelResolution)?;
        let context_window_tokens = NonZeroU64::new(description.context_window_tokens)
            .ok_or(ModelBridgeError::ZeroContextWindow)?;
        let provider_output_limit = NonZeroU64::new(description.max_output_tokens)
            .ok_or(ModelBridgeError::ZeroProviderOutputLimit)?;
        let max_output_tokens = NonZeroU32::try_from(provider_output_limit)
            .map_or(max_output_tokens, |provider_output_limit| {
                max_output_tokens.min(provider_output_limit)
            });
        serde_json::from_str::<serde_json::Value>(&description.model_spec)
            .ok()
            .filter(serde_json::Value::is_object)
            .ok_or(ModelBridgeError::InvalidModelSpec)?;
        if description.model_binding_id.len() != 64
            || !description
                .model_binding_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || description.model_binding_id != sha256_hex(description.model_spec.as_bytes())
        {
            return Err(ModelBridgeError::InvalidModelBindingId);
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
    pub const fn reasoning(&self) -> ReasoningLevel {
        self.reasoning
    }
}

impl ModelBridgeConfig {
    fn new(
        bridge: impl Into<PathBuf>,
        provider: impl Into<String>,
        model: impl Into<String>,
        credential_store: impl Into<PathBuf>,
        model_spec: Option<String>,
        reasoning: Option<ReasoningLevel>,
    ) -> Result<Self, ModelBridgeError> {
        let mut config = Self::for_provider(bridge, provider, credential_store)?;
        let model = model.into();
        if model.is_empty() {
            return Err(ModelBridgeError::EmptyModel);
        }
        config.model = Some(model);
        if let Some(model_spec) = model_spec {
            serde_json::from_str::<serde_json::Value>(&model_spec)
                .ok()
                .filter(serde_json::Value::is_object)
                .ok_or(ModelBridgeError::InvalidModelSpec)?;
            config.model_spec = Some(model_spec);
        }
        config.reasoning = reasoning;
        Ok(config)
    }

    pub(crate) fn for_provider(
        bridge: impl Into<PathBuf>,
        provider: impl Into<String>,
        credential_store: impl Into<PathBuf>,
    ) -> Result<Self, ModelBridgeError> {
        let bridge = std::fs::canonicalize(bridge.into()).map_err(ModelBridgeError::Bridge)?;
        if !bridge.is_file() {
            return Err(ModelBridgeError::BridgeNotFile(bridge));
        }
        let credential_store = std::fs::canonicalize(credential_store.into())
            .map_err(ModelBridgeError::CredentialStore)?;
        if !credential_store.is_file() {
            return Err(ModelBridgeError::CredentialStoreNotFile(credential_store));
        }
        let provider = provider.into();
        if provider != "xai" && provider != "opencode-go" {
            return Err(ModelBridgeError::UnsupportedProvider(provider));
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
            .env("RENOA_MODEL_ACTION", action)
            .env("RENOA_MODEL_PROVIDER", &self.provider)
            .env("RENOA_MODEL_AUTH_STORE", &self.credential_store)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        if let Some(model) = &self.model {
            command.env("RENOA_MODEL", model);
        }
        if let Some(limit) = max_output_tokens {
            command.env("RENOA_MODEL_MAX_OUTPUT_TOKENS", limit.get().to_string());
        }
        if let Some(model_spec) = &self.model_spec {
            command.env("RENOA_MODEL_SPEC", model_spec);
        }
        if let Some(reasoning) = self.reasoning {
            command.env("RENOA_MODEL_REASONING", reasoning.as_str());
        }
        configure_process_group(&mut command);
        command
    }
}

impl Model for BridgeModel {
    fn stream(
        &self,
        request: ModelRequest,
        cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        crate::model_stream::stream_model(
            self.config.clone(),
            self.max_output_tokens,
            &request,
            cancellation,
        )
    }
}

impl renoa_agent_loop::ContextSizer for BridgeModel {
    fn estimate_input_tokens(&self, request: &ModelRequest) -> u64 {
        crate::model_context::estimate_input_tokens(request)
    }
}

async fn describe_bridge(config: ModelBridgeConfig) -> Result<BridgeDescription, ModelError> {
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
    config: ModelBridgeConfig,
    action: &str,
    max_output_tokens: Option<NonZeroU32>,
    input: Vec<u8>,
    cancellation: CancellationToken,
) -> Result<Vec<u8>, ModelError> {
    let mut child = config
        .command(action, max_output_tokens)
        .spawn()
        .map_err(|error| model_error("start model adapter", error))?;
    let pid = child_pid_raw(&child)
        .map_err(|error| ModelError::new(format!("model adapter ownership failed: {error}")))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| ModelError::new("model adapter stdin is unavailable"))?;
    let writer = tokio::spawn(async move { stdin.write_all(&input).await });
    let stdout = drain(child.stdout.take().expect("piped stdout"));
    let stderr = drain(child.stderr.take().expect("piped stderr"));
    let exit = tokio::select! {
        biased;
        () = cancellation.cancelled() => {
            stop_bridge_child(&mut child, pid).await?;
            ProcessExit::Cancelled
        }
        status = child.wait() => ProcessExit::Finished(
            status.map_err(|error| model_error("wait for model adapter", error))?
        ),
        () = tokio::time::sleep(CONTROL_ACTION_TIMEOUT) => {
            stop_bridge_child(&mut child, pid).await?;
            ProcessExit::TimedOut
        }
    };
    if matches!(&exit, ProcessExit::Finished(_)) {
        stop_bridge_child(&mut child, pid).await?;
    }
    let write_result = writer
        .await
        .map_err(|error| ModelError::new(format!("model request writer failed: {error}")))?;
    let stdout = join_output(stdout, "stdout").await?;
    let stderr = join_output(stderr, "stderr").await?;
    let status = match exit {
        ProcessExit::Finished(status) => status,
        ProcessExit::Cancelled => return Err(ModelError::cancelled("model request was cancelled")),
        ProcessExit::TimedOut => {
            return Err(ModelError::timeout(
                "model adapter control action exceeded its 30-second deadline",
            ));
        }
    };
    write_result.map_err(|error| model_error("write model request", error))?;
    if !status.success() {
        return Err(ModelError::new(format!(
            "model adapter exited with {status}: {}",
            String::from_utf8_lossy(&stderr.bytes)
        )));
    }
    if stdout.truncated {
        return Err(ModelError::new("model adapter response exceeded 16 MiB"));
    }
    Ok(stdout.bytes)
}

#[derive(Deserialize)]
struct BridgeEnvelope<T> {
    ok: bool,
    response: Option<T>,
    error: Option<String>,
    #[serde(default)]
    error_kind: Option<ModelErrorKind>,
    #[serde(default)]
    inference_outcome: Option<InferenceOutcome>,
    #[serde(default)]
    diagnostic: Option<ModelFailureDiagnostic>,
}

#[derive(Deserialize)]
struct BridgeDescription {
    context_window_tokens: u64,
    max_output_tokens: u64,
    model_binding_id: String,
    model_spec: String,
    reasoning_level: ReasoningLevel,
}

pub(crate) fn decode_response<T: DeserializeOwned>(encoded: &[u8]) -> Result<T, ModelError> {
    let envelope: BridgeEnvelope<T> = serde_json::from_slice(encoded)
        .map_err(|error| model_error("decode model adapter response", error))?;
    if envelope.ok {
        return match (envelope.response, envelope.error) {
            (Some(response), None) => Ok(response),
            _ => Err(ModelError::new(
                "model adapter returned an invalid envelope",
            )),
        };
    }
    match envelope.error {
        Some(error) => Err(classified_error(
            error,
            envelope.error_kind,
            envelope.inference_outcome,
            envelope.diagnostic,
        )),
        None => Err(ModelError::new(
            "model adapter returned an invalid envelope",
        )),
    }
}

pub(crate) fn classified_error(
    message: String,
    kind: Option<ModelErrorKind>,
    inference_outcome: Option<InferenceOutcome>,
    diagnostic: Option<ModelFailureDiagnostic>,
) -> ModelError {
    let kind = kind.unwrap_or(ModelErrorKind::Unknown);
    let inference_outcome = inference_outcome.unwrap_or(InferenceOutcome::Unknown);
    ModelError::classified(kind, inference_outcome, message, diagnostic)
}

enum ProcessExit {
    Cancelled,
    TimedOut,
    Finished(std::process::ExitStatus),
}

pub(crate) async fn stop_bridge_child(
    child: &mut tokio::process::Child,
    pid: u32,
) -> Result<(), ModelError> {
    stop_process_group_raw(child, pid)
        .await
        .map_err(|error| ModelError::new(format!("model adapter cleanup failed: {error}")))
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
        .map_err(|error| ModelError::new(format!("model adapter {name} reader failed: {error}")))?
        .map_err(|error| model_error(&format!("read model adapter {name}"), error))
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
