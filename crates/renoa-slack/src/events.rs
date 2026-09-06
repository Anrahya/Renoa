use std::{sync::Arc, time::Duration};

use renoa_agent::{AgentEvent, AgentEventSink, AssistantDelta, BoxFuture, ToolOutput};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::{api::SlackApi, ingress::Topic};

pub(crate) struct Progress {
    latest: watch::Sender<String>,
    private_conversation: bool,
}

impl Progress {
    pub(crate) fn start(
        api: Arc<SlackApi>,
        topic: Topic,
        ts: String,
        stop: CancellationToken,
    ) -> (Arc<Self>, tokio::task::JoinHandle<()>) {
        let private_conversation = topic.channel.starts_with('D');
        let (latest, mut receiver) = watch::channel("Working…".to_owned());
        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(2));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut retry_pending = false;
            loop {
                tokio::select! {
                    () = stop.cancelled() => break,
                    _ = interval.tick() => {
                        let Ok(changed) = receiver.has_changed() else { break };
                        if !changed && !retry_pending { continue; }
                        let text = receiver.borrow_and_update().clone();
                        // The worker joins this task before final delivery, so a
                        // delayed progress update cannot replace the final answer.
                        match api.update(&topic, &ts, &text).await {
                            Ok(()) => retry_pending = false,
                            Err(crate::api::ApiError::RateLimited(delay)) => {
                                retry_pending = true;
                                tokio::select! { () = stop.cancelled() => break, () = tokio::time::sleep(delay) => {} }
                            }
                            Err(crate::api::ApiError::Unknown(error)) => {
                                retry_pending = true;
                                eprintln!("Slack progress update will retry: {error}");
                                tokio::select! { () = stop.cancelled() => break, () = tokio::time::sleep(Duration::from_secs(5)) => {} }
                            }
                            Err(error @ crate::api::ApiError::Rejected(_)) => {
                                retry_pending = false;
                                eprintln!("Slack progress update failed: {error}");
                            }
                        }
                    }
                }
            }
        });
        (
            Arc::new(Self {
                latest,
                private_conversation,
            }),
            task,
        )
    }

    pub(crate) fn quiet() -> Arc<Self> {
        Arc::new(Self {
            latest: watch::channel(String::new()).0,
            private_conversation: false,
        })
    }
}

impl AgentEventSink for Progress {
    fn emit(&self, event: AgentEvent) -> BoxFuture<'_, ()> {
        match event {
            AgentEvent::MessageStart {
                role: renoa_agent::MessageRole::Assistant,
            } => {
                self.latest.send_replace(String::new());
            }
            AgentEvent::MessageUpdate {
                delta: AssistantDelta::Text { text },
                ..
            } => {
                self.latest.send_modify(|current| {
                    current.push_str(&text);
                    *current = tail(current, 3500);
                });
            }
            AgentEvent::ModelRequestStart { .. } | AgentEvent::MessageAbort => {
                self.latest.send_replace("Thinking…".to_owned());
            }
            AgentEvent::ToolExecutionStart { call } => {
                self.latest.send_replace(format!("Using {}…", call.name));
            }
            AgentEvent::ToolExecutionUpdate { call, update } => {
                if call.name == "extension_manage"
                    && let Some(link) = authorization_link(&update)
                {
                    self.latest.send_replace(if self.private_conversation { link } else {
                        "Account setup needs a private conversation. Use !cancel, then continue setup in a DM with Arcee.".to_owned()
                    });
                }
            }
            // Reasoning and raw provider/tool payloads are never published.
            _ => {}
        }
        Box::pin(async {})
    }
}

fn authorization_link(update: &ToolOutput) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Action {
        status: String,
        authorization_url: Option<String>,
        setup_url: Option<String>,
    }
    let text = update.content.iter().find_map(|block| match block {
        renoa_agent::ContentBlock::Text { text, .. } => Some(text.as_str()),
        renoa_agent::ContentBlock::Image { .. } => None,
    })?;
    let action: Action = serde_json::from_str(text).ok()?;
    let link = match action.status.as_str() {
        "authorization_required" => action.authorization_url?,
        "credential_required" => action.setup_url?,
        _ => return None,
    };
    let url = url::Url::parse(&link).ok()?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || link.len() > 3000
    {
        return None;
    }
    Some(format!(
        "Open this secure page to finish connecting the capability. Keep credentials out of chat.\n{link}"
    ))
}

fn tail(text: &str, limit: usize) -> String {
    let count = text.chars().count();
    if count <= limit {
        return text.to_owned();
    }
    text.chars().skip(count - limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, response::IntoResponse as _, routing::post};
    use renoa_agent::{ContentBlock, ToolCall};

    #[tokio::test]
    async fn setup_link_retries_rate_limits_and_transient_errors_without_new_events() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let attempts = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&attempts);
        let (sent, mut received) = tokio::sync::mpsc::unbounded_channel();
        let app = Router::new().route(
            "/chat.update",
            post(move |Json(body): Json<serde_json::Value>| {
                let attempt = counter.fetch_add(1, Ordering::SeqCst);
                let sent = sent.clone();
                async move {
                    sent.send(body).expect("record update");
                    match attempt {
                        0 => (
                            axum::http::StatusCode::TOO_MANY_REQUESTS,
                            [("retry-after", "1")],
                            "slow down",
                        )
                            .into_response(),
                        1 => Json(serde_json::json!({"ok":false,"error":"internal_error"}))
                            .into_response(),
                        _ => Json(serde_json::json!({"ok":true})).into_response(),
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let origin = url::Url::parse(&format!(
            "http://{}/",
            listener.local_addr().expect("address")
        ))
        .expect("origin");
        let server_stop = CancellationToken::new();
        let shutdown = server_stop.clone();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
        });
        let api = Arc::new(
            SlackApi::with_origin("xoxb-test".to_owned(), "xapp-test".to_owned(), origin)
                .expect("api"),
        );
        let stop = CancellationToken::new();
        let (progress, task) = Progress::start(
            api,
            Topic {
                channel: "D1".to_owned(),
                thread: String::new(),
            },
            "1.000001".to_owned(),
            stop.clone(),
        );
        let link = "https://renoa.live/setup#private-key";
        progress.latest.send_replace(link.to_owned());
        tokio::time::timeout(Duration::from_secs(15), async {
            for _ in 0..3 {
                let body = received.recv().await.expect("update attempted");
                assert_eq!(body["text"], link);
                assert_eq!(body["ts"], "1.000001");
            }
        })
        .await
        .expect("link retried without another event");
        stop.cancel();
        task.await.expect("progress stopped");
        server_stop.cancel();
        server.await.expect("server task").expect("server exit");
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn setup_urls_are_only_visible_in_private_conversations() {
        for (status, key) in [
            ("authorization_required", "authorization_url"),
            ("credential_required", "setup_url"),
        ] {
            for private in [false, true] {
                let (latest, receiver) = watch::channel(String::new());
                let progress = Progress {
                    latest,
                    private_conversation: private,
                };
                let link = "https://renoa.live/setup#secret-key";
                progress
                    .emit(AgentEvent::ToolExecutionUpdate {
                        call: ToolCall {
                            id: "tool-1".to_owned(),
                            name: "extension_manage".to_owned(),
                            arguments: serde_json::json!({}),
                            thought_signature: None,
                            namespace: None,
                        },
                        update: ToolOutput {
                            content: vec![ContentBlock::text(
                                serde_json::json!({"status":status,key:link}).to_string(),
                            )],
                            details: None,
                            is_error: false,
                        },
                    })
                    .await;
                let text = receiver.borrow();
                assert_eq!(text.contains(link), private);
                if !private {
                    assert!(text.contains("DM"));
                }
            }
        }
    }
}
