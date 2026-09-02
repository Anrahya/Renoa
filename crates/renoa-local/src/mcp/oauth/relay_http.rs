use std::time::Duration;

use futures_util::StreamExt as _;
use renoa_oauth_relay_protocol::OAuthRelayErrorResponse;
use tokio_util::sync::CancellationToken;

use crate::mcp::{McpHostError, McpOAuthError};

pub(super) const HTTP_DEADLINE: Duration = Duration::from_secs(15);
const RETRY_BASE_DELAY: Duration = Duration::from_millis(200);
const MAX_HTTP_ATTEMPTS: u32 = 3;
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

pub(super) async fn send_with_retry(
    mut request: impl FnMut() -> reqwest::RequestBuilder,
    cancellation: &CancellationToken,
) -> Result<reqwest::Response, McpHostError> {
    let mut attempt = 1_u32;
    loop {
        require_active(cancellation)?;
        let result = tokio::select! {
            biased;
            () = cancellation.cancelled() => return Err(McpOAuthError::Cancelled.into()),
            result = request().send() => result,
        };
        match result {
            Ok(response)
                if !retryable_status(response.status()) || attempt == MAX_HTTP_ATTEMPTS =>
            {
                return Ok(response);
            }
            Ok(_response) => {}
            Err(error) if attempt == MAX_HTTP_ATTEMPTS => {
                return Err(relay_unavailable(&error.to_string()));
            }
            Err(_) => {}
        }
        let delay = RETRY_BASE_DELAY.saturating_mul(attempt);
        tokio::select! {
            biased;
            () = cancellation.cancelled() => return Err(McpOAuthError::Cancelled.into()),
            () = tokio::time::sleep(delay) => {}
        }
        attempt += 1;
    }
}

pub(super) async fn decode_success<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, McpHostError> {
    let status = response.status();
    let mut body = read_bounded(response).await?;
    if !status.is_success() {
        let parsed = serde_json::from_slice::<OAuthRelayErrorResponse>(&body).ok();
        body.fill(0);
        let detail = parsed.map_or_else(
            || format!("HTTP {status}"),
            |error| format!("{} ({status}): {}", error.code, error.message),
        );
        return Err(relay_unavailable(&detail));
    }
    let parsed = serde_json::from_slice(&body)
        .map_err(|_| relay_unavailable("relay returned malformed JSON"));
    body.fill(0);
    parsed
}

async fn read_bounded(response: reqwest::Response) -> Result<Vec<u8>, McpHostError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(relay_unavailable("relay response exceeded its boundary"));
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| relay_unavailable(&error.to_string()))?;
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            body.fill(0);
            return Err(relay_unavailable("relay response exceeded its boundary"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn retryable_status(status: reqwest::StatusCode) -> bool {
    matches!(
        status,
        reqwest::StatusCode::REQUEST_TIMEOUT
            | reqwest::StatusCode::INTERNAL_SERVER_ERROR
            | reqwest::StatusCode::BAD_GATEWAY
            | reqwest::StatusCode::SERVICE_UNAVAILABLE
            | reqwest::StatusCode::GATEWAY_TIMEOUT
    )
}

fn require_active(cancellation: &CancellationToken) -> Result<(), McpHostError> {
    if cancellation.is_cancelled() {
        Err(McpOAuthError::Cancelled.into())
    } else {
        Ok(())
    }
}

pub(super) fn relay_unavailable(detail: &str) -> McpHostError {
    McpOAuthError::CallbackUnavailable(format!("remote callback relay failed: {detail}")).into()
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use tokio::{
        io::{AsyncReadExt as _, AsyncWriteExt as _},
        net::TcpListener,
    };
    use tokio_util::sync::CancellationToken;

    use super::send_with_retry;

    #[tokio::test]
    async fn transient_gateway_failures_retry_the_same_request() {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind retry server");
        let address = listener.local_addr().expect("retry server address");
        let attempts = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&attempts);
        let server = tokio::spawn(async move {
            for status in ["503 Service Unavailable", "502 Bad Gateway", "200 OK"] {
                let (mut stream, _) = listener.accept().await.expect("accept relay request");
                let mut request = [0_u8; 2_048];
                let read = stream.read(&mut request).await.expect("read relay request");
                assert!(request[..read].starts_with(b"GET /relay HTTP/1.1"));
                observed.fetch_add(1, Ordering::SeqCst);
                stream
                    .write_all(
                        format!(
                            "HTTP/1.1 {status}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
                        )
                        .as_bytes(),
                    )
                    .await
                    .expect("write relay response");
            }
        });
        let client = reqwest::Client::new();
        let endpoint = format!("http://{address}/relay");
        let response = send_with_retry(|| client.get(&endpoint), &CancellationToken::new())
            .await
            .expect("retry transient gateway failures");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        server.await.expect("retry server joins");
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }
}
