use std::{
    io::{BufRead, BufReader},
    path::Path,
    process::{Child, Command, Stdio},
    time::Duration,
};

#[cfg(unix)]
use std::{process::ExitStatus, thread, time::Instant};

use futures_util::{SinkExt, StreamExt};
use renoa_control::{ClientMessage, Coordinator, EnrollmentToken, JSON_WS_VERSION, ServerMessage};
use serde::Deserialize;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use uuid::Uuid;

#[derive(Deserialize)]
struct Ready {
    endpoint: String,
}

#[derive(Deserialize)]
struct EnrollmentCreated {
    token: EnrollmentToken,
}

struct CoordinatorProcess(Child);

impl CoordinatorProcess {
    fn start(database: &Path) -> Self {
        let binary = std::env::var_os("CARGO_BIN_EXE_renoa-coordinator")
            .expect("Cargo did not build the renoa-coordinator binary");
        let child = Command::new(binary)
            .arg("serve")
            .arg(database)
            .arg("0")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start coordinator process");
        Self(child)
    }

    fn read_ready(&mut self) -> Ready {
        let stdout = self.0.stdout.take().expect("coordinator stdout");
        let mut line = String::new();
        let bytes = BufReader::new(stdout)
            .read_line(&mut line)
            .expect("read coordinator readiness");
        assert_ne!(bytes, 0, "coordinator exited before reporting readiness");
        serde_json::from_str(&line).expect("parse coordinator readiness")
    }

    #[cfg(unix)]
    fn terminate(&mut self) -> ExitStatus {
        let signal = Command::new("kill")
            .arg("-TERM")
            .arg(self.0.id().to_string())
            .status()
            .expect("send coordinator termination signal");
        assert!(signal.success(), "failed to signal coordinator");

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(status) = self.0.try_wait().expect("read coordinator status") {
                return status;
            }
            assert!(Instant::now() < deadline, "coordinator did not stop");
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for CoordinatorProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn coordinator_executable_serves_the_existing_protocol() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("control.db");
    let enrollment = Command::new(
        std::env::var_os("CARGO_BIN_EXE_renoa-coordinator")
            .expect("Cargo did not build the renoa-coordinator binary"),
    )
    .arg("enroll-surface")
    .arg(&database)
    .arg(Uuid::from_u128(1).to_string())
    .arg("service_test")
    .output()
    .expect("run enrollment command");
    assert!(
        enrollment.status.success(),
        "enrollment command failed: {}",
        String::from_utf8_lossy(&enrollment.stderr)
    );
    let EnrollmentCreated { token } =
        serde_json::from_slice(&enrollment.stdout).expect("parse enrollment output");

    let mut process = CoordinatorProcess::start(&database);
    let ready = process.read_ready();
    let (mut enrollment, _) = connect_async(&ready.endpoint)
        .await
        .expect("connect for enrollment");
    send(
        &mut enrollment,
        &ClientMessage::Enroll {
            version: JSON_WS_VERSION,
            token,
        },
    )
    .await;
    let ServerMessage::Enrolled { credentials, .. } = receive(&mut enrollment).await else {
        panic!("coordinator did not enroll the surface");
    };

    let (mut surface, _) = connect_async(&ready.endpoint)
        .await
        .expect("connect authenticated surface");
    send(
        &mut surface,
        &ClientMessage::Authenticate {
            version: JSON_WS_VERSION,
            credentials,
        },
    )
    .await;
    assert_eq!(
        receive(&mut surface).await,
        ServerMessage::Authenticated {
            version: JSON_WS_VERSION,
        }
    );
}

#[cfg(unix)]
#[test]
fn coordinator_executable_stops_cleanly_on_termination() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("control.db");
    Coordinator::open(&database).expect("open coordinator database");

    let mut process = CoordinatorProcess::start(&database);
    process.read_ready();
    assert!(process.terminate().success());
}

async fn send(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    message: &ClientMessage,
) {
    socket
        .send(Message::Text(
            serde_json::to_string(message)
                .expect("serialize client message")
                .into(),
        ))
        .await
        .expect("send client message");
}

async fn receive(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> ServerMessage {
    let message = socket
        .next()
        .await
        .expect("server closed connection")
        .expect("receive server message");
    let Message::Text(json) = message else {
        panic!("expected text frame");
    };
    serde_json::from_str(&json).expect("deserialize server message")
}
