use std::{net::Ipv4Addr, time::Duration};

use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpStream},
};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::mcp::{McpHostError, McpOAuthError};

const CALLBACK_PATH: &str = "/oauth/callback";
const CALLBACK_LIFETIME: Duration = Duration::from_mins(10);
const RESPONSE_DEADLINE: Duration = Duration::from_secs(2);
const MAX_REQUEST_BYTES: usize = 16 * 1_024;
const MAX_VALUE_BYTES: usize = 16 * 1_024;

pub(super) struct OAuthCallbackListener {
    listener: TcpListener,
    port: u16,
    expires_at_ms: i64,
}

pub(super) struct ReceivedCallback {
    pub(super) authorization_code: String,
    pub(super) issuer: Option<String>,
    response: TcpStream,
}

impl OAuthCallbackListener {
    pub(super) async fn bind_new() -> Result<Self, McpHostError> {
        Self::bind(0, now_ms()?.saturating_add(duration_ms(CALLBACK_LIFETIME)?)).await
    }

    pub(super) async fn resume(port: u16, expires_at_ms: i64) -> Result<Self, McpHostError> {
        if expires_at_ms <= now_ms()? {
            return Err(McpOAuthError::CallbackExpired.into());
        }
        Self::bind(port, expires_at_ms).await
    }

    async fn bind(port: u16, expires_at_ms: i64) -> Result<Self, McpHostError> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await?;
        let port = listener.local_addr()?.port();
        Ok(Self {
            listener,
            port,
            expires_at_ms,
        })
    }

    pub(super) const fn port(&self) -> u16 {
        self.port
    }

    pub(super) const fn expires_at_ms(&self) -> i64 {
        self.expires_at_ms
    }

    pub(super) fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}{CALLBACK_PATH}", self.port)
    }

    pub(super) async fn receive(
        &self,
        expected_state: &str,
        cancellation: &CancellationToken,
    ) -> Result<ReceivedCallback, McpHostError> {
        loop {
            let accept_remaining = remaining(self.expires_at_ms)?;
            let accepted = tokio::select! {
                biased;
                () = cancellation.cancelled() => return Err(McpOAuthError::Cancelled.into()),
                () = tokio::time::sleep(accept_remaining) => {
                    return Err(McpOAuthError::CallbackExpired.into());
                }
                accepted = self.listener.accept() => accepted,
            }?;
            let (mut stream, peer) = accepted;
            if !peer.ip().is_loopback() {
                respond(&mut stream, 403, "Rejected").await;
                continue;
            }
            match read_callback(
                &mut stream,
                self.port,
                expected_state,
                remaining(self.expires_at_ms)?,
                cancellation,
            )
            .await
            {
                Ok(Some(data)) => {
                    return Ok(ReceivedCallback {
                        authorization_code: data.authorization_code,
                        issuer: data.issuer,
                        response: stream,
                    });
                }
                Ok(None) => respond(&mut stream, 404, "Not found").await,
                Err(CallbackReadError::Authorization(error)) => {
                    respond(&mut stream, 400, "Authorization was not completed").await;
                    return Err(McpOAuthError::CallbackRejected(error).into());
                }
                Err(CallbackReadError::Rejected) => {
                    respond(&mut stream, 400, "Invalid callback").await;
                }
                Err(CallbackReadError::Io(error)) => return Err(McpHostError::Io(error)),
                Err(CallbackReadError::Cancelled) => {
                    return Err(McpOAuthError::Cancelled.into());
                }
                Err(CallbackReadError::Expired) => {
                    return Err(McpOAuthError::CallbackExpired.into());
                }
            }
        }
    }
}

impl ReceivedCallback {
    pub(super) async fn acknowledge(mut self) {
        respond(
            &mut self.response,
            200,
            "Renoa received the authorization. You can close this tab.",
        )
        .await;
    }
}

struct CallbackData {
    authorization_code: String,
    issuer: Option<String>,
}

enum CallbackReadError {
    Authorization(String),
    Rejected,
    Io(std::io::Error),
    Cancelled,
    Expired,
}

async fn read_callback(
    stream: &mut TcpStream,
    port: u16,
    expected_state: &str,
    remaining: Duration,
    cancellation: &CancellationToken,
) -> Result<Option<CallbackData>, CallbackReadError> {
    let request = tokio::select! {
        biased;
        () = cancellation.cancelled() => return Err(CallbackReadError::Cancelled),
        () = tokio::time::sleep(remaining) => return Err(CallbackReadError::Expired),
        result = read_head(stream) => result.map_err(CallbackReadError::Io)?,
    };
    let text = std::str::from_utf8(&request).map_err(|_| CallbackReadError::Rejected)?;
    let mut lines = text.split("\r\n");
    let request_line = lines.next().ok_or(CallbackReadError::Rejected)?;
    let mut parts = request_line.split_ascii_whitespace();
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(CallbackReadError::Rejected);
    };
    if method != "GET" || !matches!(version, "HTTP/1.1" | "HTTP/1.0") {
        return Err(CallbackReadError::Rejected);
    }
    let expected_host = format!("127.0.0.1:{port}");
    let hosts = lines
        .filter_map(|line| line.split_once(':'))
        .filter_map(|(name, value)| name.eq_ignore_ascii_case("host").then_some(value.trim()))
        .collect::<Vec<_>>();
    if hosts.as_slice() != [expected_host.as_str()] {
        return Err(CallbackReadError::Rejected);
    }
    let url = Url::parse(&format!("http://{expected_host}{target}"))
        .map_err(|_| CallbackReadError::Rejected)?;
    if url.path() != CALLBACK_PATH {
        return Ok(None);
    }
    let mut code = None;
    let mut state = None;
    let mut issuer = None;
    let mut oauth_error = None;
    for (name, value) in url.query_pairs() {
        if !valid_callback_value(&value) {
            return Err(CallbackReadError::Rejected);
        }
        let slot = match name.as_ref() {
            "code" => &mut code,
            "state" => &mut state,
            "iss" => &mut issuer,
            "error" => &mut oauth_error,
            _ => continue,
        };
        if slot.replace(value.into_owned()).is_some() {
            return Err(CallbackReadError::Rejected);
        }
    }
    let Some(state) = state else {
        return Err(CallbackReadError::Rejected);
    };
    if !constant_time_eq(state.as_bytes(), expected_state.as_bytes()) {
        return Err(CallbackReadError::Rejected);
    }
    if let Some(error) = oauth_error {
        let safe = if valid_oauth_error(&error) {
            error
        } else {
            "authorization_rejected".to_owned()
        };
        return Err(CallbackReadError::Authorization(safe));
    }
    let authorization_code = code
        .filter(|value| !value.is_empty())
        .ok_or(CallbackReadError::Rejected)?;
    Ok(Some(CallbackData {
        authorization_code,
        issuer,
    }))
}

fn valid_callback_value(value: &str) -> bool {
    value.len() <= MAX_VALUE_BYTES && !value.bytes().any(|byte| byte.is_ascii_control())
}

async fn read_head(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 2_048];
    loop {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "OAuth callback closed before its HTTP headers",
            ));
        }
        if request.len().saturating_add(read) > MAX_REQUEST_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "OAuth callback headers exceeded their boundary",
            ));
        }
        request.extend_from_slice(&buffer[..read]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            return Ok(request);
        }
    }
}

async fn respond(stream: &mut TcpStream, status: u16, message: &str) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        _ => "Not Found",
    };
    let body = format!("<!doctype html><meta charset=utf-8><title>Renoa</title><p>{message}</p>");
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{body}",
        body.len()
    );
    // The callback result is already decided; a browser that stops reading must not stall it.
    let _best_effort = tokio::time::timeout(RESPONSE_DEADLINE, async {
        stream.write_all(response.as_bytes()).await?;
        stream.shutdown().await
    })
    .await;
}

fn remaining(expires_at_ms: i64) -> Result<Duration, McpHostError> {
    let millis = expires_at_ms.saturating_sub(now_ms()?);
    u64::try_from(millis)
        .ok()
        .filter(|value| *value > 0)
        .map(Duration::from_millis)
        .ok_or_else(|| McpOAuthError::CallbackExpired.into())
}

fn now_ms() -> Result<i64, McpHostError> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| {
            McpOAuthError::Invalid(format!("system clock is before Unix time: {error}"))
        })?;
    i64::try_from(elapsed.as_millis()).map_err(|error| {
        McpOAuthError::Invalid(format!("system clock cannot be represented: {error}")).into()
    })
}

fn duration_ms(duration: Duration) -> Result<i64, McpHostError> {
    i64::try_from(duration.as_millis()).map_err(|error| {
        McpOAuthError::Invalid(format!(
            "OAuth callback duration cannot be represented: {error}"
        ))
        .into()
    })
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    for index in 0..left.len().max(right.len()) {
        let left = left.get(index).copied().unwrap_or(0);
        let right = right.get(index).copied().unwrap_or(0);
        difference |= usize::from(left ^ right);
    }
    difference == 0
}

fn valid_oauth_error(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use tokio::{
        io::{AsyncReadExt as _, AsyncWriteExt as _},
        net::{TcpListener, TcpStream},
        sync::{Mutex, oneshot},
    };
    use tokio_util::sync::CancellationToken;

    use super::{CallbackReadError, OAuthCallbackListener, read_callback};

    #[tokio::test]
    async fn wrong_state_is_ignored_and_success_waits_for_host_acknowledgement() {
        let listener = OAuthCallbackListener::bind_new()
            .await
            .expect("bind callback fixture");
        let port = listener.port();
        let cancellation = CancellationToken::new();
        let receive =
            tokio::spawn(async move { listener.receive("expected-state", &cancellation).await });

        let rejected = send_and_read(port, "code=bad&state=wrong-state").await;
        assert!(rejected.starts_with("HTTP/1.1 400"));

        let (early_sender, early_receiver) = oneshot::channel();
        let response = Arc::new(Mutex::new(Vec::new()));
        let client = {
            let response = Arc::clone(&response);
            tokio::spawn(async move {
                let mut stream = send(port, "code=one-time-code&state=expected-state").await;
                let mut bytes = Vec::new();
                let read = tokio::time::timeout(
                    Duration::from_millis(100),
                    stream.read_to_end(&mut bytes),
                )
                .await;
                let _ignored = early_sender.send(read.is_ok());
                if read.is_err() {
                    stream
                        .read_to_end(&mut bytes)
                        .await
                        .expect("read acknowledged callback");
                }
                *response.lock().await = bytes;
            })
        };
        let received = receive
            .await
            .expect("callback task joins")
            .expect("valid callback is received");
        assert!(!early_receiver.await.expect("observe early response"));
        assert_eq!(received.authorization_code, "one-time-code");
        received.acknowledge().await;
        client.await.expect("callback client joins");
        assert!(response.lock().await.starts_with(b"HTTP/1.1 200"));
    }

    #[tokio::test]
    async fn cancellation_interrupts_a_partial_callback_request() {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind partial callback fixture");
        let port = listener.local_addr().expect("callback address").port();
        let (client, accepted) =
            tokio::join!(TcpStream::connect(("127.0.0.1", port)), listener.accept());
        let mut client = client.expect("connect partial callback fixture");
        let (mut server, _) = accepted.expect("accept partial callback fixture");
        client
            .write_all(b"GET /oauth/callback?code=one")
            .await
            .expect("write partial callback");
        let cancellation = CancellationToken::new();
        let mut read = Box::pin(read_callback(
            &mut server,
            port,
            "expected-state",
            Duration::from_secs(30),
            &cancellation,
        ));
        assert!(futures_util::poll!(&mut read).is_pending());

        cancellation.cancel();
        let result = tokio::time::timeout(Duration::from_secs(1), read)
            .await
            .expect("cancelled partial callback settles");
        assert!(matches!(result, Err(CallbackReadError::Cancelled)));
    }

    async fn send_and_read(port: u16, query: &str) -> String {
        let mut stream = send(port, query).await;
        let mut bytes = Vec::new();
        stream
            .read_to_end(&mut bytes)
            .await
            .expect("read callback response");
        String::from_utf8(bytes).expect("callback response is UTF-8")
    }

    async fn send(port: u16, query: &str) -> TcpStream {
        let mut stream = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect callback fixture");
        stream
            .write_all(
                format!(
                    "GET /oauth/callback?{query} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .expect("write callback request");
        stream
    }
}
