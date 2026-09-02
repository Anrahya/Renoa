use std::{sync::Arc, time::Duration};

use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{
    api::TelegramApi,
    ingress::Topic,
    log,
    store::{StoreError, SurfaceStore},
};

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ActionLink {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) message: String,
    pub(crate) button: String,
    pub(crate) url: Url,
    pub(crate) expires_at_ms: Option<i64>,
    pub(crate) sensitive_fragment: bool,
}

impl ActionLink {
    pub(crate) fn new(
        id: String,
        title: String,
        message: String,
        button: String,
        url: Url,
        expires_at_ms: Option<i64>,
    ) -> Self {
        Self {
            id,
            title,
            message,
            button,
            url,
            expires_at_ms,
            sensitive_fragment: false,
        }
    }

    pub(crate) fn sensitive(
        id: String,
        title: String,
        message: String,
        button: String,
        url: Url,
        expires_at_ms: Option<i64>,
    ) -> Self {
        Self {
            id,
            title,
            message,
            button,
            url,
            expires_at_ms,
            sensitive_fragment: true,
        }
    }

    fn display_text(&self) -> String {
        format!("{}\n\n{}", self.title, self.message)
    }
}

pub(crate) async fn deliver(
    api: &Arc<TelegramApi>,
    store: &SurfaceStore,
    update_id: i64,
    topic: Topic,
    action: ActionLink,
    cancellation: &CancellationToken,
) -> Result<(), StoreError> {
    loop {
        if !store
            .begin_action_delivery(update_id, topic, action.clone())
            .await?
        {
            return Ok(());
        }
        let text = action.display_text();
        let sent = match api
            .send_action(
                topic.chat_id,
                topic.thread_id,
                &text,
                &action.button,
                action.url.as_str(),
            )
            .await
        {
            Ok(sent) => Ok(sent),
            Err(error) if error.is_invalid_rich_text() => {
                let fallback = format!("{text}\n\n{}", action.url);
                api.send_plain(topic.chat_id, topic.thread_id, &fallback)
                    .await
            }
            Err(error) => Err(error),
        };
        match sent {
            Ok(sent) => {
                store
                    .mark_action_delivered(&action.id, sent.message_id)
                    .await?;
                log::event(
                    "info",
                    "surface_action_delivered",
                    &serde_json::json!({
                        "update_id": update_id,
                        "action_id": action.id,
                        "telegram_message_id": sent.message_id,
                    }),
                );
                return Ok(());
            }
            Err(error) if error.retry_after().is_some() => {
                let delay = Duration::from_secs(error.retry_after().unwrap_or(1).clamp(1, 60));
                store.defer_action_delivery(&action.id).await?;
                tokio::select! {
                    () = cancellation.cancelled() => return Ok(()),
                    () = tokio::time::sleep(delay) => {}
                }
            }
            Err(error) if error.is_definite_rejection() => {
                let detail = error.to_string();
                store
                    .mark_action_delivery_failed(&action.id, detail.clone())
                    .await?;
                log::event(
                    "error",
                    "surface_action_rejected",
                    &serde_json::json!({"action_id": action.id, "error": detail}),
                );
                return Ok(());
            }
            Err(error) => {
                let detail = error.to_string();
                store
                    .mark_action_delivery_unknown(&action.id, detail.clone())
                    .await?;
                log::event(
                    "error",
                    "surface_action_delivery_unknown",
                    &serde_json::json!({"action_id": action.id, "error": detail}),
                );
                return Ok(());
            }
        }
    }
}
