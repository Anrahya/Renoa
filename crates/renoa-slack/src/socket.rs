use std::{sync::Arc, time::Duration};

use futures_util::{SinkExt as _, StreamExt as _};
use tokio::sync::Notify;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};
use tokio_util::sync::CancellationToken;

use crate::{
    SlackError,
    api::{ApiError, SlackApi},
    ingress::{self, Envelope},
    service::{Active, now_ms, pause},
    store::Store,
};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

pub(crate) struct Receiver {
    pub(crate) host: renoa_local::LocalHost,
    pub(crate) api: Arc<SlackApi>,
    pub(crate) store: Store,
    pub(crate) active: Arc<Active>,
    pub(crate) wake: Arc<Notify>,
    pub(crate) shutdown: CancellationToken,
    pub(crate) team: String,
    pub(crate) bot: String,
    pub(crate) user: String,
}

pub(crate) async fn run(receiver: Receiver) -> Result<(), SlackError> {
    let mut attempt = 0_u32;
    while !receiver.shutdown.is_cancelled() {
        let result = tokio::select! {
            () = receiver.shutdown.cancelled() => return Ok(()),
            result = connect(&receiver.api) => result,
        };
        match result {
            Ok(mut socket) => {
                attempt = 0;
                receive(&receiver, &mut socket).await?;
            }
            Err(ApiError::Rejected(reason)) => return Err(ApiError::Rejected(reason).into()),
            Err(ApiError::RateLimited(delay)) => {
                pause(&receiver.shutdown, delay).await;
                continue;
            }
            Err(_) => eprintln!("Slack Socket Mode connection unavailable; reconnecting"),
        }
        attempt = attempt.saturating_add(1);
        pause(
            &receiver.shutdown,
            Duration::from_secs(1_u64 << attempt.min(5)),
        )
        .await;
    }
    Ok(())
}

async fn connect(api: &SlackApi) -> Result<Socket, ApiError> {
    let url = api.connection_url().await?;
    let config = WebSocketConfig::default()
        .max_message_size(Some(1024 * 1024))
        .max_frame_size(Some(1024 * 1024));
    match tokio::time::timeout(
        Duration::from_secs(15),
        connect_async_with_config(url, Some(config), false),
    )
    .await
    {
        Ok(Ok((socket, _))) => Ok(socket),
        _ => Err(ApiError::Unknown("Socket Mode handshake failed".to_owned())),
    }
}

async fn receive(receiver: &Receiver, socket: &mut Socket) -> Result<(), SlackError> {
    let mut heartbeat = tokio::time::interval(Duration::from_secs(20));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_received = tokio::time::Instant::now();
    loop {
        let message = tokio::select! {
            () = receiver.shutdown.cancelled()=>return Ok(()),
            _ = heartbeat.tick()=>{
                if last_received.elapsed()>Duration::from_secs(90) { return Ok(()); }
                if !send_frame(socket,Message::Ping(Vec::new().into())).await { return Ok(()); }
                continue;
            }
            message=socket.next()=>message,
        };
        last_received = tokio::time::Instant::now();
        match message {
            Some(Ok(Message::Text(text))) => {
                let envelope: Envelope = serde_json::from_str(&text)?;
                if envelope.kind == "disconnect" {
                    return Ok(());
                }
                if let Some(ack) = receiver.admit_envelope(envelope).await?
                    && !send_frame(socket, Message::Text(ack.into())).await
                {
                    return Ok(());
                }
            }
            Some(Ok(Message::Ping(bytes))) => {
                if !send_frame(socket, Message::Pong(bytes)).await {
                    return Ok(());
                }
            }
            Some(Ok(Message::Pong(_) | Message::Binary(_) | Message::Frame(_))) => {}
            Some(Ok(Message::Close(_)) | Err(_)) | None => return Ok(()),
        }
    }
}

impl Receiver {
    pub(crate) async fn admit_envelope(
        &self,
        envelope: Envelope,
    ) -> Result<Option<String>, SlackError> {
        let Some(id) = envelope.id else {
            return Ok(None);
        };
        if id.is_empty() || id.len() > 256 {
            return Err(SlackError::Invalid(
                "invalid Socket Mode envelope identity".to_owned(),
            ));
        }
        if envelope.kind == "events_api" {
            let payload = envelope
                .payload
                .ok_or_else(|| SlackError::Invalid("missing Slack event payload".to_owned()))?;
            if let Some(input) = ingress::parse(payload, &self.team, &self.user, &self.bot)? {
                let selection = self.select_agent(&input.text).await?;
                let admission = self
                    .store
                    .admit_with_agent(input, now_ms()?, selection)
                    .await?;
                if let Some(target) = admission.cancel_target {
                    self.active.cancel(target).await;
                }
                if admission.queued {
                    self.wake.notify_one();
                }
            }
        }
        // The caller sends this acknowledgement only after durable admission.
        Ok(Some(serde_json::to_string(
            &serde_json::json!({"envelope_id":id}),
        )?))
    }
}

async fn send_frame(socket: &mut Socket, message: Message) -> bool {
    matches!(
        tokio::time::timeout(Duration::from_secs(5), socket.send(message)).await,
        Ok(Ok(()))
    )
}

#[cfg(test)]
#[path = "socket_tests.rs"]
mod tests;
