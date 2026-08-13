use std::{io, path::PathBuf};

use futures_util::{StreamExt, stream};
use renoa_agent::{Model, ModelError, ModelEvent, ModelEventStream, ModelRequest, ModelResponse};
use serde::Deserialize;
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
}

/// A provider adapter that invokes Pi AI for one exact Renoa model request.
pub struct PiModel {
    bridge: PathBuf,
    provider: String,
    model: String,
    credential_store: PathBuf,
}

impl PiModel {
    /// Configures the local Pi AI process adapter.
    ///
    /// # Errors
    ///
    /// Returns an error when paths or the concrete provider selection are invalid.
    pub fn new(
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
        let bridge = self.bridge.clone();
        let provider = self.provider.clone();
        let model = self.model.clone();
        let credential_store = self.credential_store.clone();
        stream::once(async move {
            invoke_bridge(
                bridge,
                provider,
                model,
                credential_store,
                request,
                cancellation,
            )
            .await
            .map(|response| ModelEvent::Completed { response })
        })
        .boxed()
    }
}

async fn invoke_bridge(
    bridge: PathBuf,
    provider: String,
    model: String,
    credential_store: PathBuf,
    request: ModelRequest,
    cancellation: CancellationToken,
) -> Result<ModelResponse, ModelError> {
    let input = serde_json::to_vec(&request)
        .map_err(|error| model_error("encode Pi model request", error))?;
    let mut child = Command::new("node")
        .arg(bridge)
        .env("RENOA_PI_PROVIDER", provider)
        .env("RENOA_PI_MODEL", model)
        .env("RENOA_PI_AUTH_STORE", credential_store)
        .kill_on_drop(true)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
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
    decode_response(&stdout.bytes)
}

#[derive(Deserialize)]
struct BridgeEnvelope {
    ok: bool,
    response: Option<ModelResponse>,
    error: Option<String>,
}

fn decode_response(encoded: &[u8]) -> Result<ModelResponse, ModelError> {
    let envelope: BridgeEnvelope = serde_json::from_slice(encoded)
        .map_err(|error| model_error("decode Pi model response", error))?;
    match (envelope.ok, envelope.response, envelope.error) {
        (true, Some(response), None) => Ok(response),
        (false, None, Some(error)) => Err(ModelError::new(error)),
        _ => Err(ModelError::new(
            "Pi model bridge returned an invalid envelope",
        )),
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
