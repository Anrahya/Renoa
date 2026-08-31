use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use super::{ApiError, TelegramApi, safe_description};

#[test]
fn remote_errors_are_definite_but_transport_errors_are_not() {
    let remote = ApiError::Remote {
        method: "sendRichMessage",
        code: 400,
        description: "bad markdown".to_owned(),
        retry_after: None,
    };
    assert!(remote.is_definite_rejection());
    assert!(remote.is_invalid_rich_text());
    let transport = ApiError::Transport {
        method: "sendRichMessage",
        category: "connection",
        detail: "connection reset".to_owned(),
    };
    assert!(!transport.is_definite_rejection());
}

#[test]
fn server_descriptions_are_bounded_and_strip_control_bytes() {
    let input = format!("bad\0{}", "x".repeat(2000));
    let safe = safe_description(Some(&input), "unrelated-token");
    assert!(!safe.contains('\0'));
    assert_eq!(safe.chars().count(), 1024);
}

#[tokio::test]
async fn server_descriptions_cannot_echo_the_bot_token() {
    let (origin, _requests, server) = fake_server(vec![
        r#"{"ok":false,"error_code":401,"description":"rejected 123:must-not-leak"}"#,
    ])
    .await;
    let api = TelegramApi::for_test(&origin, "123:must-not-leak").expect("test API");

    let rendered = api
        .get_me()
        .await
        .expect_err("remote rejection")
        .to_string();

    assert!(!rendered.contains("must-not-leak"));
    assert!(rendered.contains("[REDACTED]"));
    server.await.expect("fake server task");
}

#[tokio::test]
async fn polling_and_native_drafts_cross_the_real_http_boundary_exactly() {
    let (origin, mut requests, server) = fake_server(vec![
        r#"{"ok":true,"result":[{"update_id":8},{"update_id":7,"stopped_message_generation":{"chat":{"id":42,"type":"private"},"draft_id":99}}]}"#,
        r#"{"ok":true,"result":true}"#,
    ])
    .await;
    let api = TelegramApi::for_test(&origin, "123:test-secret").expect("test API");

    let updates = api.updates(7).await.expect("poll updates");
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0].update.id, 7);
    assert_eq!(updates[1].update.id, 8);
    assert_eq!(
        updates[0]
            .update
            .stopped_message_generation
            .as_ref()
            .expect("native stop")
            .draft_id,
        99
    );
    api.send_draft(
        42,
        Some(5),
        99,
        Some("Checking extensions…"),
        Some("Found Exa."),
    )
    .await
    .expect("send draft");

    let poll = requests.recv().await.expect("poll request");
    assert!(poll.starts_with("POST /bot123:test-secret/getUpdates HTTP/1.1"));
    let poll_body = request_body(&poll);
    assert_eq!(poll_body["offset"], 7);
    assert_eq!(
        poll_body["allowed_updates"],
        serde_json::json!(["message", "stopped_message_generation"])
    );
    let draft = requests.recv().await.expect("draft request");
    assert!(draft.starts_with("POST /bot123:test-secret/sendRichMessageDraft HTTP/1.1"));
    let draft_body = request_body(&draft);
    assert_eq!(draft_body["draft_id"], 99);
    assert_eq!(draft_body["message_thread_id"], 5);
    assert_eq!(draft_body["can_stop"], true);
    assert_eq!(draft_body["keep_on_stop"], true);
    assert_eq!(
        draft_body["rich_message"]["blocks"],
        serde_json::json!([
            {"type": "thinking", "text": "Checking extensions…"},
            {"type": "paragraph", "text": "Found Exa."}
        ])
    );
    server.await.expect("fake server task");
}

#[tokio::test]
async fn transport_failures_never_render_the_bot_token_or_request_url() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake server");
    let origin = format!(
        "http://{}",
        listener.local_addr().expect("listener address")
    );
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept request");
        drop(stream);
    });
    let api = TelegramApi::for_test(&origin, "123:must-not-leak").expect("test API");
    let rendered = api
        .get_me()
        .await
        .expect_err("connection must fail")
        .to_string();
    assert!(!rendered.contains("must-not-leak"));
    assert!(!rendered.contains(&origin));
    server.await.expect("fake server task");
}

async fn fake_server(
    responses: Vec<&'static str>,
) -> (
    String,
    tokio::sync::mpsc::Receiver<String>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake server");
    let origin = format!(
        "http://{}",
        listener.local_addr().expect("listener address")
    );
    let (sender, receiver) = tokio::sync::mpsc::channel(responses.len());
    let task = tokio::spawn(async move {
        for body in responses {
            let (mut stream, _) = listener.accept().await.expect("accept request");
            let request = read_request(&mut stream).await;
            sender.send(request).await.expect("record request");
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write response");
        }
    });
    (origin, receiver, task)
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> String {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    let header_end = loop {
        let count = stream.read(&mut buffer).await.expect("read request");
        assert_ne!(count, 0, "request ended before headers");
        request.extend_from_slice(&buffer[..count]);
        if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8(request[..header_end].to_vec()).expect("UTF-8 headers");
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().expect("content length"))
        })
        .expect("content-length header");
    while request.len() < header_end + content_length {
        let count = stream.read(&mut buffer).await.expect("read request body");
        assert_ne!(count, 0, "request ended before body");
        request.extend_from_slice(&buffer[..count]);
    }
    String::from_utf8(request).expect("UTF-8 request")
}

fn request_body(request: &str) -> serde_json::Value {
    let (_, body) = request.split_once("\r\n\r\n").expect("request body");
    serde_json::from_str(body).expect("JSON request body")
}
