//! Turns a gateway message into a queued turn for the right agent.
//!
//! A message in a bound channel, or in a thread of one, goes to that
//! channel's agent; any other goes to the default agent. Each queued message
//! keeps a description of where it was written, which is submitted with it.

use std::{
    collections::HashMap,
    sync::{Mutex, PoisonError},
    time::{Duration, Instant},
};

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
/// How long a channel Discord could not describe is not asked about again.
const RETRY_LOOKUP_AFTER: Duration = Duration::from_secs(60);

/// What the gateway needs to accept a message.
pub(crate) struct Inbox<'a> {
    pub(crate) store: &'a SurfaceStore,
    pub(crate) api: &'a DiscordApi,
    pub(crate) wake: &'a Notify,
    pub(crate) guild_id: &'a Snowflake,
    pub(crate) operator_user_id: &'a Snowflake,
    /// When each channel's last lookup failed.
    pub(crate) failed_lookups: Mutex<HashMap<String, Instant>>,
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
    let mut place = if in_guild {
        inbox.store.place(&route.channel_id)?
    } else {
        None
    };
    // A thread routes by its parent, so an unseen one is looked up before
    // deciding whether its message is a turn; any other channel only once it is.
    if in_guild && place.is_none() && route.in_thread {
        place = locate(inbox, &route.channel_id).await?;
    }
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
    if in_guild && place.is_none() {
        place = locate(inbox, &route.channel_id).await?;
    }
    let context = match &place {
        Some(place) => {
            let parent = match &place.thread_parent_id {
                Some(parent_id) => inbox.store.place(parent_id)?,
                None => None,
            };
            places::describe(Some(inbox.guild_id.as_str()), place, parent.as_ref())
        }
        None => places::describe(
            in_guild.then_some(inbox.guild_id.as_str()),
            &unknown(&route.channel_id),
            None,
        ),
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

/// Asks Discord about a server channel the directory has not seen, and
/// remembers the answer. If Discord cannot say, the message is placed by its
/// own channel alone, and the channel is not asked about again for a minute.
async fn locate(inbox: &Inbox<'_>, channel_id: &str) -> Result<Option<Place>, DiscordError> {
    let recently_failed = |failures: &HashMap<String, Instant>| {
        failures
            .get(channel_id)
            .is_some_and(|at| at.elapsed() < RETRY_LOOKUP_AFTER)
    };
    if recently_failed(
        &inbox
            .failed_lookups
            .lock()
            .unwrap_or_else(PoisonError::into_inner),
    ) {
        return Ok(None);
    }
    let failure = match tokio::time::timeout(LOOKUP_TIMEOUT, inbox.api.channel(channel_id)).await {
        Ok(Ok(channel)) if channel.id.as_str() == channel_id => {
            let place = Place::of(&channel);
            inbox.store.remember_place(&place)?;
            return Ok(Some(place));
        }
        Ok(Ok(_)) => "Discord described a different channel".to_owned(),
        Ok(Err(error)) => error.to_string(),
        Err(_) => "Discord did not answer in time".to_owned(),
    };
    inbox
        .failed_lookups
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(channel_id.to_owned(), Instant::now());
    renoa_telemetry::event(
        "renoa.discord",
        "warn",
        "channel_lookup_failed",
        &serde_json::json!({ "channel_id": channel_id, "error": failure }),
    );
    Ok(None)
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
