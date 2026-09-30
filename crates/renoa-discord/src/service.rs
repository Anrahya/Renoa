use std::sync::Arc;

use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::{
    DiscordError,
    api::{ApiError, DiscordApi},
    config::Rcp,
    gateway,
    progress::{self, Progress},
    rcp,
    snowflake::Snowflake,
    store::{Outbound, SurfaceStore},
};

pub(crate) struct Surface {
    pub(crate) guild_id: Snowflake,
    pub(crate) operator_user_id: Snowflake,
    pub(crate) token: String,
    pub(crate) data_directory: std::path::PathBuf,
    pub(crate) rcp: Rcp,
}

/// Runs the gateway, the coordinator link, and reply delivery until one stops.
pub(crate) async fn run(
    surface: Surface,
    agent_id: uuid::Uuid,
    api: DiscordApi,
    shutdown: CancellationToken,
) -> Result<(), DiscordError> {
    let store = SurfaceStore::open(&surface.data_directory)?;
    store.bind_identity(&surface.guild_id, &surface.operator_user_id, agent_id)?;
    store.recover()?;
    let store = Arc::new(store);
    let turns = Arc::new(Notify::new());
    let deliveries = Arc::new(Notify::new());
    let api = Arc::new(api);
    let mut tasks = tokio::task::JoinSet::new();
    {
        let (store, turns, shutdown, api) = (
            Arc::clone(&store),
            Arc::clone(&turns),
            shutdown.clone(),
            Arc::clone(&api),
        );
        let guild_id = surface.guild_id.clone();
        let operator_user_id = surface.operator_user_id.clone();
        let token = surface.token.clone();
        tasks.spawn(async move {
            gateway::maintain(
                &api,
                &store,
                &turns,
                &shutdown,
                &guild_id,
                &operator_user_id,
                &token,
            )
            .await
        });
    }
    let (progress, progress_updates) = Progress::channel();
    tasks.spawn(progress::run(
        Arc::clone(&api),
        Arc::clone(&store),
        progress_updates,
        shutdown.clone(),
    ));
    tasks.spawn(rcp::maintain(
        rcp::Link {
            endpoint: surface.rcp.endpoint,
            credentials: surface.rcp.credentials,
            store: Arc::clone(&store),
            turns: Arc::clone(&turns),
            deliveries: Arc::clone(&deliveries),
            progress,
        },
        shutdown.clone(),
    ));
    {
        let (store, deliveries, shutdown, api) = (
            Arc::clone(&store),
            Arc::clone(&deliveries),
            shutdown.clone(),
            Arc::clone(&api),
        );
        tasks.spawn(async move { post_replies(&api, &store, &deliveries, &shutdown).await });
    }
    let first = tokio::select! {
        () = shutdown.cancelled() => None,
        result = tasks.join_next() => result,
    };
    shutdown.cancel();
    let mut failure = match first {
        Some(Ok(result)) => result.err(),
        Some(Err(error)) => Some(DiscordError::Task(error)),
        None => None,
    };
    while let Some(result) = tasks.join_next().await {
        if failure.is_none()
            && let Ok(Err(error)) = result
        {
            failure = Some(error);
        }
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

async fn post_replies(
    api: &DiscordApi,
    store: &SurfaceStore,
    wake: &Notify,
    shutdown: &CancellationToken,
) -> Result<(), DiscordError> {
    loop {
        if shutdown.is_cancelled() {
            return Ok(());
        }
        if let Some(outbound) = store.next_outbound()? {
            deliver(api, store, &outbound, shutdown).await?;
            continue;
        }
        tokio::select! {
            () = shutdown.cancelled() => return Ok(()),
            () = wake.notified() => {}
        }
    }
}

async fn deliver(
    api: &DiscordApi,
    store: &SurfaceStore,
    outbound: &Outbound,
    shutdown: &CancellationToken,
) -> Result<(), DiscordError> {
    store.mark_sending(&outbound.command_id, outbound.chunk)?;
    let reply_to = outbound.reply_to.as_deref();
    match api
        .create_message(&outbound.channel_id, &outbound.body, reply_to)
        .await
    {
        Ok(reply_id) => {
            let reply_id = Snowflake::parse(&reply_id)?;
            store.mark_sent(&outbound.command_id, outbound.chunk, &reply_id)?;
            Ok(())
        }
        Err(ApiError::RateLimited(delay)) => {
            store.release_sending(&outbound.command_id, outbound.chunk)?;
            tokio::select! {
                () = shutdown.cancelled() => Ok(()),
                () = tokio::time::sleep(delay) => Ok(()),
            }
        }
        Err(ApiError::Unknown(_)) => {
            store.mark_unknown(&outbound.command_id, outbound.chunk)?;
            Ok(())
        }
        Err(ApiError::Unauthorized) => {
            store.release_sending(&outbound.command_id, outbound.chunk)?;
            Err(ApiError::Unauthorized.into())
        }
        Err(error) => {
            renoa_telemetry::event(
                "renoa.discord",
                "warn",
                "reply_rejected",
                &serde_json::json!({
                    "command_id": outbound.command_id,
                    "chunk": outbound.chunk,
                    "error": error.to_string(),
                }),
            );
            store.mark_failed(&outbound.command_id, outbound.chunk)?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    use super::deliver;
    use crate::{api::DiscordApi, snowflake::Snowflake, store::SurfaceStore};

    fn snowflake(value: &str) -> Snowflake {
        Snowflake::parse(value).expect("snowflake")
    }

    #[tokio::test]
    async fn rejected_credentials_leave_the_reply_retryable() {
        let directory = tempfile::tempdir().expect("temp directory");
        let store = SurfaceStore::open(directory.path()).expect("store");
        store
            .bind_identity(&snowflake("10"), &snowflake("20"), Uuid::new_v4())
            .expect("identity");
        store
            .enqueue(
                &snowflake("101"),
                &snowflake("202"),
                &snowflake("20"),
                b"message",
                "task",
                None,
            )
            .expect("enqueue");
        store.answer_locally("101", "answer").expect("answer");
        let outbound = store
            .next_outbound()
            .expect("next outbound")
            .expect("outbound");

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).await.expect("request");
            stream
                .write_all(
                    b"HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                )
                .await
                .expect("response");
        });
        let api = DiscordApi::with_origin("bad-token".to_owned(), format!("http://{address}"))
            .expect("api");
        let error = deliver(&api, &store, &outbound, &CancellationToken::new())
            .await
            .expect_err("unauthorized");
        assert!(error.to_string().contains("token"), "{error}");
        let retry = store
            .next_outbound()
            .expect("retryable outbound")
            .expect("pending reply");
        assert_eq!(retry.reply_to.as_deref(), Some("101"));
        assert_eq!(retry.chunk, 0);
    }
}
