//! Turns a gateway message into a queued turn for the right agent.
//!
//! A message in a bound channel, or in a thread of one, goes to that
//! channel's agent; any other goes to the default agent. Each queued message
//! keeps a description of where it was written, which is submitted with it.

use std::time::Duration;

use tokio::sync::Notify;

use crate::{
    DiscordError,
    api::DiscordApi,
    ingress,
    places::{self, Place},
    snowflake::Snowflake,
    store::{Enqueue, SurfaceStore},
};

/// How long the gateway waits for Discord to describe an unseen channel.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);

/// What the gateway needs to accept a message.
pub(crate) struct Inbox<'a> {
    pub(crate) store: &'a SurfaceStore,
    pub(crate) api: &'a DiscordApi,
    pub(crate) wake: &'a Notify,
    pub(crate) guild_id: &'a Snowflake,
    pub(crate) operator_user_id: &'a Snowflake,
}

/// Queues one `MESSAGE_CREATE` payload when it is a turn. An unreadable
/// payload is ignored, since Discord's event stream is external input.
pub(crate) async fn accept(
    inbox: &Inbox<'_>,
    bot_user_id: Option<&str>,
    payload: &[u8],
) -> Result<(), DiscordError> {
    let Some(bot_user_id) = bot_user_id else {
        return Ok(());
    };
    let route = match ingress::route(payload) {
        Ok(route) => route,
        Err(error) => {
            eprintln!("renoa-discord: ignored unreadable Discord message: {error}");
            return Ok(());
        }
    };
    let in_guild = route.guild_id.as_deref() == Some(inbox.guild_id.as_str());
    let place = if in_guild {
        Some(locate(inbox, &route.channel_id).await?)
    } else {
        None
    };
    let in_thread = route.in_thread
        || place
            .as_ref()
            .is_some_and(|place| place.thread_parent_id.is_some());
    let replies_to_bot = match route.reference_id.as_deref() {
        Some(message_id) => inbox.store.has_reply(message_id)?,
        None => false,
    };
    let active_conversation = inbox.store.is_bound(&route.channel_id)?
        || (in_thread && inbox.store.has_conversation(&route.channel_id)?);
    let addressed = match ingress::addressed(
        payload,
        &Snowflake::parse(bot_user_id)?,
        inbox.guild_id,
        inbox.operator_user_id,
        replies_to_bot,
        active_conversation,
    ) {
        Ok(Some(addressed)) => addressed,
        Ok(None) => return Ok(()),
        Err(error) => {
            eprintln!("renoa-discord: ignored unreadable Discord message: {error}");
            return Ok(());
        }
    };
    let context = match &place {
        Some(place) => {
            let parent = match &place.thread_parent_id {
                Some(parent_id) => inbox.store.place(parent_id)?,
                None => None,
            };
            places::describe(Some(inbox.guild_id.as_str()), place, parent.as_ref())
        }
        None => places::describe(None, &unknown(&route.channel_id), None),
    };
    match inbox.store.enqueue(
        &addressed.message_id,
        &addressed.channel_id,
        &addressed.author_id,
        &addressed.canonical,
        &addressed.prompt,
        Some(&context),
    )? {
        Enqueue::Fresh => inbox.wake.notify_one(),
        Enqueue::Duplicate => {}
    }
    Ok(())
}

/// The directory's entry for a server channel, asking Discord once for one it
/// has not seen. If Discord cannot say, the message is placed by its own
/// channel alone and the next message asks again.
async fn locate(inbox: &Inbox<'_>, channel_id: &str) -> Result<Place, DiscordError> {
    if let Some(place) = inbox.store.place(channel_id)? {
        return Ok(place);
    }
    let failure = match tokio::time::timeout(LOOKUP_TIMEOUT, inbox.api.channel(channel_id)).await {
        Ok(Ok(channel)) if channel.id.as_str() == channel_id => {
            let place = Place::of(&channel);
            inbox.store.remember_place(&place)?;
            return Ok(place);
        }
        Ok(Ok(_)) => "Discord described a different channel".to_owned(),
        Ok(Err(error)) => error.to_string(),
        Err(_) => "Discord did not answer in time".to_owned(),
    };
    renoa_telemetry::event(
        "renoa.discord",
        "warn",
        "channel_lookup_failed",
        &serde_json::json!({ "channel_id": channel_id, "error": failure }),
    );
    Ok(unknown(channel_id))
}

fn unknown(channel_id: &str) -> Place {
    Place {
        channel_id: channel_id.to_owned(),
        name: None,
        thread_parent_id: None,
    }
}

#[cfg(test)]
#[path = "routing_tests.rs"]
mod tests;
