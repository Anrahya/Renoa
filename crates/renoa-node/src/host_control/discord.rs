//! Owner Discord setup: connect the Host's bot, then route channels to agents.

use renoa_discord::{DiscordBindingRequest, DiscordConnectRequest, DiscordControl, DiscordError};
use renoa_local::LocalHost;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::reply::{CONFLICT, Reply, UNAVAILABLE, log};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Inspection {
    bot_token: String,
}

pub(super) fn status(discord: &DiscordControl) -> Reply {
    reply(
        discord.status(),
        "Discord settings cannot be read. Saved settings are preserved.",
    )
}

pub(super) async fn channels(discord: &DiscordControl) -> Reply {
    reply(
        discord.channels().await,
        "Discord channels could not be read. Retry when Discord is reachable.",
    )
}

/// Reads what a token reaches. The token is not stored.
pub(super) async fn inspect(discord: &DiscordControl, request: Inspection) -> Reply {
    reply(
        discord.inspect(request.bot_token).await,
        "Discord could not be reached. Retry the token check.",
    )
}

pub(super) async fn connect(
    discord: &DiscordControl,
    host: &LocalHost,
    request: DiscordConnectRequest,
) -> Reply {
    let operation_id = request.operation_id;
    receipt(
        operation_id,
        discord.connect(host, request).await,
        "The Discord connection could not be confirmed. Refresh to see whether it was saved.",
    )
}

pub(super) async fn bind(
    discord: &DiscordControl,
    host: &LocalHost,
    request: DiscordBindingRequest,
) -> Reply {
    let operation_id = request.operation_id;
    receipt(
        operation_id,
        discord.bind(host, request).await,
        "Channel binding could not be confirmed. Check bot access, then retry the saved request.",
    )
}

fn receipt<T: Serialize>(
    operation_id: Uuid,
    result: Result<T, DiscordError>,
    unknown: &str,
) -> Reply {
    reply(
        result.map(|record| serde_json::json!({ "operation_id": operation_id, "record": record })),
        unknown,
    )
}

/// Discord's own rejection explains itself; anything else leaves the outcome unknown.
fn reply<T: Serialize>(result: Result<T, DiscordError>, unknown: &str) -> Reply {
    match result {
        Ok(value) => Reply::json(&value),
        Err(DiscordError::Invalid(reason)) => Reply::failure(CONFLICT, "discord_rejected", &reason),
        Err(error) => {
            log("discord", &error.to_string());
            Reply::failure(UNAVAILABLE, "discord_unavailable", unknown)
        }
    }
}
