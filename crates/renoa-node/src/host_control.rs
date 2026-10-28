//! The owner's control of the Host, answered by the node that executes it.
//!
//! The coordinator authenticates the owner and relays each call as a method
//! and a path under `/v1/host`; this module is the only place the operations,
//! their paths and their body limits are named. A change is retried by the
//! caller under the `operation_id` the Host records a receipt for, so a call
//! lost on the way is recovered, not repeated.

mod agents;
mod automations;
mod discord;
mod profile;
mod reply;

use std::sync::Arc;

use renoa_control::HostMethod;
use renoa_discord::DiscordControl;
use renoa_local::{HostObserver, LocalHost};
use renoa_protocol::PrincipalId;

use crate::bridge::NodeError;
use reply::{METHOD_NOT_ALLOWED, NOT_FOUND, UNAVAILABLE, decode, log};

pub(crate) use reply::Reply;

/// Most operations take a short JSON object.
const BODY_LIMIT: usize = 4096;
/// Agent creation and a profile edit carry instructions and documents.
const LARGE_BODY_LIMIT: usize = 64 * 1024;

pub(crate) struct HostControl {
    host: Arc<LocalHost>,
    observer: HostObserver,
    discord: DiscordControl,
}

#[derive(Debug, PartialEq, Eq)]
enum Operation {
    Observe,
    AgentOptions,
    AgentDefinition(String),
    AgentCreate,
    ProfileRead,
    ProfileReplace,
    DiscordStatus,
    DiscordInspect,
    DiscordConnect,
    DiscordChannels,
    DiscordBind,
    AutomationEnabled(String),
}

/// Every operation at a path, by method. The one table of the Host's control
/// surface; a path it does not list is unknown.
fn operations_at(segments: &[&str]) -> Vec<(HostMethod, Operation)> {
    use HostMethod::{Get, Post, Put};
    use Operation as Op;
    match segments {
        [] => vec![(Get, Op::Observe)],
        ["agents", "options"] => vec![(Get, Op::AgentOptions)],
        ["agents", id] => vec![(Get, Op::AgentDefinition((*id).to_owned()))],
        ["agents"] => vec![(Post, Op::AgentCreate)],
        ["profile"] => vec![(Get, Op::ProfileRead), (Put, Op::ProfileReplace)],
        ["discord"] => vec![(Get, Op::DiscordStatus)],
        ["discord", "inspection"] => vec![(Post, Op::DiscordInspect)],
        ["discord", "connection"] => vec![(Post, Op::DiscordConnect)],
        ["discord", "channels"] => vec![(Get, Op::DiscordChannels)],
        ["discord", "bindings"] => vec![(Post, Op::DiscordBind)],
        ["automations", id, "enabled"] => vec![(Post, Op::AutomationEnabled((*id).to_owned()))],
        _ => Vec::new(),
    }
}

fn resolve(method: HostMethod, path: &str) -> Result<Operation, Reply> {
    let not_found = || {
        Reply::failure(
            NOT_FOUND,
            "not_found",
            "This address is not part of the Host's control.",
        )
    };
    let rest = path.strip_prefix("/v1/host").ok_or_else(not_found)?;
    let segments: Vec<&str> = if rest.is_empty() {
        Vec::new()
    } else {
        rest.strip_prefix('/')
            .ok_or_else(not_found)?
            .split('/')
            .collect()
    };
    let operations = operations_at(&segments);
    if operations.is_empty() {
        return Err(not_found());
    }
    operations
        .into_iter()
        .find_map(|(allowed, operation)| (allowed == method).then_some(operation))
        .ok_or_else(|| {
            Reply::failure(
                METHOD_NOT_ALLOWED,
                "method_not_allowed",
                "This address does not accept that method.",
            )
        })
}

impl HostControl {
    /// Binds the control to the Host this node executes.
    ///
    /// # Errors
    ///
    /// Returns an error when the Host's catalog cannot be observed or its
    /// installation root is invalid.
    pub(crate) fn open(host: Arc<LocalHost>) -> Result<Self, NodeError> {
        let root = host.home().path();
        let observer = HostObserver::open(root)
            .map_err(|error| NodeError::Configuration(error.to_string()))?;
        let discord = DiscordControl::open(root)
            .map_err(|error| NodeError::Configuration(error.to_string()))?;
        Ok(Self {
            host,
            observer,
            discord,
        })
    }

    /// Answers one relayed call. `principal` is the owner the coordinator
    /// authenticated for the node; a body cannot name another.
    pub(crate) async fn handle(
        &self,
        principal: PrincipalId,
        method: HostMethod,
        path: &str,
        body: Option<&str>,
    ) -> Reply {
        let operation = match resolve(method, path) {
            Ok(operation) => operation,
            Err(reply) => return reply,
        };
        // A Host replaced under a running node is never served as the old one.
        if self.host.host_id().await.ok() != Some(self.observer.host_id()) {
            return Reply::failure(
                UNAVAILABLE,
                "host_unavailable",
                "Host records are temporarily unavailable. Showing the last received state.",
            );
        }
        self.run(operation, principal, body)
            .await
            .unwrap_or_else(|reply| reply)
    }

    async fn run(
        &self,
        operation: Operation,
        principal: PrincipalId,
        body: Option<&str>,
    ) -> Result<Reply, Reply> {
        let host = self.host.as_ref();
        let host_id = self.observer.host_id();
        Ok(match operation {
            Operation::Observe => self.observe().await,
            Operation::AgentOptions => agents::options(host).await,
            Operation::AgentDefinition(id) => agents::definition(host, &id).await,
            Operation::AgentCreate => {
                let request = decode(
                    body,
                    LARGE_BODY_LIMIT,
                    "Send a valid agent creation request.",
                )?;
                agents::create(host, host_id, principal, request).await
            }
            Operation::ProfileRead => profile::read(host, principal).await,
            Operation::ProfileReplace => {
                let edit = decode(
                    body,
                    LARGE_BODY_LIMIT,
                    "Send the profile's expected_revision and its complete new content as JSON.",
                )?;
                profile::replace(host, principal, edit).await
            }
            Operation::DiscordStatus => discord::status(&self.discord),
            Operation::DiscordChannels => discord::channels(&self.discord).await,
            Operation::DiscordInspect => {
                let request = decode(body, BODY_LIMIT, DISCORD_HINT)?;
                discord::inspect(&self.discord, request).await
            }
            Operation::DiscordConnect => {
                let request = decode(body, BODY_LIMIT, DISCORD_HINT)?;
                discord::connect(&self.discord, host, request).await
            }
            Operation::DiscordBind => {
                let request = decode(body, BODY_LIMIT, DISCORD_HINT)?;
                discord::bind(&self.discord, host, request).await
            }
            Operation::AutomationEnabled(id) => {
                let request = decode(
                    body,
                    BODY_LIMIT,
                    "Send an operation ID, expected revision, and enabled state as JSON.",
                )?;
                automations::set_enabled(host, host_id, principal, &id, request).await
            }
        })
    }

    async fn observe(&self) -> Reply {
        match self.observer.snapshot().await {
            Ok(snapshot) => Reply::json(&snapshot),
            Err(error) => {
                log("observation", &error.to_string());
                Reply::failure(
                    UNAVAILABLE,
                    "host_unavailable",
                    "Host records are temporarily unavailable. Showing the last received state.",
                )
            }
        }
    }
}

const DISCORD_HINT: &str = "Send a valid Discord request.";

#[cfg(test)]
mod tests;
