//! Turns a gateway message into a queued turn for the right agent.
//!
//! A message in a bound channel, or in a thread of one, goes to that
//! channel's agent; any other goes to the default agent. Each queued message
//! keeps whether the operator or a guest wrote it and a description of where
//! and by whom, which are submitted with it. The operator's `/new` starts a new
//! conversation in the channel instead of becoming a turn.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use renoa_protocol::Author;
use tokio::sync::Notify;

use crate::{
    DiscordError,
    api::DiscordApi,
    ingress,
    places::{self, Place, Sender},
    snowflake::Snowflake,
    store::{Enqueue, SurfaceStore},
};

/// How long the gateway waits for Discord to describe an unseen channel.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a channel Discord could not describe is not asked about again.
const RETRY_LOOKUP_AFTER: Duration = Duration::from_secs(60);

/// Who the gateway wakes for the work it records.
#[derive(Clone, Copy)]
pub(crate) struct Wake<'a> {
    /// The coordinator link, for a queued turn.
    pub(crate) turns: &'a Notify,
    /// Reply delivery, for an answer the surface gives itself.
    pub(crate) replies: &'a Notify,
}

/// What the gateway needs to accept a message.
pub(crate) struct Inbox<'a> {
    pub(crate) store: &'a SurfaceStore,
    pub(crate) api: &'a DiscordApi,
    pub(crate) wake: Wake<'a>,
    pub(crate) guild_id: &'a Snowflake,
    pub(crate) operator_user_id: &'a Snowflake,
    /// When each channel's lookup failed within the last
    /// [`RETRY_LOOKUP_AFTER`]; older entries are dropped as new ones arrive.
    pub(crate) failed_lookups: HashMap<Snowflake, Instant>,
}

/// Queues one `MESSAGE_CREATE` payload when it is a turn. An unreadable
/// payload is ignored, since Discord's event stream is external input.
pub(crate) async fn accept(
    inbox: &mut Inbox<'_>,
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
    let in_guild = route.guild_id.as_ref() == Some(inbox.guild_id);
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
        || (in_thread && inbox.store.has_conversation(route.channel_id.as_str())?);
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
    // Only the operator speaks as the owner; anyone else in a shared channel
    // is a guest, whose turn reads no `USER.md` and runs no tools.
    let author = if &addressed.author_id == inbox.operator_user_id {
        Author::Principal
    } else {
        Author::Guest
    };
    if in_guild && place.is_none() {
        place = locate(inbox, &route.channel_id).await?;
    }
    if author == Author::Principal && addressed.prompt.eq_ignore_ascii_case("/new") {
        if inbox.store.start_conversation(&addressed)? == Enqueue::Fresh {
            inbox.wake.replies.notify_one();
        }
        return Ok(());
    }
    let sender = Sender {
        name: &addressed.author_name,
        author,
    };
    let context = match &place {
        Some(place) => {
            let parent = match &place.thread_parent_id {
                Some(parent_id) => inbox.store.place(parent_id)?,
                None => None,
            };
            let parent_name = parent.as_ref().and_then(|parent| parent.name.as_deref());
            places::describe(Some(inbox.guild_id), place, parent_name, &sender)
        }
        None => places::describe(
            in_guild.then_some(inbox.guild_id),
            &Place::unknown(route.channel_id.clone()),
            None,
            &sender,
        ),
    };
    match inbox
        .store
        .enqueue(&addressed, sender.author, Some(&context))?
    {
        Enqueue::Fresh => inbox.wake.turns.notify_one(),
        Enqueue::Duplicate => {}
    }
    Ok(())
}

/// Asks Discord about a server channel the directory has not seen, and
/// remembers the answer. If Discord cannot say, the message is placed by its
/// own channel alone, and the channel is not asked about again for
/// [`RETRY_LOOKUP_AFTER`].
async fn locate(
    inbox: &mut Inbox<'_>,
    channel_id: &Snowflake,
) -> Result<Option<Place>, DiscordError> {
    if inbox
        .failed_lookups
        .get(channel_id)
        .is_some_and(|at| at.elapsed() < RETRY_LOOKUP_AFTER)
    {
        return Ok(None);
    }
    let lookup = inbox.api.channel(channel_id.as_str());
    let failure = match tokio::time::timeout(LOOKUP_TIMEOUT, lookup).await {
        Ok(Ok(channel)) if &channel.id == channel_id => {
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
        .retain(|_, at| at.elapsed() < RETRY_LOOKUP_AFTER);
    inbox
        .failed_lookups
        .insert(channel_id.clone(), Instant::now());
    renoa_telemetry::event(
        "renoa.discord",
        "warn",
        "channel_lookup_failed",
        &serde_json::json!({ "channel_id": channel_id, "error": failure }),
    );
    Ok(None)
}

#[cfg(test)]
#[path = "routing_tests.rs"]
mod tests;
