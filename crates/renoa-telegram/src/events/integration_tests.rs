use std::sync::Arc;

use renoa_agent::{AgentEvent, AgentEventSink as _, ContentBlock, ToolCall, ToolOutput};
use tempfile::tempdir;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio_util::sync::CancellationToken;

use super::SurfaceEvents;
use crate::{
    actions::ActionLink,
    api::TelegramApi,
    ingress::{InboundKind, ParsedUpdate, Topic},
    store::{PendingAction, SurfaceStore},
};

#[tokio::test]
async fn an_authorization_update_crosses_the_durable_store_and_permanent_message_boundary() {
    let directory = tempdir().expect("temporary Telegram action fixture");
    let workspace = directory.path().join("workspace");
    std::fs::create_dir(&workspace).expect("create workspace");
    let store = SurfaceStore::open(directory.path()).expect("open surface store");
    store
        .bind_identity(9, 42, &workspace)
        .await
        .expect("bind surface identity");
    let topic = Topic {
        chat_id: 42,
        thread_id: Some(7),
    };
    store
        .admit(ParsedUpdate {
            update_id: 1,
            canonical: b"authorization-update".to_vec(),
            topic: Some(topic),
            message_id: Some(10),
            kind: InboundKind::Prompt("connect exa".to_owned()),
        })
        .await
        .expect("admit prompt");
    let PendingAction::Execute(work) = store
        .next_action()
        .await
        .expect("load work")
        .expect("queued work")
    else {
        panic!("prompt did not become executable")
    };
    store
        .mark_running(work.update_id)
        .await
        .expect("start work");
    let (origin, request, server) = action_server().await;
    let cancellation = CancellationToken::new();
    let events = SurfaceEvents::for_turn(
        Arc::new(TelegramApi::for_test(&origin, "9:test").expect("test API")),
        store.clone(),
        work.update_id,
        topic,
        work.request_id,
        cancellation,
    );
    let update = ToolOutput {
        content: vec![ContentBlock::text(
            r#"{"status":"authorization_required","connection":"plugin.digest.default","display_name":"notion","authorization_url":"https://provider.example/authorize?state=one","expires_at_ms":9999999999999,"message":"Open it"}"#,
        )],
        details: None,
        is_error: false,
    };

    events
        .emit(AgentEvent::ToolExecutionUpdate {
            call: tool_call(),
            update: update.clone(),
        })
        .await;

    let request = request.await.expect("receive action request");
    assert!(request.starts_with("POST /bot9:test/sendRichMessage HTTP/1.1"));
    assert!(request.contains("https://provider.example/authorize?state=one"));
    let action_id = format!("{}/call-1/authorization", work.request_id);
    let action = ActionLink::new(
        action_id,
        "Authorize Notion".to_owned(),
        "Open the provider page to finish connecting this MCP.".to_owned(),
        "Authorize".to_owned(),
        url::Url::parse("https://provider.example/authorize?state=one")
            .expect("valid provider URL"),
        Some(9_999_999_999_999),
    );
    assert!(
        !store
            .begin_action_delivery(work.update_id, topic, action)
            .await
            .expect("delivered action is idempotent")
    );
    server.await.expect("action server task");
}

fn tool_call() -> ToolCall {
    ToolCall {
        id: "call-1".to_owned(),
        name: "extension_manage".to_owned(),
        arguments: serde_json::json!({}),
        thought_signature: None,
        namespace: None,
    }
}

async fn action_server() -> (
    String,
    tokio::sync::oneshot::Receiver<String>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind action server");
    let origin = format!("http://{}", listener.local_addr().expect("server address"));
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept action request");
        let request = read_request(&mut stream).await;
        sender.send(request).expect("record action request");
        let body = r#"{"ok":true,"result":{"message_id":81}}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream
            .write_all(response.as_bytes())
            .await
            .expect("write action response");
    });
    (origin, receiver, server)
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
