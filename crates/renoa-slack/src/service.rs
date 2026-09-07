use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use renoa_kernel::AgentId;
use renoa_local::AgentRecord;
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    Config, SlackError,
    api::SlackApi,
    config::read_token,
    ingress, socket,
    store::{Binding, Store},
    worker::Worker,
};

#[derive(Default)]
pub(crate) struct Active(Mutex<Option<(Uuid, CancellationToken)>>);

impl Active {
    pub(crate) async fn register(&self, id: Uuid, cancellation: CancellationToken) {
        *self.0.lock().await = Some((id, cancellation));
    }
    pub(crate) async fn clear(&self) {
        *self.0.lock().await = None;
    }
    pub(crate) async fn cancel(&self, id: Uuid) {
        if let Some((active, token)) = self.0.lock().await.as_ref()
            && *active == id
        {
            token.cancel();
        }
    }
}

/// Runs the Slack operator until shutdown or a required task fails.
///
/// # Errors
/// Returns configuration, authentication, Host, storage, or supervision failures.
pub async fn run(config: Config, shutdown: CancellationToken) -> Result<(), SlackError> {
    let bot = read_token(&config.bot_token_file, "xoxb-")?;
    let app = read_token(&config.app_token_file, "xapp-")?;
    let api = Arc::new(SlackApi::new(bot, app)?);
    let identity = api.identity().await?;
    validate_identity(&identity)?;
    let host = config.host()?;
    config.preflight().await?;
    let host_id = host.host_id().await?;
    let agent_id = AgentId::from_uuid(config.agent_id);
    let profile = renoa_local::AgentProfileId::new(renoa_local::ARCEE_PROFILE_ID)
        .map_err(renoa_local::LocalHostError::from)?;
    let store = Store::open(
        &config.data_directory,
        &Binding {
            host_id,
            agent_id: config.agent_id,
            team: &identity.team,
            bot: &identity.user,
            user: &config.allowed_user_id,
            workspace: &config.workspace,
        },
    )?;
    host.ensure_agent(AgentRecord {
        id: agent_id,
        profile,
        name: "Arcee".to_owned(),
        created_by: None,
    })
    .await?;
    let active = Arc::new(Active::default());
    let wake = Arc::new(Notify::new());
    let channel_wake = Arc::new(Notify::new());
    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn(
        crate::channels::Channels {
            host: host.clone(),
            store: store.clone(),
            api: Arc::clone(&api),
            bot: identity.user.clone(),
            user: config.allowed_user_id.clone(),
            shutdown: shutdown.clone(),
            wake: Arc::clone(&channel_wake),
        }
        .run(),
    );
    tasks.spawn(
        crate::routines::Routines {
            host: host.clone(),
            store: store.clone(),
            api: Arc::clone(&api),
            shutdown: shutdown.clone(),
        }
        .run(),
    );
    tasks.spawn(socket::run(socket::Receiver {
        host: host.clone(),
        api: Arc::clone(&api),
        store: store.clone(),
        active: Arc::clone(&active),
        wake: Arc::clone(&wake),
        shutdown: shutdown.clone(),
        team: identity.team,
        bot: identity.user,
        user: config.allowed_user_id,
    }));
    tasks.spawn(
        Worker {
            host,
            agent_id,
            workspace: config.workspace,
            api,
            store,
            active,
            wake,
            shutdown: shutdown.clone(),
            session: None,
            channel_wake,
        }
        .run(),
    );
    eprintln!("Slack operator started: Host {host_id}, Agent {agent_id}");
    supervise(tasks, shutdown).await
}

async fn supervise(
    mut tasks: tokio::task::JoinSet<Result<(), SlackError>>,
    shutdown: CancellationToken,
) -> Result<(), SlackError> {
    let first = tokio::select! {
        () = shutdown.cancelled() => None,
        result = tasks.join_next() => result,
    };
    shutdown.cancel();
    // All tasks observe shutdown; the worker records cancellation and joins its
    // progress publisher before releasing its kernel and surface ownership.
    let mut failure = match first {
        Some(Ok(result)) => result.err(),
        Some(Err(error)) => Some(error.into()),
        None => None,
    };
    while let Some(result) = tasks.join_next().await {
        let error = match result {
            Ok(result) => result.err(),
            Err(error) => Some(error.into()),
        };
        if failure.is_none() {
            failure = error;
        }
    }
    if let Some(error) = failure {
        return Err(error);
    }
    Ok(())
}

pub(crate) fn now_ms() -> Result<i64, SlackError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| SlackError::Invalid(e.to_string()))?
        .as_millis();
    i64::try_from(millis)
        .map_err(|_| SlackError::Invalid("system clock exceeds supported range".to_owned()))
}

pub(crate) async fn pause(shutdown: &CancellationToken, duration: Duration) {
    tokio::select! { () = shutdown.cancelled() => {}, () = tokio::time::sleep(duration) => {} }
}

fn validate_identity(identity: &crate::api::Identity) -> Result<(), SlackError> {
    if !ingress::valid_id(&identity.team, b"T")
        || !ingress::valid_id(&identity.user, b"UW")
        || !ingress::valid_id(&identity.bot, b"B")
    {
        return Err(SlackError::Invalid(
            "Slack credential does not identify a workspace bot".to_owned(),
        ));
    }
    Ok(())
}
