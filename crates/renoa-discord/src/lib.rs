//! Discord surface mail log.
//!
//! Discord surface for an existing Host agent.
//!
//! Guild members address the bot by mentioning it. The bound operator can also
//! send a direct message. The surface does not create agents.

mod api;
mod config;
mod error;
mod gateway;
mod ingress;
#[cfg(test)]
mod live_test;
mod service;
mod snowflake;
mod store;

pub use error::DiscordError;
pub use store::Admission;

use std::path::Path;

use config::Config;

/// Opens the Discord surface store and binds the guild, operator, and existing agent.
///
/// # Errors
///
/// Returns configuration, filesystem, or database failures. A stored guild,
/// operator, or agent that differs from the launch file is rejected and left
/// unchanged.
pub fn bind(config_path: &Path) -> Result<(), DiscordError> {
    let config = Config::read(config_path)?;
    open_bound(&config)?;
    Ok(())
}

/// Records one Discord message in the surface store.
///
/// The same message id and payload returns [`Admission::Duplicate`]. A reused
/// message id with different content is rejected and does not replace the row.
/// Any author can be recorded. Who may address the bot is decided when a
/// Discord event is read, not by this store.
///
/// # Errors
///
/// Returns configuration, filesystem, or database failures, including a missing
/// identity binding or conflicting message content. Invalid message fields are
/// rejected before the store is created.
pub fn admit(
    config_path: &Path,
    message_id: &str,
    channel_id: &str,
    author_id: &str,
    canonical: &[u8],
) -> Result<Admission, DiscordError> {
    if canonical.is_empty() {
        return Err(DiscordError::Invalid(
            "Discord message canonical payload must not be empty".to_owned(),
        ));
    }
    let message = store::IncomingMessage {
        message_id: snowflake::Snowflake::parse(message_id)?,
        channel_id: snowflake::Snowflake::parse(channel_id)?,
        author_id: snowflake::Snowflake::parse(author_id)?,
        canonical: canonical.to_vec(),
    };
    let config = Config::read(config_path)?;
    let store = open_bound(&config)?;
    store.admit(message)
}

/// Connects to Discord and serves mentions and operator direct messages.
///
/// The process uses the agent id already stored for this surface. It does not
/// create an agent. A missing agent fails before the Discord database is opened.
///
/// # Errors
///
/// Returns configuration, Discord, Host, or database failures. Ctrl-C shuts the
/// connection down.
pub async fn run(config_path: &Path) -> Result<(), DiscordError> {
    let config = Config::read(config_path)?;
    let host = config.open_host()?;
    let api = api::DiscordApi::new(config.token.clone())?;
    let shutdown = tokio_util::sync::CancellationToken::new();
    let service = service::run(
        service::Surface {
            host,
            agent_id: config.agent_id,
            workspace: config.workspace.clone(),
            guild_id: config.guild_id.clone(),
            operator_user_id: config.operator_user_id.clone(),
            token: config.token,
            data_directory: config.data_directory,
        },
        api,
        shutdown.clone(),
    );
    tokio::pin!(service);
    tokio::select! {
        result = &mut service => result,
        result = tokio::signal::ctrl_c() => {
            result?;
            shutdown.cancel();
            service.await
        }
    }
}

fn open_bound(config: &Config) -> Result<store::SurfaceStore, DiscordError> {
    let store = store::SurfaceStore::open(&config.data_directory)?;
    store.bind_identity(&config.guild_id, &config.operator_user_id, config.agent_id)?;
    Ok(store)
}

#[cfg(all(test, unix))]
mod launch_tests {
    use std::{fs, os::unix::fs::PermissionsExt as _, path::Path};

    use super::{admit, bind};

    const AGENT: &str = "11111111-1111-4111-8111-111111111111";

    fn write_config(path: &Path, data: &Path, token: &Path, guild: &str, agent: &str) {
        let workspace = path.parent().expect("config directory").join("workspace");
        let bridge = path.parent().expect("config directory").join("bridge");
        let auth = path.parent().expect("config directory").join("auth");
        fs::create_dir_all(&workspace).expect("workspace");
        fs::write(&bridge, "").expect("bridge");
        fs::write(&auth, "").expect("auth");
        fs::write(
            path,
            serde_json::to_vec(&serde_json::json!({
                "data_directory": data,
                "guild_id": guild,
                "operator_user_id": "20",
                "agent_id": agent,
                "bot_token_file": token,
                "workspace": workspace,
                "model_bridge": bridge,
                "model_auth_store": auth,
                "model": "fixture-model",
                "provider": "opencode-go",
            }))
            .expect("config json"),
        )
        .expect("config");
    }

    fn private_token(path: &Path) {
        fs::write(path, "discord-token\n").expect("token");
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("mode");
    }

    #[test]
    fn launch_binds_one_existing_agent() {
        let root = tempfile::tempdir().expect("temp directory");
        let token = root.path().join("token");
        private_token(&token);
        let data = root.path().join("data");
        let config = root.path().join("discord.json");
        write_config(&config, &data, &token, "10", AGENT);

        bind(&config).expect("first launch");
        bind(&config).expect("same launch");

        let changed = root.path().join("changed.json");
        write_config(
            &changed,
            &data,
            &token,
            "10",
            "22222222-2222-4222-8222-222222222222",
        );
        let error = bind(&changed).expect_err("agent change");
        assert!(error.to_string().contains("differs"), "{error}");
    }

    #[test]
    fn a_bad_message_is_rejected_before_the_store_exists() {
        let root = tempfile::tempdir().expect("temp directory");
        let token = root.path().join("token");
        private_token(&token);
        let data = root.path().join("data");
        let config = root.path().join("discord.json");
        write_config(&config, &data, &token, "10", AGENT);

        let error = admit(&config, "0", "202", "20", b"hello").expect_err("bad message id");
        assert!(error.to_string().contains("snowflake"), "{error}");
        assert!(
            !data.exists(),
            "a rejected message must not create the data directory"
        );
    }

    #[test]
    fn a_shared_token_file_is_rejected_before_the_store_exists() {
        let root = tempfile::tempdir().expect("temp directory");
        let token = root.path().join("token");
        fs::write(&token, "discord-token\n").expect("token");
        fs::set_permissions(&token, fs::Permissions::from_mode(0o644)).expect("mode");
        let data = root.path().join("data");
        let config = root.path().join("discord.json");
        write_config(&config, &data, &token, "10", AGENT);

        let error = bind(&config).expect_err("shared token");
        assert!(error.to_string().contains("group or other"), "{error}");
        assert!(
            !data.exists(),
            "a rejected launch must not create the data directory"
        );
    }
}
