use std::{io, path::PathBuf, str, time::Duration};

use serde_json::Value;
use tokio::{
    io::{AsyncRead, AsyncReadExt as _},
    process::Command,
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use super::{McpCredentialError, McpHostError};
use crate::process::{child_pid_raw, configure_process_group, stop_process_group_raw};

mod config;
#[cfg(test)]
mod tests;

pub(crate) use config::{McpConnectionAuth, McpOAuthRegistration};

const GH_DEADLINE: Duration = Duration::from_secs(10);
const GH_SOURCE: &str = "GitHub CLI";
const SECRET_SERVICE_SOURCE: &str = "Secret Service";
const MAX_TOKEN_BYTES: usize = 16 * 1_024;
const MAX_STDERR_BYTES: usize = 4 * 1_024;

#[derive(Clone)]
pub(crate) struct McpCredentialResolver {
    gh_executable: PathBuf,
    secret_tool_executable: PathBuf,
}

impl Default for McpCredentialResolver {
    fn default() -> Self {
        Self {
            gh_executable: PathBuf::from("gh"),
            secret_tool_executable: PathBuf::from("secret-tool"),
        }
    }
}

impl McpCredentialResolver {
    pub(super) fn secret_tool_executable(&self) -> PathBuf {
        self.secret_tool_executable.clone()
    }

    #[cfg(test)]
    pub(crate) fn with_gh_executable(path: PathBuf) -> Self {
        Self {
            gh_executable: path,
            secret_tool_executable: PathBuf::from("secret-tool"),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_executables(gh: PathBuf, secret_tool: PathBuf) -> Self {
        Self {
            gh_executable: gh,
            secret_tool_executable: secret_tool,
        }
    }

    pub(crate) async fn resolve(
        &self,
        reference: &McpConnectionAuth,
        cancellation: CancellationToken,
    ) -> Result<Option<McpAuthorization>, McpCredentialError> {
        match reference {
            McpConnectionAuth::None => Ok(None),
            McpConnectionAuth::GhCli { hostname, account } => self
                .resolve_gh_token(hostname, account, cancellation)
                .await
                .map(|token| Some(McpAuthorization { token })),
            McpConnectionAuth::SecretServiceBearer { credential_id } => self
                .resolve_secret_service_token(credential_id, cancellation)
                .await
                .map(|token| Some(McpAuthorization { token })),
            McpConnectionAuth::OAuth { .. } => Err(McpCredentialError::Unavailable {
                source_name: "OAuth",
                reference: "an interactive MCP connection".to_owned(),
                status: "authorization required".to_owned(),
                guidance: "authorize the connection through extension_manage".to_owned(),
            }),
        }
    }

    async fn resolve_gh_token(
        &self,
        hostname: &str,
        account: &str,
        cancellation: CancellationToken,
    ) -> Result<SecretToken, McpCredentialError> {
        self.resolve_command_token(
            GH_SOURCE,
            &self.gh_executable,
            ["auth", "token", "--hostname", hostname, "--user", account],
            format!("account `{account}` on `{hostname}`"),
            format!("run `gh auth status --hostname {hostname}`"),
            cancellation,
        )
        .await
    }

    async fn resolve_secret_service_token(
        &self,
        credential_id: &str,
        cancellation: CancellationToken,
    ) -> Result<SecretToken, McpCredentialError> {
        self.resolve_command_token(
            SECRET_SERVICE_SOURCE,
            &self.secret_tool_executable,
            ["lookup", "application", "renoa", "credential", credential_id],
            format!("credential `{credential_id}`"),
            format!(
                "store it with `secret-tool store --label='Renoa {credential_id}' application renoa credential {credential_id}`"
            ),
            cancellation,
        )
        .await
    }

    async fn resolve_command_token<const N: usize>(
        &self,
        source_name: &'static str,
        executable: &std::path::Path,
        arguments: [&str; N],
        reference: String,
        guidance: String,
        cancellation: CancellationToken,
    ) -> Result<SecretToken, McpCredentialError> {
        if cancellation.is_cancelled() {
            return Err(McpCredentialError::Cancelled);
        }
        let mut command = Command::new(executable);
        command
            .args(arguments)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        configure_process_group(&mut command);
        let mut child = command
            .spawn()
            .map_err(|source| McpCredentialError::Start {
                source_name,
                source,
            })?;
        let pid = match child_pid_raw(&child) {
            Ok(pid) => pid,
            Err(error) => {
                child
                    .kill()
                    .await
                    .map_err(|cleanup| McpCredentialError::Cleanup {
                        source_name,
                        detail: cleanup.to_string(),
                    })?;
                return Err(McpCredentialError::Cleanup {
                    source_name,
                    detail: format!("credential process has no identity: {error}"),
                });
            }
        };
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            stop_process_group_raw(&mut child, pid)
                .await
                .map_err(|error| McpCredentialError::Cleanup {
                    source_name,
                    detail: error.to_string(),
                })?;
            return Err(McpCredentialError::MissingPipe(source_name));
        };
        let stdout = drain_bounded(stdout, MAX_TOKEN_BYTES.saturating_add(2));
        let stderr = drain_bounded(stderr, MAX_STDERR_BYTES);
        let deadline = tokio::time::sleep(GH_DEADLINE);
        tokio::pin!(deadline);
        let signal = tokio::select! {
            biased;
            () = cancellation.cancelled() => CredentialSignal::Cancelled,
            () = &mut deadline => CredentialSignal::Deadline,
            status = child.wait() => CredentialSignal::Exited(status),
        };
        let cleanup = stop_process_group_raw(&mut child, pid)
            .await
            .map_err(|error| McpCredentialError::Cleanup {
                source_name,
                detail: error.to_string(),
            });
        let (stdout, stderr) = tokio::join!(stdout, stderr);
        cleanup?;
        let mut stdout = joined(stdout, source_name, "stdout")?;
        let mut stderr = joined(stderr, source_name, "stderr")?;
        stderr.bytes.fill(0);
        if stdout.truncated {
            stdout.bytes.fill(0);
            return Err(McpCredentialError::OutputLimit(source_name));
        }
        match signal {
            CredentialSignal::Cancelled => {
                stdout.bytes.fill(0);
                Err(McpCredentialError::Cancelled)
            }
            CredentialSignal::Deadline => {
                stdout.bytes.fill(0);
                Err(McpCredentialError::Timeout(source_name))
            }
            CredentialSignal::Exited(Err(error)) => {
                stdout.bytes.fill(0);
                Err(McpCredentialError::Wait {
                    source_name,
                    source: error,
                })
            }
            CredentialSignal::Exited(Ok(status)) if !status.success() => {
                stdout.bytes.fill(0);
                Err(McpCredentialError::Unavailable {
                    source_name,
                    reference,
                    status: status.to_string(),
                    guidance,
                })
            }
            CredentialSignal::Exited(Ok(_)) => {
                SecretToken::from_command_output(std::mem::take(&mut stdout.bytes), source_name)
            }
        }
    }
}

enum CredentialSignal {
    Exited(io::Result<std::process::ExitStatus>),
    Cancelled,
    Deadline,
}

pub(crate) struct McpAuthorization {
    token: SecretToken,
}

impl McpAuthorization {
    pub(super) fn from_token(mut token: String) -> Result<Self, McpCredentialError> {
        let bytes = std::mem::take(&mut token).into_bytes();
        SecretToken::from_command_output(bytes, "OAuth").map(|token| Self { token })
    }
    #[cfg(test)]
    pub(crate) fn for_test(token: &str) -> Self {
        Self {
            token: SecretToken::from_command_output(token.as_bytes().to_vec(), "test source")
                .expect("test authorization token is valid"),
        }
    }

    pub(crate) fn bearer(&self) -> &str {
        self.token.expose()
    }

    pub(crate) fn redact_text(&self, value: &mut String) {
        if value.contains(self.bearer()) {
            *value = value.replace(self.bearer(), "[REDACTED]");
        }
    }

    pub(crate) fn redact_json(&self, value: &mut Value) {
        match value {
            Value::String(text) => self.redact_text(text),
            Value::Array(values) => {
                for value in values {
                    self.redact_json(value);
                }
            }
            Value::Object(values) => {
                for value in values.values_mut() {
                    self.redact_json(value);
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
    }
}

struct SecretToken {
    bytes: Vec<u8>,
}

impl SecretToken {
    fn from_command_output(
        mut bytes: Vec<u8>,
        source_name: &'static str,
    ) -> Result<Self, McpCredentialError> {
        if bytes.ends_with(b"\n") {
            bytes.pop();
            if bytes.ends_with(b"\r") {
                bytes.pop();
            }
        }
        let valid = !bytes.is_empty()
            && bytes.len() <= MAX_TOKEN_BYTES
            && bytes.iter().all(u8::is_ascii_graphic)
            && str::from_utf8(&bytes).is_ok();
        if !valid {
            bytes.fill(0);
            return Err(McpCredentialError::InvalidOutput(source_name));
        }
        Ok(Self { bytes })
    }

    fn expose(&self) -> &str {
        str::from_utf8(&self.bytes).expect("validated credential token remains UTF-8")
    }
}

impl Drop for SecretToken {
    fn drop(&mut self) {
        self.bytes.fill(0);
    }
}

struct BoundedOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

impl Drop for BoundedOutput {
    fn drop(&mut self) {
        self.bytes.fill(0);
    }
}

fn drain_bounded(
    mut reader: impl AsyncRead + Unpin + Send + 'static,
    limit: usize,
) -> JoinHandle<io::Result<BoundedOutput>> {
    tokio::spawn(async move {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 4_096];
        let mut truncated = false;
        loop {
            let read = reader.read(&mut buffer).await?;
            if read == 0 {
                return Ok(BoundedOutput { bytes, truncated });
            }
            let retained = read.min(limit.saturating_sub(bytes.len()));
            bytes.extend_from_slice(&buffer[..retained]);
            truncated |= retained < read;
        }
    })
}

fn joined(
    result: Result<io::Result<BoundedOutput>, tokio::task::JoinError>,
    source_name: &'static str,
    stream: &'static str,
) -> Result<BoundedOutput, McpCredentialError> {
    result
        .map_err(|source| McpCredentialError::ReaderTask {
            source_name,
            stream,
            source,
        })?
        .map_err(|error| McpCredentialError::Read {
            source_name,
            stream,
            source: error,
        })
}

fn validate_hostname(hostname: &str) -> Result<(), McpHostError> {
    let valid = !hostname.is_empty()
        && hostname.len() <= 253
        && hostname.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        });
    if valid {
        Ok(())
    } else {
        Err(McpHostError::Invalid(
            "GitHub CLI hostname is not a valid DNS hostname".to_owned(),
        ))
    }
}

fn validate_account(account: &str) -> Result<(), McpHostError> {
    if !account.is_empty()
        && account.len() <= 128
        && account
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        Ok(())
    } else {
        Err(McpHostError::Invalid(
            "GitHub CLI account must be 1-128 ASCII letters, digits, or '-'".to_owned(),
        ))
    }
}
