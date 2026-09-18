//! Provider wire fixtures and fake providers for the compiled adapter tests.
//! Keeping them here leaves the test module focused on adapter behavior.

use std::{
    io::{Read as _, Write as _},
    net::{Shutdown, TcpListener, TcpStream},
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

use serde_json::Value;

const SSE: &str = concat!(
    "data: {\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"created\":1,",
    "\"model\":\"grok-4.6\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",",
    "\"content\":\"from-compiled-adapter\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"created\":1,",
    "\"model\":\"grok-4.6\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],",
    "\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":4,\"total_tokens\":16,",
    "\"prompt_tokens_details\":{\"cached_tokens\":2,\"cache_write_tokens\":1}}}\n\n",
    "data: [DONE]\n\n",
);

/// One content delta followed by a stream end with no `finish_reason`, which is
/// the provider behavior that produced the live unknown outcome in issue #28.
const TRUNCATED_SSE: &str = concat!(
    "data: {\"id\":\"chatcmpl-truncated\",\"object\":\"chat.completion.chunk\",\"created\":1,",
    "\"model\":\"grok-4.6\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",",
    "\"content\":\"discarded-partial-answer\"},\"finish_reason\":null}]}\n\n",
);

pub(super) fn assert_complete_chat_requests(requests: &[Vec<u8>]) {
    for request in requests {
        let text = String::from_utf8_lossy(request);
        assert!(text.starts_with("POST "), "method: {text}");
        assert!(
            text.contains("/chat/completions"),
            "chat completions route: {text}"
        );
        assert!(
            text.to_ascii_lowercase().contains("authorization: bearer "),
            "auth header: {text}"
        );
        let body = request
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|header_end| &request[header_end + 4..])
            .expect("HTTP body");
        let parsed: Value = serde_json::from_slice(body).expect("JSON body");
        assert_eq!(parsed["model"], "grok-4.6");
        assert!(parsed.get("messages").is_some());
    }
}

pub(super) fn serve_reset_after_complete_request(
    listener: &TcpListener,
    received: &Mutex<Vec<Vec<u8>>>,
) {
    for _ in 0..3 {
        let (mut stream, _) = accept_with_timeout(listener, Duration::from_secs(5));
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("fake provider read timeout");
        let request = read_complete_http_request(&mut stream).expect("read complete request");
        received.lock().expect("request lock").push(request);
        let _ = stream.shutdown(Shutdown::Both);
    }
}

/// Serves one stream that ends after a content delta without a finish reason,
/// then one complete chat completion for the replay of the same effect.
pub(super) fn serve_truncated_then_complete(
    listener: &TcpListener,
    received: &Mutex<Vec<Vec<u8>>>,
) {
    for response in [TRUNCATED_SSE, SSE] {
        let (mut stream, _) = accept_with_timeout(listener, Duration::from_secs(5));
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("fake provider read timeout");
        let request = read_complete_http_request(&mut stream).expect("read complete request");
        received.lock().expect("request lock").push(request);
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{response}",
            response.len()
        );
        stream
            .write_all(response.as_bytes())
            .expect("write provider SSE");
        stream
            .shutdown(Shutdown::Both)
            .expect("end the provider stream for this fixture");
    }
}

pub(super) fn serve_one_chat_completion(listener: &TcpListener) {
    let (mut stream, _) = accept_with_timeout(listener, Duration::from_secs(5));
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("fake provider read timeout");
    let _ = read_complete_http_request(&mut stream).expect("read provider request");
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{SSE}",
        SSE.len()
    );
    stream
        .write_all(response.as_bytes())
        .expect("write provider SSE");
}

fn read_complete_http_request(stream: &mut std::net::TcpStream) -> std::io::Result<Vec<u8>> {
    let mut data = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Ok(data);
        }
        data.extend_from_slice(&buffer[..read]);
        let Some(header_end) = data.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let content_length = std::str::from_utf8(&data[..header_end])
            .ok()
            .and_then(content_length)
            .unwrap_or(0);
        while data.len() < header_end + 4 + content_length {
            let read = stream.read(&mut buffer)?;
            if read == 0 {
                return Ok(data);
            }
            data.extend_from_slice(&buffer[..read]);
        }
        return Ok(data);
    }
}

fn accept_with_timeout(
    listener: &TcpListener,
    timeout: Duration,
) -> (TcpStream, std::net::SocketAddr) {
    listener
        .set_nonblocking(true)
        .expect("fake provider accept nonblocking");
    let started = Instant::now();
    loop {
        match listener.accept() {
            Ok(accepted) => {
                accepted
                    .0
                    .set_nonblocking(false)
                    .expect("fake provider stream blocking");
                return accepted;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    started.elapsed() < timeout,
                    "fake provider accept timed out after {timeout:?}"
                );
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("fake provider accept failed: {error}"),
        }
    }
}

fn content_length(headers: &str) -> Option<usize> {
    headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        (name.eq_ignore_ascii_case("content-length"))
            .then(|| value.trim().parse().ok())
            .flatten()
    })
}
