//! Discord surface for existing Host agents.
//!
//! The owner connects one bot from the Control Room. Bound guild channels route
//! ordinary messages to their selected Host agent. Other guild messages require
//! a mention or reply; the operator can send a DM. The surface does not create
//! agents or run them: each channel's conversation is an RCP task executed by
//! the node that advertises its agent.

mod actions;
mod api;
mod config;
mod connection;
mod control;
mod discovery;
mod error;
mod gateway;
mod ingress;
#[cfg(test)]
mod live_test;
mod rcp;
mod service;
mod snowflake;
mod store;

pub use actions::{OperatorChannel, SetupDelivery};
pub use control::{
    DiscordBinding, DiscordBindingRequest, DiscordConnectRequest, DiscordControl, DiscordStatus,
};
pub use discovery::{DiscordChannel, DiscordGuild, DiscordInspection};
pub use error::DiscordError;

use std::path::Path;

use config::Config;

/// Opens the Discord surface store and binds the connected guild, operator,
/// and default agent.
///
/// # Errors
///
/// Returns configuration, filesystem, or database failures. A stored guild,
/// operator, or agent that differs from the connection is rejected and left
/// unchanged.
pub fn bind(config_path: &Path) -> Result<(), DiscordError> {
    let config = Config::read(config_path)?;
    open_bound(&config)?;
    Ok(())
}

/// Serves bound channels, mentions, replies, and operator direct messages.
///
/// Messages reach agents through the RCP coordinator. A message whose agent's
/// node is offline is answered as not sent, never held.
///
/// # Errors
///
/// Returns configuration, Discord, coordinator authentication, or database
/// failures. Ctrl-C or SIGTERM shuts the connection down.
pub async fn run(config_path: &Path) -> Result<(), DiscordError> {
    let Config {
        home,
        connection,
        rcp,
    } = Config::read(config_path)?;
    let api = api::DiscordApi::new(connection.bot_token.clone())?;
    let shutdown = tokio_util::sync::CancellationToken::new();
    let service = service::run(
        service::Surface {
            guild_id: connection.guild_id,
            operator_user_id: connection.operator_user_id,
            token: connection.bot_token,
            data_directory: home.path().to_owned(),
            rcp,
        },
        connection.agent_id,
        api,
        shutdown.clone(),
    );
    tokio::pin!(service);
    tokio::select! {
        result = &mut service => result,
        result = stop_signal() => {
            result?;
            shutdown.cancel();
            service.await
        }
    }
}

async fn stop_signal() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut termination =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! { result = tokio::signal::ctrl_c() => result, _ = termination.recv() => Ok(()) }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await
}

fn open_bound(config: &Config) -> Result<store::SurfaceStore, DiscordError> {
    let store = store::SurfaceStore::open(config.home.path())?;
    let connection = &config.connection;
    store.bind_identity(
        &connection.guild_id,
        &connection.operator_user_id,
        connection.agent_id,
    )?;
    Ok(store)
}

#[cfg(all(test, unix))]
mod launch_tests {
    use std::{fs, os::unix::fs::PermissionsExt as _, path::Path};

    use renoa_local::RenoaHome;
    use uuid::Uuid;

    use super::{DiscordConnectRequest, bind, connection::Connection};

    const AGENT: &str = "11111111-1111-4111-8111-111111111111";

    fn write_runtime(path: &Path, home: &Path) {
        let credentials = home.join("credentials/discord-rcp-device.json");
        if let Some(parent) = credentials.parent() {
            fs::create_dir_all(parent).expect("credential directory");
        }
        fs::write(
            &credentials,
            serde_json::to_vec(&serde_json::json!({
                "deviceId": Uuid::new_v4(),
                "credential": "00".repeat(32),
            }))
            .expect("credential json"),
        )
        .expect("credential");
        fs::set_permissions(&credentials, fs::Permissions::from_mode(0o600)).expect("mode");
        fs::write(
            path,
            serde_json::to_vec(&serde_json::json!({
                "home": home,
                "rcp": {"endpoint": "ws://127.0.0.1:9/connect", "credentials": credentials},
            }))
            .expect("runtime json"),
        )
        .expect("runtime");
    }

    fn connect(home: &Path, agent: &str) {
        let home = RenoaHome::at(home).expect("home");
        home.initialize().expect("layout");
        let request = DiscordConnectRequest {
            operation_id: Uuid::new_v4(),
            bot_token: "discord.token".to_owned(),
            guild_id: "10".to_owned(),
            agent_id: Uuid::parse_str(agent).expect("agent"),
        };
        Connection {
            operation_id: request.operation_id,
            bot_name: "Renoa".to_owned(),
            guild_id: crate::snowflake::Snowflake::parse("10").expect("guild"),
            guild_name: "Home".to_owned(),
            operator_user_id: crate::snowflake::Snowflake::parse("20").expect("operator"),
            agent_id: request.agent_id,
            bot_token: request.bot_token.clone(),
        }
        .publish(&home, &request)
        .expect("connection");
    }

    #[test]
    fn launch_binds_the_connected_agent_and_a_manual_reconnect_cannot_retarget_it() {
        let root = tempfile::tempdir().expect("temp directory");
        let home = root.path().join("home");
        let config = root.path().join("discord.json");
        connect(&home, AGENT);
        write_runtime(&config, &home);

        bind(&config).expect("first launch");
        bind(&config).expect("same launch");

        fs::remove_file(home.join("credentials/discord.json")).expect("manual reset");
        connect(&home, "22222222-2222-4222-8222-222222222222");
        let error = bind(&config).expect_err("agent change");
        assert!(error.to_string().contains("differs"), "{error}");
    }

    #[test]
    fn launch_requires_a_connection() {
        let root = tempfile::tempdir().expect("temp directory");
        let home = root.path().join("home");
        RenoaHome::at(&home)
            .expect("home")
            .initialize()
            .expect("layout");
        let config = root.path().join("discord.json");
        write_runtime(&config, &home);

        let error = bind(&config).expect_err("unconnected");
        assert!(error.to_string().contains("Control Room"), "{error}");
        assert!(!home.join("state/surfaces/discord").exists());
    }

    #[test]
    fn a_shared_connection_is_rejected_before_the_store_exists() {
        let root = tempfile::tempdir().expect("temp directory");
        let home = root.path().join("home");
        let config = root.path().join("discord.json");
        connect(&home, AGENT);
        write_runtime(&config, &home);
        fs::set_permissions(
            home.join("credentials/discord.json"),
            fs::Permissions::from_mode(0o644),
        )
        .expect("mode");

        let error = bind(&config).expect_err("shared connection");
        assert!(error.to_string().contains("group or other"), "{error}");
        assert!(
            !home.join("state/surfaces/discord").exists(),
            "a rejected launch must not create the surface store"
        );
    }
}
