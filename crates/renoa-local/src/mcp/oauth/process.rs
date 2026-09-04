use std::{io, path::Path, time::Duration};

use serde::Serialize;
use serde_json::Value;
use tokio::{io::AsyncWriteExt as _, process::Command, sync::oneshot};
use tokio_util::sync::CancellationToken;

use crate::{
    mcp::{McpAdapterError, McpCredentialHeader, McpRemoteFailure},
    process::{child_pid_raw, configure_process_group, stop_process_group_raw},
};

use super::SensitiveString;

mod capture;
mod record;

use capture::{drain, stop_and_capture, stop_and_discard};
use record::parse_record;

const WIRE_VERSION: u32 = 9;
const PROCESS_DEADLINE: Duration = Duration::from_secs(35);
const MAX_REQUEST_BYTES: usize = 1_024 * 1_024;
const MAX_STDOUT_BYTES: usize = 20 * 1_024 * 1_024;
const MAX_STDERR_BYTES: usize = 64 * 1_024;
const MAX_OAUTH_STATE_BYTES: usize = 512 * 1_024;
const MAX_OAUTH_VALUE_BYTES: usize = 16 * 1_024;

pub(super) enum OAuthResult {
    Redirect {
        authorization_url: String,
        state: Value,
    },
    Authorized {
        authorization: McpCredentialHeader,
        state: Value,
    },
    RefreshRequired {
        state: Value,
    },
    Failed {
        failure: McpRemoteFailure,
        state: Value,
    },
}

pub(super) enum AdapterOAuthResult {
    Discovered(OAuthDiscovery),
    Flow(OAuthResult),
}

impl AdapterOAuthResult {
    fn into_flow(self) -> Result<OAuthResult, McpAdapterError> {
        match self {
            Self::Flow(result) => Ok(result),
            Self::Discovered(_) => Err(McpAdapterError::Protocol(
                "OAuth adapter returned metadata for a flow request".to_owned(),
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct OAuthDiscovery {
    pub(super) issuer: String,
    pub(super) client_metadata_supported: bool,
    pub(super) dynamic_registration_supported: bool,
}

#[derive(Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub(super) enum OAuthRegistration {
    Dynamic,
    ClientMetadata {
        client_metadata_url: String,
    },
    PreRegistered {
        issuer: String,
        client_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        client_secret: Option<SensitiveString>,
    },
}

impl OAuthRegistration {
    fn exact_secret(&self) -> Option<&str> {
        match self {
            Self::PreRegistered {
                client_secret: Some(secret),
                ..
            } => Some(secret.expose()),
            Self::Dynamic
            | Self::ClientMetadata { .. }
            | Self::PreRegistered {
                client_secret: None,
                ..
            } => None,
        }
    }
}

pub(super) struct OAuthBegin<'a> {
    pub(super) endpoint: &'a str,
    pub(super) csrf_state: &'a str,
    pub(super) redirect_uri: &'a str,
    pub(super) force_reauthorization: bool,
    pub(super) requested_scope: Option<&'a str>,
    pub(super) registration: &'a OAuthRegistration,
    pub(super) prior: Option<&'a Value>,
}

pub(super) async fn discover(
    adapter: &Path,
    endpoint: &str,
    cancellation: CancellationToken,
) -> Result<OAuthDiscovery, McpAdapterError> {
    match run(
        adapter,
        &OAuthRequest::Discover {
            wire_version: WIRE_VERSION,
            action: "oauth_discover",
            endpoint,
        },
        cancellation,
    )
    .await?
    {
        AdapterOAuthResult::Discovered(discovery) => Ok(discovery),
        AdapterOAuthResult::Flow(_) => Err(McpAdapterError::Protocol(
            "OAuth adapter returned a flow result for metadata discovery".to_owned(),
        )),
    }
}

pub(super) async fn begin(
    adapter: &Path,
    request: OAuthBegin<'_>,
    cancellation: CancellationToken,
) -> Result<OAuthResult, McpAdapterError> {
    run(
        adapter,
        &OAuthRequest::Begin {
            wire_version: WIRE_VERSION,
            action: "oauth_begin",
            endpoint: request.endpoint,
            csrf_state: request.csrf_state,
            redirect_uri: request.redirect_uri,
            force_reauthorization: request.force_reauthorization,
            requested_scope: request.requested_scope,
            registration: request.registration,
            oauth_state: request.prior,
        },
        cancellation,
    )
    .await?
    .into_flow()
}

pub(super) async fn exchange(
    adapter: &Path,
    endpoint: &str,
    authorization_code: &str,
    issuer: Option<&str>,
    registration: &OAuthRegistration,
    state: &Value,
    cancellation: CancellationToken,
) -> Result<OAuthResult, McpAdapterError> {
    run(
        adapter,
        &OAuthRequest::Exchange {
            wire_version: WIRE_VERSION,
            action: "oauth_exchange",
            endpoint,
            authorization_code,
            issuer,
            registration,
            oauth_state: state,
        },
        cancellation,
    )
    .await?
    .into_flow()
}

pub(super) async fn token(
    adapter: &Path,
    endpoint: &str,
    state: &Value,
    cancellation: CancellationToken,
) -> Result<OAuthResult, McpAdapterError> {
    run(
        adapter,
        &OAuthRequest::Token {
            wire_version: WIRE_VERSION,
            action: "oauth_token",
            endpoint,
            oauth_state: state,
        },
        cancellation,
    )
    .await?
    .into_flow()
}

pub(super) async fn refresh(
    adapter: &Path,
    endpoint: &str,
    registration: &OAuthRegistration,
    state: &Value,
    cancellation: CancellationToken,
) -> Result<OAuthResult, McpAdapterError> {
    run(
        adapter,
        &OAuthRequest::Refresh {
            wire_version: WIRE_VERSION,
            action: "oauth_refresh",
            endpoint,
            registration,
            oauth_state: state,
        },
        cancellation,
    )
    .await?
    .into_flow()
}

#[derive(Serialize)]
#[serde(untagged)]
enum OAuthRequest<'a> {
    Discover {
        wire_version: u32,
        action: &'static str,
        endpoint: &'a str,
    },
    Begin {
        wire_version: u32,
        action: &'static str,
        endpoint: &'a str,
        csrf_state: &'a str,
        redirect_uri: &'a str,
        force_reauthorization: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        requested_scope: Option<&'a str>,
        registration: &'a OAuthRegistration,
        #[serde(skip_serializing_if = "Option::is_none")]
        oauth_state: Option<&'a Value>,
    },
    Exchange {
        wire_version: u32,
        action: &'static str,
        endpoint: &'a str,
        authorization_code: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        issuer: Option<&'a str>,
        registration: &'a OAuthRegistration,
        oauth_state: &'a Value,
    },
    Token {
        wire_version: u32,
        action: &'static str,
        endpoint: &'a str,
        oauth_state: &'a Value,
    },
    Refresh {
        wire_version: u32,
        action: &'static str,
        endpoint: &'a str,
        registration: &'a OAuthRegistration,
        oauth_state: &'a Value,
    },
}

impl OAuthRequest<'_> {
    const fn endpoint(&self) -> &str {
        match self {
            Self::Discover { endpoint, .. }
            | Self::Begin { endpoint, .. }
            | Self::Exchange { endpoint, .. }
            | Self::Token { endpoint, .. }
            | Self::Refresh { endpoint, .. } => endpoint,
        }
    }

    fn exact_secrets(&self) -> Vec<&str> {
        let mut secrets = self
            .registration()
            .and_then(OAuthRegistration::exact_secret)
            .into_iter()
            .collect::<Vec<_>>();
        match self {
            Self::Discover { .. } => {}
            Self::Begin {
                csrf_state,
                oauth_state,
                ..
            } => {
                secrets.push(*csrf_state);
                if let Some(state) = oauth_state {
                    collect_state_secrets(state, &mut secrets);
                }
            }
            Self::Exchange {
                authorization_code,
                oauth_state,
                ..
            } => {
                secrets.push(*authorization_code);
                collect_state_secrets(oauth_state, &mut secrets);
            }
            Self::Token { oauth_state, .. } | Self::Refresh { oauth_state, .. } => {
                collect_state_secrets(oauth_state, &mut secrets);
            }
        }
        secrets
    }

    const fn registration(&self) -> Option<&OAuthRegistration> {
        match self {
            Self::Begin { registration, .. }
            | Self::Exchange { registration, .. }
            | Self::Refresh { registration, .. } => Some(registration),
            Self::Discover { .. } | Self::Token { .. } => None,
        }
    }
}

fn collect_state_secrets<'a>(value: &'a Value, secrets: &mut Vec<&'a str>) {
    let mut pending = vec![("", value)];
    while let Some((key, value)) = pending.pop() {
        match value {
            Value::String(value) if secret_state_key(key) => secrets.push(value),
            Value::Array(values) => {
                pending.extend(values.iter().map(|value| (key, value)));
            }
            Value::Object(values) => {
                pending.extend(values.iter().map(|(key, value)| (key.as_str(), value)));
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }
}

fn secret_state_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.contains("token")
        || key.contains("secret")
        || key.contains("verifier")
        || matches!(key.as_str(), "csrf_state" | "authorization_code")
}

async fn run(
    adapter: &Path,
    request: &OAuthRequest<'_>,
    cancellation: CancellationToken,
) -> Result<AdapterOAuthResult, McpAdapterError> {
    if cancellation.is_cancelled() {
        return Err(McpAdapterError::Cancelled);
    }
    let mut encoded = serde_json::to_vec(request).map_err(McpAdapterError::Encode)?;
    if encoded.len() > MAX_REQUEST_BYTES {
        encoded.fill(0);
        return Err(McpAdapterError::InputLimit);
    }
    let exact_secrets = request.exact_secrets();
    let result = run_process(
        adapter,
        &encoded,
        request.endpoint(),
        &exact_secrets,
        cancellation,
    )
    .await;
    encoded.fill(0);
    result
}

async fn run_process(
    adapter: &Path,
    request: &[u8],
    expected_endpoint: &str,
    exact_secrets: &[&str],
    cancellation: CancellationToken,
) -> Result<AdapterOAuthResult, McpAdapterError> {
    let deadline = tokio::time::Instant::now() + PROCESS_DEADLINE;
    let mut command = Command::new("node");
    command
        .arg("--dns-result-order=ipv4first")
        .arg(adapter)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    configure_process_group(&mut command);
    let mut child = command.spawn().map_err(McpAdapterError::Start)?;
    let pid = match child_pid_raw(&child) {
        Ok(pid) => pid,
        Err(error) => {
            child
                .kill()
                .await
                .map_err(|cleanup| McpAdapterError::Cleanup(cleanup.to_string()))?;
            return Err(McpAdapterError::Cleanup(error.to_string()));
        }
    };
    let (Some(mut stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        stop_process_group_raw(&mut child, pid)
            .await
            .map_err(|error| McpAdapterError::Cleanup(error.to_string()))?;
        return Err(McpAdapterError::MissingPipe("configured standard-I/O pipe"));
    };
    let (terminal_sender, mut terminal_receiver) = oneshot::channel();
    let stdout = drain(stdout, MAX_STDOUT_BYTES, Some(terminal_sender));
    let stderr = drain(stderr, MAX_STDERR_BYTES, None);
    let write = async {
        stdin.write_all(request).await?;
        stdin.shutdown().await
    };
    tokio::pin!(write);
    let write_result = tokio::select! {
        biased;
        result = &mut write => Some(result),
        () = cancellation.cancelled() => None,
        () = tokio::time::sleep_until(deadline) => None,
    };
    drop(stdin);
    match write_result {
        Some(Ok(())) => {}
        Some(Err(source)) => {
            stop_and_discard(&mut child, pid, stdout, stderr).await?;
            return Err(McpAdapterError::Write(source));
        }
        None => {
            stop_and_discard(&mut child, pid, stdout, stderr).await?;
            return if cancellation.is_cancelled() {
                Err(McpAdapterError::Cancelled)
            } else {
                Err(McpAdapterError::Timeout)
            };
        }
    }
    let signal = wait_for_signal(&mut child, &mut terminal_receiver, deadline, &cancellation).await;
    match signal {
        Signal::Cancelled => {
            stop_and_discard(&mut child, pid, stdout, stderr).await?;
            Err(McpAdapterError::Cancelled)
        }
        Signal::Deadline => {
            stop_and_discard(&mut child, pid, stdout, stderr).await?;
            Err(McpAdapterError::Timeout)
        }
        Signal::Exited(Err(source)) => {
            stop_and_discard(&mut child, pid, stdout, stderr).await?;
            Err(McpAdapterError::Wait(source))
        }
        Signal::Exited(Ok(_)) | Signal::Terminal => {
            let (stdout, stderr) = stop_and_capture(&mut child, pid, stdout, stderr).await?;
            parse_record(stdout, stderr, expected_endpoint, exact_secrets)
        }
    }
}

async fn wait_for_signal(
    child: &mut tokio::process::Child,
    terminal: &mut oneshot::Receiver<()>,
    deadline: tokio::time::Instant,
    cancellation: &CancellationToken,
) -> Signal {
    let wait = child.wait();
    tokio::pin!(wait);
    tokio::select! {
        biased;
        terminal = terminal => if terminal.is_ok() {
            Signal::Terminal
        } else {
            tokio::select! {
                biased;
                status = &mut wait => Signal::Exited(status),
                () = cancellation.cancelled() => Signal::Cancelled,
                () = tokio::time::sleep_until(deadline) => Signal::Deadline,
            }
        },
        status = &mut wait => Signal::Exited(status),
        () = cancellation.cancelled() => Signal::Cancelled,
        () = tokio::time::sleep_until(deadline) => Signal::Deadline,
    }
}

enum Signal {
    Exited(io::Result<std::process::ExitStatus>),
    Terminal,
    Deadline,
    Cancelled,
}
