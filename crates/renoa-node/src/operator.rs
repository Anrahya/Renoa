//! Plugin setup links reach the Host operator privately, never through the
//! RCP task journal: a credential-setup link carries the key that keeps a
//! relayed credential unreadable to the coordinator.

use std::sync::Arc;

use renoa_agent::AgentEventSink;
use renoa_discord::{OperatorChannel, SetupDelivery};
use renoa_local::RenoaHome;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{node_log, projection::NoopEvents};

/// The event sink for one command's turn and, when the Host has a Discord
/// connection, the delivery that may stop the turn.
pub(crate) fn setup_sink(
    home: &RenoaHome,
    command_id: Uuid,
    cancellation: &CancellationToken,
) -> (Arc<dyn AgentEventSink>, Option<Arc<SetupDelivery>>) {
    match OperatorChannel::open(home) {
        Ok(Some(channel)) => {
            let delivery = channel.setup_delivery(command_id, cancellation.clone());
            (delivery.clone(), Some(delivery))
        }
        Ok(None) => (Arc::new(NoopEvents), None),
        Err(error) => {
            node_log::event(
                "warn",
                "operator_channel_unavailable",
                &serde_json::json!({
                    "command_id": command_id,
                    "error": error.to_string(),
                }),
            );
            (Arc::new(NoopEvents), None)
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use renoa_local::RenoaHome;
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    use super::setup_sink;

    #[test]
    fn setup_links_have_a_private_channel_only_once_discord_is_connected() {
        let files = tempfile::tempdir().expect("temporary directory");
        let home = RenoaHome::at(files.path().join("home")).expect("home");
        home.initialize().expect("home layout");
        let command = Uuid::new_v4();

        let (_, delivery) = setup_sink(&home, command, &CancellationToken::new());
        assert!(delivery.is_none(), "no Discord connection, no delivery");

        let connection = home.discord_connection();
        std::fs::write(
            &connection,
            serde_json::to_vec(&serde_json::json!({
                "operation_id": Uuid::new_v4(),
                "bot_name": "Renoa",
                "guild_id": "10",
                "guild_name": "Home",
                "operator_user_id": "20",
                "agent_id": Uuid::new_v4(),
                "bot_token": "discord.token",
            }))
            .expect("connection json"),
        )
        .expect("write connection");
        std::fs::set_permissions(&connection, std::fs::Permissions::from_mode(0o600))
            .expect("private connection");

        let (_, delivery) = setup_sink(&home, command, &CancellationToken::new());
        assert!(delivery.is_some(), "a connected Host delivers setup links");
    }
}
