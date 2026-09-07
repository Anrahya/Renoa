use std::{sync::Arc, time::Duration};

use renoa_agent::{AgentEvent, AgentEventSink, AssistantDelta, BoxFuture};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::{
    actions::{Action, Actions},
    api::SlackApi,
    ingress::Topic,
};

mod preview;
use preview::Preview;

pub(crate) struct Progress {
    latest: watch::Sender<Preview>,
    actions: Option<Actions>,
    action_error: tokio::sync::Mutex<Option<String>>,
}

impl Progress {
    pub(crate) fn start(
        api: Arc<SlackApi>,
        topic: Topic,
        ts: Option<String>,
        stop: CancellationToken,
        actions: Option<Actions>,
    ) -> (Arc<Self>, tokio::task::JoinHandle<()>) {
        let (latest, mut receiver) = watch::channel(Preview::default());
        let task = tokio::spawn(async move {
            // 48 updates/minute leaves headroom below chat.update's Tier 3
            // floor of 50/minute for final delivery and command responses.
            let mut interval = tokio::time::interval(Duration::from_millis(1250));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut retry_pending = false;
            let mut delivered = String::new();
            loop {
                tokio::select! {
                    () = stop.cancelled() => break,
                    _ = interval.tick() => {
                        let Ok(changed) = receiver.has_changed() else { break };
                        if !changed && !retry_pending { continue; }
                        let text = receiver.borrow_and_update().render();
                        let Some(ts) = &ts else { continue; };
                        if text.trim().is_empty() || (!retry_pending && text == delivered) { continue; }
                        // The worker joins this task before final delivery, so a
                        // delayed progress update cannot replace the final answer.
                        match api.update(&topic, ts, &text).await {
                            Ok(()) => { retry_pending = false; delivered = text; },
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
                actions,
                action_error: tokio::sync::Mutex::new(None),
            }),
            task,
        )
    }

    fn status(&self, status: String) {
        self.latest.send_modify(|preview| preview.status = status);
    }

    pub(crate) async fn action_error(&self) -> Option<String> {
        self.action_error.lock().await.take()
    }
}

impl AgentEventSink for Progress {
    fn emit(&self, event: AgentEvent) -> BoxFuture<'_, ()> {
        match event {
            AgentEvent::MessageStart {
                role: renoa_agent::MessageRole::Assistant,
            } => {
                self.latest.send_modify(Preview::start_message);
            }
            AgentEvent::MessageUpdate {
                delta: AssistantDelta::Text { text },
                ..
            } => {
                self.latest.send_modify(|preview| preview.append(&text));
            }
            AgentEvent::ModelRequestStart { .. } | AgentEvent::MessageAbort => {
                self.status("Thinking…".to_owned());
            }
            AgentEvent::ModelRetryAttempt {
                next_attempt,
                category,
                delay_ms,
                ..
            } => {
                let reason = if category == renoa_agent::ModelErrorKind::RateLimited {
                    "The model provider is rate limiting requests."
                } else {
                    "The model request hit a temporary error."
                };
                self.status(format!(
                    "{reason} Retrying in {}s (attempt {next_attempt}). Use !cancel to stop.",
                    delay_ms.div_ceil(1000)
                ));
            }
            AgentEvent::ToolExecutionStart { call } => {
                self.status(format!("Using {}…", call.name));
            }
            AgentEvent::ToolExecutionUpdate { call, update } => {
                if call.name == "extension_manage"
                    && let Some(action) = Action::parse(&update)
                    && let Some(actions) = &self.actions
                {
                    self.status("Account setup needs your action. Check the separate setup message in this conversation.".to_owned());
                    return Box::pin(async move {
                        if let Err(error) = actions.deliver(&call.id, action).await {
                            let message = format!("{error}");
                            self.status(message.clone());
                            *self.action_error.lock().await = Some(message);
                            actions.cancellation.cancel();
                        }
                    });
                }
            }
            // Reasoning and raw provider/tool payloads are never published.
            _ => {}
        }
        Box::pin(async {})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, response::IntoResponse as _, routing::post};

    #[tokio::test]
    async fn tool_and_retry_events_keep_text_visible_until_the_next_message_has_text() {
        let (latest, receiver) = watch::channel(Preview::default());
        let progress = Progress {
            latest,
            actions: None,
            action_error: tokio::sync::Mutex::new(None),
        };
        progress.emit(text_event("I will check the logs.")).await;
        progress
            .emit(AgentEvent::ToolExecutionStart {
                call: renoa_agent::ToolCall {
                    id: "call-1".to_owned(),
                    name: "read".to_owned(),
                    arguments: serde_json::json!({}),
                    thought_signature: None,
                    namespace: None,
                },
            })
            .await;
        assert_eq!(
            receiver.borrow().render(),
            "I will check the logs.\n\nUsing read…"
        );
        progress.emit(AgentEvent::MessageAbort).await;
        progress
            .emit(AgentEvent::MessageStart {
                role: renoa_agent::MessageRole::Assistant,
            })
            .await;
        progress.emit(text_event("")).await;
        assert_eq!(
            receiver.borrow().render(),
            "I will check the logs.\n\nThinking…"
        );
        progress.emit(text_event("The logs ")).await;
        progress.emit(text_event("look healthy.")).await;
        assert_eq!(receiver.borrow().render(), "The logs look healthy.");
    }

    #[tokio::test]
    async fn long_unicode_replies_keep_their_beginning_and_bound_the_preview() {
        let (latest, receiver) = watch::channel(Preview::default());
        let progress = Progress {
            latest,
            actions: None,
            action_error: tokio::sync::Mutex::new(None),
        };
        progress.emit(text_event("Beginning: ")).await;
        progress.emit(text_event(&"界".repeat(4000))).await;
        let preview = receiver.borrow().render();
        assert!(preview.starts_with("Beginning: "));
        assert!(preview.contains("full response will appear when finished"));
        assert!(preview.chars().count() < 3600);
        progress.emit(text_event("later content")).await;
        assert_eq!(receiver.borrow().render(), preview);
    }

    fn text_event(text: &str) -> AgentEvent {
        AgentEvent::MessageUpdate {
            content_index: 0,
            delta: AssistantDelta::Text {
                text: text.to_owned(),
            },
        }
    }

    #[tokio::test]
    async fn model_retry_progress_explains_wait_without_publishing_diagnostics() {
        let (latest, receiver) = watch::channel(Preview::default());
        let progress = Progress {
            latest,
            actions: None,
            action_error: tokio::sync::Mutex::new(None),
        };
        for (category, reason) in [
            (renoa_agent::ModelErrorKind::RateLimited, "rate limiting"),
            (renoa_agent::ModelErrorKind::Network, "temporary error"),
        ] {
            progress
                .emit(AgentEvent::ModelRetryAttempt {
                    invocation_id: "private-invocation".to_owned(),
                    attempt: 1,
                    next_attempt: 2,
                    category,
                    delay_ms: 5_100,
                    cause_code: Some("private-diagnostic".to_owned()),
                })
                .await;
            let message = receiver.borrow().render();
            assert!(message.contains(reason));
            assert!(message.contains("Retrying in 6s (attempt 2)"));
            assert!(message.contains("!cancel"));
            assert!(!message.contains("private"));
        }
    }

    #[tokio::test]
    async fn progress_retries_rate_limits_and_transient_errors_without_new_events() {
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
            Some("1.000001".to_owned()),
            stop.clone(),
            None,
        );
        let link = "Waiting for the provider.";
        progress.status(link.to_owned());
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
}
