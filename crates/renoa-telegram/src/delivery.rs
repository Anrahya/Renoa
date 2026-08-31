use std::{sync::Arc, time::Duration};

use crate::{
    api::TelegramApi,
    log,
    store::{DeliveryItem, StoreError, SurfaceStore},
};

const MAX_RICH_MESSAGE_UTF16_UNITS: usize = 32_768;

pub(crate) enum DeliveryProgress {
    Complete,
    More,
    RetryAfter(Duration),
}

pub(crate) async fn deliver(
    api: &Arc<TelegramApi>,
    store: &SurfaceStore,
    item: DeliveryItem,
) -> Result<DeliveryProgress, StoreError> {
    let chunks = split_message(&item.text);
    let Some(chunk) = chunks.get(item.cursor) else {
        return Err(StoreError::Invalid(format!(
            "update {} delivery cursor exceeds its result",
            item.update_id
        )));
    };
    store.mark_delivering(item.update_id).await?;
    let sent = match api
        .send_markdown(item.topic.chat_id, item.topic.thread_id, chunk)
        .await
    {
        Ok(sent) => Ok(sent),
        Err(error) if error.is_invalid_rich_text() => {
            api.send_plain(item.topic.chat_id, item.topic.thread_id, chunk)
                .await
        }
        Err(error) => Err(error),
    };
    match sent {
        Ok(sent) => {
            let complete = item.cursor + 1 == chunks.len();
            store
                .mark_chunk_delivered(item.update_id, item.cursor, sent.message_id, complete)
                .await?;
            log::event(
                "info",
                "final_chunk_delivered",
                &serde_json::json!({
                    "update_id": item.update_id,
                    "chunk": item.cursor,
                    "complete": complete,
                    "telegram_message_id": sent.message_id,
                }),
            );
            Ok(if complete {
                DeliveryProgress::Complete
            } else {
                DeliveryProgress::More
            })
        }
        Err(error) if error.retry_after().is_some() => {
            let delay = Duration::from_secs(error.retry_after().unwrap_or(1).clamp(1, 60));
            store.defer_delivery(item.update_id).await?;
            log::event(
                "warn",
                "final_delivery_rate_limited",
                &serde_json::json!({"update_id": item.update_id, "retry_after_ms": delay.as_millis()}),
            );
            Ok(DeliveryProgress::RetryAfter(delay))
        }
        Err(error) if error.is_definite_rejection() => {
            let message = error.to_string();
            store
                .mark_delivery_failed(item.update_id, message.clone())
                .await?;
            log::event(
                "error",
                "final_delivery_rejected",
                &serde_json::json!({"update_id": item.update_id, "error": message}),
            );
            Ok(DeliveryProgress::Complete)
        }
        Err(error) => {
            let message = error.to_string();
            store
                .mark_delivery_unknown(item.update_id, message.clone())
                .await?;
            log::event(
                "error",
                "final_delivery_unknown",
                &serde_json::json!({"update_id": item.update_id, "error": message}),
            );
            Ok(DeliveryProgress::Complete)
        }
    }
}

fn split_message(text: &str) -> Vec<String> {
    let text = if text.is_empty() { "Done." } else { text };
    let mut chunks = Vec::new();
    let mut remaining = text;
    loop {
        let hard_end = utf16_prefix(remaining, MAX_RICH_MESSAGE_UTF16_UNITS);
        if hard_end == remaining.len() {
            chunks.push(remaining.to_owned());
            break;
        }
        let candidate = &remaining[..hard_end];
        let split = candidate
            .rfind('\n')
            .filter(|index| {
                candidate[..*index].encode_utf16().count() >= MAX_RICH_MESSAGE_UTF16_UNITS / 2
            })
            .map_or(hard_end, |index| index + 1);
        chunks.push(remaining[..split].to_owned());
        remaining = &remaining[split..];
    }
    chunks
}

fn utf16_prefix(text: &str, limit: usize) -> usize {
    let mut units = 0;
    for (index, character) in text.char_indices() {
        let next = units + character.len_utf16();
        if next > limit {
            return index;
        }
        units = next;
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::{MAX_RICH_MESSAGE_UTF16_UNITS, split_message};

    #[test]
    fn rich_message_chunks_preserve_every_utf8_character_in_order() {
        let input = format!(
            "{}\n{}",
            "a".repeat(MAX_RICH_MESSAGE_UTF16_UNITS - 10),
            "🦀".repeat(MAX_RICH_MESSAGE_UTF16_UNITS + 20)
        );
        let chunks = split_message(&input);
        assert!(chunks.len() > 1);
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.encode_utf16().count() <= MAX_RICH_MESSAGE_UTF16_UNITS)
        );
        assert_eq!(chunks.concat(), input);
    }

    #[test]
    fn an_empty_result_still_has_one_durable_message() {
        assert_eq!(split_message(""), vec!["Done."]);
    }
}
