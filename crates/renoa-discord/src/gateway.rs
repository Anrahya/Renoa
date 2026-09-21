use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio::sync::Notify;
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};
use tokio_util::sync::CancellationToken;

use crate::{
    DiscordError,
    api::{ApiError, DiscordApi},
    ingress,
    snowflake::Snowflake,
    store::{Enqueue, GatewayCursor, SurfaceStore},
};

pub(crate) const INTENTS: i64 = (1 << 0) | (1 << 9) | (1 << 12);

#[derive(Debug)]
pub(crate) struct SocketState {
    pub(crate) sequence: Option<i64>,
    pub(crate) session_id: Option<String>,
    pub(crate) resume_url: Option<String>,
    pub(crate) interval_ms: u64,
    pub(crate) bot_user_id: Option<String>,
}

#[derive(Debug)]
pub(crate) enum Step {
    Send(Value),
    Message(Vec<u8>),
    Reconnect { fresh: bool },
    Ready,
}

impl SocketState {
    #[must_use]
    pub(crate) fn new(
        session_id: Option<String>,
        resume_url: Option<String>,
        sequence: Option<i64>,
    ) -> Self {
        Self {
            sequence,
            session_id,
            resume_url,
            interval_ms: 41_250,
            bot_user_id: None,
        }
    }

    pub(crate) fn receive(&mut self, text: &str, token: &str) -> Result<Step, DiscordError> {
        let frame: Value = serde_json::from_str(text)?;
        let opcode = frame.get("op").and_then(Value::as_i64).ok_or_else(|| {
            DiscordError::Invalid("Discord gateway frame has no opcode".to_owned())
        })?;
        if opcode == 0
            && let Some(sequence) = frame.get("s").and_then(Value::as_i64)
        {
            self.sequence = Some(sequence);
        }
        match opcode {
            10 => Ok(Step::Send(self.hello_reply(token, &frame)?)),
            1 => Ok(Step::Send(self.heartbeat())),
            7 => Ok(Step::Reconnect { fresh: false }),
            9 => {
                let resumable = frame.get("d").and_then(Value::as_bool).unwrap_or(false);
                if !resumable {
                    self.session_id = None;
                    self.resume_url = None;
                }
                Ok(Step::Reconnect { fresh: !resumable })
            }
            0 => self.dispatch(&frame),
            _ => Ok(Step::Send(Value::Null)),
        }
    }

    pub(crate) fn heartbeat(&self) -> Value {
        json!({ "op": 1, "d": self.sequence })
    }

    fn hello_reply(&mut self, token: &str, frame: &Value) -> Result<Value, DiscordError> {
        self.interval_ms = frame
            .pointer("/d/heartbeat_interval")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                DiscordError::Invalid("Discord gateway hello has no heartbeat interval".to_owned())
            })?;
        if let (Some(session_id), Some(_)) = (&self.session_id, &self.resume_url) {
            return Ok(json!({
                "op": 6,
                "d": { "token": token, "session_id": session_id, "seq": self.sequence }
            }));
        }
        Ok(json!({
            "op": 2,
            "d": {
                "token": token,
                "intents": INTENTS,
                "properties": { "os": "linux", "browser": "renoa", "device": "renoa" }
            }
        }))
    }

    fn dispatch(&mut self, frame: &Value) -> Result<Step, DiscordError> {
        match frame.get("t").and_then(Value::as_str) {
            Some("READY") => {
                self.session_id = frame
                    .pointer("/d/session_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                self.resume_url = frame
                    .pointer("/d/resume_gateway_url")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                self.bot_user_id = frame
                    .pointer("/d/user/id")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                if self.session_id.is_none()
                    || self.resume_url.is_none()
                    || self.bot_user_id.is_none()
                {
                    return Err(DiscordError::Invalid(
                        "Discord READY omitted the session, resume URL, or bot user".to_owned(),
                    ));
                }
                Ok(Step::Ready)
            }
            Some("MESSAGE_CREATE") => {
                let payload = frame.get("d").cloned().ok_or_else(|| {
                    DiscordError::Invalid("Discord MESSAGE_CREATE has no payload".to_owned())
                })?;
                Ok(Step::Message(serde_json::to_vec(&payload)?))
            }
            _ => Ok(Step::Send(Value::Null)),
        }
    }
}

pub(crate) async fn maintain(
    api: &DiscordApi,
    store: &SurfaceStore,
    wake: &Notify,
    shutdown: &CancellationToken,
    guild_id: &Snowflake,
    operator_user_id: &Snowflake,
    token: &str,
) -> Result<(), DiscordError> {
    let mut fresh = false;
    loop {
        if shutdown.is_cancelled() {
            return Ok(());
        }
        let cursor = store.load_gateway()?;
        let mut state = if fresh {
            SocketState::new(None, None, None)
        } else {
            SocketState::new(cursor.session_id, cursor.resume_url, cursor.sequence)
        };
        state.bot_user_id = store.bot_user_id()?;
        let url = if !fresh
            && let Some(url) = state.resume_url.clone()
            && allowed_gateway(&url)
        {
            url
        } else {
            match api.gateway_url().await {
                Ok(url) if allowed_gateway(&url) => url,
                Ok(_) => {
                    return Err(DiscordError::Invalid(
                        "Discord gateway URL is not a Discord gateway host".to_owned(),
                    ));
                }
                Err(ApiError::Unauthorized) => {
                    return Err(DiscordError::Invalid(
                        "Discord refused the bot token".to_owned(),
                    ));
                }
                Err(_) => {
                    pause(shutdown).await;
                    continue;
                }
            }
        };
        let url = versioned(&url)?;
        match connect(
            shutdown,
            &url,
            &mut Drive {
                store,
                wake,
                guild_id,
                operator_user_id,
                token,
                state: &mut state,
            },
        )
        .await
        {
            End::Shutdown => return Ok(()),
            End::Reconnect { fresh: next } => {
                fresh = next;
                pause(shutdown).await;
            }
            End::Failed(error) => return Err(error),
        }
    }
}

async fn pause(shutdown: &CancellationToken) {
    tokio::select! {
        () = shutdown.cancelled() => {}
        () = tokio::time::sleep(Duration::from_secs(5)) => {}
    }
}

enum End {
    Shutdown,
    Reconnect { fresh: bool },
    Failed(DiscordError),
}

struct Drive<'a> {
    store: &'a SurfaceStore,
    wake: &'a Notify,
    guild_id: &'a Snowflake,
    operator_user_id: &'a Snowflake,
    token: &'a str,
    state: &'a mut SocketState,
}

async fn connect(shutdown: &CancellationToken, url: &str, drive: &mut Drive<'_>) -> End {
    let config = WebSocketConfig::default()
        .max_message_size(Some(1024 * 1024))
        .max_frame_size(Some(1024 * 1024));
    let connected = tokio::time::timeout(
        Duration::from_secs(15),
        connect_async_with_config(url, Some(config), false),
    )
    .await;
    let Ok(Ok((mut socket, _))) = connected else {
        return End::Reconnect { fresh: false };
    };
    let mut next_heartbeat = None;
    loop {
        let heartbeat = async {
            if let Some(at) = next_heartbeat {
                tokio::time::sleep_until(at).await;
            } else {
                std::future::pending::<()>().await;
            }
        };
        tokio::select! {
            () = shutdown.cancelled() => return End::Shutdown,
            () = heartbeat => {
                if socket
                    .send(Message::Text(drive.state.heartbeat().to_string().into()))
                    .await
                    .is_err()
                {
                    return End::Reconnect { fresh: false };
                }
                next_heartbeat = Some(
                    tokio::time::Instant::now() + Duration::from_millis(drive.state.interval_ms),
                );
            }
            incoming = socket.next() => {
                let Some(Ok(message)) = incoming else {
                    return End::Reconnect { fresh: false };
                };
                match message {
                    Message::Text(text) => match handle_text(&mut socket, drive, &text).await {
                        Ok(Some(at)) => next_heartbeat = Some(at),
                        Ok(None) => {}
                        Err(end) => return end,
                    },
                    Message::Close(frame) => return close_end(frame.as_ref()),
                    _ => {}
                }
            }
        }
    }
}

async fn handle_text(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    drive: &mut Drive<'_>,
    text: &str,
) -> Result<Option<tokio::time::Instant>, End> {
    let step = drive
        .state
        .receive(text, drive.token)
        .map_err(End::Failed)?;
    let heartbeat = starts_session(&step)
        .then(|| tokio::time::Instant::now() + Duration::from_millis(drive.state.interval_ms));
    match step {
        Step::Send(frame) if !frame.is_null() => {
            if socket
                .send(Message::Text(frame.to_string().into()))
                .await
                .is_err()
            {
                return Err(End::Reconnect { fresh: false });
            }
        }
        Step::Ready => remember_bot(drive)?,
        Step::Message(payload) => accept_message(drive, &payload).map_err(End::Failed)?,
        Step::Reconnect { fresh } => return Err(End::Reconnect { fresh }),
        Step::Send(_) => {}
    }
    persist_cursor(drive.store, drive.state).map_err(End::Failed)?;
    Ok(heartbeat)
}

fn remember_bot(drive: &Drive<'_>) -> Result<(), End> {
    let Some(bot_user_id) = drive.state.bot_user_id.clone() else {
        return Ok(());
    };
    let bot_user_id = Snowflake::parse(&bot_user_id).map_err(End::Failed)?;
    drive.store.remember_bot(&bot_user_id).map_err(End::Failed)
}

fn close_end(frame: Option<&tokio_tungstenite::tungstenite::protocol::CloseFrame>) -> End {
    let code = frame.map_or(0, |frame| u16::from(frame.code));
    if matches!(code, 4004 | 4010 | 4011 | 4012 | 4013 | 4014) {
        End::Failed(DiscordError::Invalid(format!(
            "Discord closed the gateway with code {code}"
        )))
    } else {
        End::Reconnect { fresh: false }
    }
}

fn starts_session(step: &Step) -> bool {
    let Step::Send(frame) = step else {
        return false;
    };
    matches!(
        frame.get("op").and_then(serde_json::Value::as_i64),
        Some(2 | 6)
    )
}

fn accept_message(drive: &Drive<'_>, payload: &[u8]) -> Result<(), DiscordError> {
    let Some(bot_user_id) = drive.state.bot_user_id.as_deref() else {
        return Ok(());
    };
    let Some(addressed) = ingress::addressed(
        payload,
        &Snowflake::parse(bot_user_id)?,
        drive.guild_id,
        drive.operator_user_id,
    )?
    else {
        return Ok(());
    };
    match drive.store.enqueue(
        &addressed.message_id,
        &addressed.channel_id,
        &addressed.author_id,
        &addressed.canonical,
        &addressed.prompt,
    )? {
        Enqueue::Fresh => drive.wake.notify_one(),
        Enqueue::Duplicate => {}
    }
    Ok(())
}

fn persist_cursor(store: &SurfaceStore, state: &SocketState) -> Result<(), DiscordError> {
    store.save_gateway(GatewayCursor {
        session_id: state.session_id.clone(),
        resume_url: state.resume_url.clone(),
        sequence: state.sequence,
    })
}

fn versioned(url: &str) -> Result<String, DiscordError> {
    let mut url = url::Url::parse(url)
        .map_err(|_| DiscordError::Invalid("Discord gateway URL is invalid".to_owned()))?;
    url.query_pairs_mut()
        .append_pair("v", "10")
        .append_pair("encoding", "json");
    Ok(url.to_string())
}

pub(crate) fn allowed_gateway(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    match parsed.scheme() {
        "wss" => host == "gateway.discord.gg" || host.ends_with(".discord.gg"),
        "ws" => host == "127.0.0.1" || host == "localhost",
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{SocketState, Step};

    #[test]
    fn hello_identifies_and_a_mention_payload_is_exposed() {
        let mut state = SocketState::new(None, None, None);
        let hello = state
            .receive(r#"{"op":10,"d":{"heartbeat_interval":1000}}"#, "token")
            .expect("hello");
        let Step::Send(frame) = hello else {
            panic!("hello did not identify");
        };
        assert_eq!(frame["op"], 2);
        assert_eq!(frame["d"]["intents"], super::INTENTS);

        let ready = state
            .receive(
                r#"{"op":0,"s":1,"t":"READY","d":{"session_id":"sess","resume_gateway_url":"wss://gateway.discord.gg","user":{"id":"50"}}}"#,
                "token",
            )
            .expect("ready");
        assert!(matches!(ready, Step::Ready));
        assert_eq!(state.bot_user_id.as_deref(), Some("50"));
        assert_eq!(state.sequence, Some(1));

        let message = state
            .receive(
                r#"{"op":0,"s":2,"t":"MESSAGE_CREATE","d":{"id":"101","content":"hi"}}"#,
                "token",
            )
            .expect("message");
        let Step::Message(payload) = message else {
            panic!("message was not exposed");
        };
        let payload: serde_json::Value = serde_json::from_slice(&payload).expect("payload");
        assert_eq!(payload["id"], "101");
    }

    #[test]
    fn an_invalid_session_drops_resume_state() {
        let mut state = SocketState::new(
            Some("sess".to_owned()),
            Some("wss://gateway.discord.gg".to_owned()),
            Some(4),
        );
        let step = state
            .receive(r#"{"op":9,"d":false}"#, "token")
            .expect("invalid");
        assert!(matches!(step, Step::Reconnect { fresh: true }));
        assert!(state.session_id.is_none());
    }
}
