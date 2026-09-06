use std::{path::PathBuf, sync::Arc};

use axum::{Json, Router, extract::State, routing::post};
use renoa_kernel::AgentId;
use renoa_local::{
    AgentRecord, LocalHost, LocalHostAdapters, LocalModelConfiguration, ModelProvider,
    arcee_profile,
};
use serde_json::{Value, json};
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    api::SlackApi,
    ingress::Envelope,
    service::Active,
    socket::Receiver,
    store::{Binding, Store},
    worker::Worker,
};

mod agents;
mod channels;
mod execution;
mod ingress;
mod transport;

struct Fixture {
    directory: tempfile::TempDir,
    worker: Worker,
    receiver: Receiver,
    sent: Arc<Mutex<Vec<Value>>>,
    api_stop: CancellationToken,
    api_task: tokio::task::JoinHandle<std::io::Result<()>>,
    bridge: PathBuf,
}

impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().expect("fixture");
        let (workspace, bridge, auth) = write_model_fixture(directory.path());
        let profile = arcee_profile(directory.path()).expect("Arcee profile");
        let profile_id = profile.id().clone();
        let host = LocalHost::new(
            directory.path(),
            LocalModelConfiguration::new(
                &bridge,
                vec![ModelProvider::OpenCodeGo],
                ModelProvider::OpenCodeGo,
                "fixture",
                auth,
            ),
            vec![profile],
            LocalHostAdapters::default(),
        )
        .expect("host");
        let agent_uuid = Uuid::new_v4();
        let agent_id = AgentId::from_uuid(agent_uuid);
        host.ensure_agent(AgentRecord {
            id: agent_id,
            profile: profile_id,
            name: "Arcee".to_owned(),
            created_by: None,
        })
        .await
        .expect("agent");
        let store = Store::open(
            directory.path(),
            &Binding {
                host_id: host.host_id().await.expect("host identity"),
                agent_id: agent_uuid,
                team: "T1",
                bot: "U2",
                user: "U3",
                workspace: &workspace,
            },
        )
        .expect("store");
        let sent = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/chat.postMessage", post(send))
            .route("/chat.update", post(send))
            .with_state(Arc::clone(&sent));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let origin = url::Url::parse(&format!(
            "http://{}/",
            listener.local_addr().expect("address")
        ))
        .expect("origin");
        let api_stop = CancellationToken::new();
        let stop = api_stop.clone();
        let api_task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(stop.cancelled_owned())
                .await
        });
        let api = Arc::new(
            SlackApi::with_origin("xoxb-test".to_owned(), "xapp-test".to_owned(), origin)
                .expect("api"),
        );
        let active = Arc::new(Active::default());
        let wake = Arc::new(Notify::new());
        let shutdown = CancellationToken::new();
        let receiver = Receiver {
            host: host.clone(),
            api: Arc::clone(&api),
            store: store.clone(),
            active: Arc::clone(&active),
            wake: Arc::clone(&wake),
            shutdown: shutdown.clone(),
            team: "T1".to_owned(),
            bot: "U2".to_owned(),
            user: "U3".to_owned(),
        };
        let worker = Worker {
            host,
            agent_id,
            workspace,
            api,
            store,
            active,
            wake,
            shutdown,
            session: None,
            channel_wake: Arc::new(Notify::new()),
        };
        Self {
            directory,
            worker,
            receiver,
            sent,
            api_stop,
            api_task,
            bridge,
        }
    }

    async fn admit(&self, event: &str, ts: &str, text: &str) -> String {
        self.receiver
            .admit_envelope(envelope(event, ts, text))
            .await
            .expect("admission")
            .expect("ack")
    }

    async fn stop(self) {
        self.api_stop.cancel();
        self.api_task.await.expect("API task").expect("API stopped");
    }
}

async fn send(State(sent): State<Arc<Mutex<Vec<Value>>>>, Json(body): Json<Value>) -> Json<Value> {
    let mut sent = sent.lock().await;
    sent.push(body);
    Json(json!({"ok":true,"ts":format!("{}.000001",sent.len()+10)}))
}

fn envelope(event: &str, ts: &str, text: &str) -> Envelope {
    serde_json::from_value(json!({"type":"events_api","envelope_id":format!("socket-{event}"),"payload":{
        "type":"event_callback","team_id":"T1","api_app_id":"A1","authorizations":[{"team_id":"T1","user_id":"U2","is_bot":true}],"event_id":event,
        "event":{"type":"message","user":"U3","channel":"D1","channel_type":"im","ts":ts,"text":text}
    }})).expect("envelope fixture")
}

fn write_model_fixture(directory: &std::path::Path) -> (PathBuf, PathBuf, PathBuf) {
    let workspace = directory.join("workspace");
    std::fs::create_dir(&workspace).expect("workspace");
    let bridge = directory.join("bridge.mjs");
    std::fs::write(&bridge, include_str!("bridge.mjs")).expect("bridge");
    let auth = directory.join("auth.sqlite");
    std::fs::write(&auth, "").expect("auth");
    (workspace, bridge, auth)
}
