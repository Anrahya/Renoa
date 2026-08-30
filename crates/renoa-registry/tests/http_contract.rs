use std::path::Path;

use renoa_registry::Registry;
use renoa_registry_protocol::{
    ARCHIVE_DIGEST_HEADER, ErrorResponse, PACKAGE_MEDIA_TYPE, PublishDisposition, PublishResult,
    RegistryChanges, RegistryStatus,
};
use reqwest::{Client, StatusCode, header};
use sha2::{Digest as _, Sha256};
use tokio::{net::TcpListener, task::JoinHandle};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn publication_is_idempotent_conflict_safe_and_durable() {
    let directory = tempfile::tempdir().expect("temporary registry");
    let server = RunningRegistry::start(directory.path()).await;
    let client = Client::new();
    let package = "a".repeat(64);
    let first_body = b"first deterministic archive";

    let first = publish(&client, server.origin(), &package, first_body, None)
        .await
        .error_for_status()
        .expect("publish package")
        .json::<PublishResult>()
        .await
        .expect("decode publication");
    assert_eq!(first.disposition(), PublishDisposition::Published);
    assert_eq!(first.revision(), 1);

    let duplicate = publish(&client, server.origin(), &package, first_body, None)
        .await
        .error_for_status()
        .expect("repeat exact package")
        .json::<PublishResult>()
        .await
        .expect("decode duplicate publication");
    assert_eq!(duplicate.disposition(), PublishDisposition::Existing);
    assert_eq!(duplicate.revision(), 1);

    let conflict = publish(
        &client,
        server.origin(),
        &package,
        b"different archive",
        None,
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    let conflict = conflict
        .json::<ErrorResponse>()
        .await
        .expect("decode conflict");
    assert_eq!(conflict.code(), "conflict");

    let bad_digest = publish(
        &client,
        server.origin(),
        &"b".repeat(64),
        b"untrusted archive",
        Some("c".repeat(64)),
    )
    .await;
    assert_eq!(bad_digest.status(), StatusCode::BAD_REQUEST);
    assert_eq!(server.status().await.current_revision(), 1);

    let changes = client
        .get(format!("{}v1/changes?after=0&limit=100", server.origin()))
        .send()
        .await
        .expect("request changes")
        .error_for_status()
        .expect("successful changes")
        .json::<RegistryChanges>()
        .await
        .expect("decode changes");
    assert_eq!(changes.packages().len(), 1);
    assert_eq!(changes.packages()[0].package_digest().as_str(), package);
    let registry_id = changes.registry_id();

    server.stop().await;
    let restarted = RunningRegistry::start(directory.path()).await;
    let status = restarted.status().await;
    assert_eq!(status.registry_id(), registry_id);
    assert_eq!(status.current_revision(), 1);
    let downloaded = client
        .get(format!("{}v1/packages/{package}", restarted.origin()))
        .send()
        .await
        .expect("download package")
        .error_for_status()
        .expect("successful download")
        .bytes()
        .await
        .expect("read package");
    assert_eq!(downloaded.as_ref(), first_body);
    restarted.stop().await;
}

async fn publish(
    client: &Client,
    origin: &str,
    package: &str,
    body: &[u8],
    declared_digest: Option<String>,
) -> reqwest::Response {
    let digest = declared_digest.unwrap_or_else(|| hex(&Sha256::digest(body)));
    client
        .put(format!("{origin}v1/packages/{package}"))
        .header(header::CONTENT_TYPE, PACKAGE_MEDIA_TYPE)
        .header(header::CONTENT_LENGTH, body.len())
        .header(ARCHIVE_DIGEST_HEADER, digest)
        .body(body.to_vec())
        .send()
        .await
        .expect("send publication")
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[(byte >> 4) as usize]));
        output.push(char::from(HEX[(byte & 0x0f) as usize]));
    }
    output
}

struct RunningRegistry {
    origin: String,
    shutdown: CancellationToken,
    task: JoinHandle<()>,
}

impl RunningRegistry {
    async fn start(state: &Path) -> Self {
        let registry = Registry::open(state).expect("open registry");
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind registry listener");
        let address = listener.local_addr().expect("registry address");
        let shutdown = CancellationToken::new();
        let task_shutdown = shutdown.clone();
        let task = tokio::spawn(async move {
            registry
                .serve(listener, task_shutdown)
                .await
                .expect("serve registry");
        });
        Self {
            origin: format!("http://{address}/"),
            shutdown,
            task,
        }
    }

    fn origin(&self) -> &str {
        &self.origin
    }

    async fn status(&self) -> RegistryStatus {
        reqwest::get(format!("{}v1/status", self.origin))
            .await
            .expect("request registry status")
            .error_for_status()
            .expect("successful registry status")
            .json()
            .await
            .expect("decode registry status")
    }

    async fn stop(self) {
        self.shutdown.cancel();
        self.task.await.expect("join registry server");
    }
}
