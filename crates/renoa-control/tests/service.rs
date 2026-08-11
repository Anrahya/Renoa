use std::{
    io::{BufRead, BufReader},
    path::Path,
    process::{Child, Command, Stdio},
    time::Duration,
};

#[cfg(unix)]
use std::{process::ExitStatus, thread, time::Instant};

use futures_util::{SinkExt, StreamExt};
use renoa_control::{
    ClientMessage, Coordinator, DeviceCredentials, EnrollmentToken, JSON_WS_VERSION, ServerMessage,
    TaskId, TaskSummary,
};
use renoa_protocol::{
    CommandEnvelope, CommandId, CommandInput, PrincipalId, SurfaceRef, TargetRef,
};
use serde::Deserialize;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};
use uuid::Uuid;

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

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
        let child = coordinator_command()
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

fn coordinator_command() -> Command {
    Command::new(
        std::env::var_os("CARGO_BIN_EXE_renoa-coordinator")
            .expect("Cargo did not build the renoa-coordinator binary"),
    )
}

fn create_surface_enrollment(database: &Path) -> EnrollmentToken {
    enrollment_token(
        coordinator_command()
            .arg("enroll-surface")
            .arg(database)
            .arg(Uuid::from_u128(1).to_string())
            .arg("service_test"),
        "surface",
    )
}

fn create_node_enrollment(database: &Path) -> EnrollmentToken {
    enrollment_token(
        coordinator_command()
            .arg("enroll-node")
            .arg(database)
            .arg(Uuid::from_u128(2).to_string()),
        "node",
    )
}

fn enrollment_token(command: &mut Command, peer: &str) -> EnrollmentToken {
    let output = command.output().expect("run enrollment command");
    assert!(
        output.status.success(),
        "{peer} enrollment command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let EnrollmentCreated { token } =
        serde_json::from_slice(&output.stdout).expect("parse enrollment output");
    token
}

fn create_task(database: &Path) {
    let output = coordinator_command()
        .arg("create-task")
        .arg(database)
        .arg(Uuid::from_u128(3).to_string())
        .arg(Uuid::from_u128(1).to_string())
        .arg(Uuid::from_u128(2).to_string())
        .arg("workspace:service-test")
        .output()
        .expect("run task creation command");
    assert!(
        output.status.success(),
        "task creation command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty(), "task creation must be silent");
}

#[tokio::test]
async fn coordinator_executable_provisions_and_serves_the_existing_protocol() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("control.db");
    let surface_token = create_surface_enrollment(&database);
    let node_token = create_node_enrollment(&database);
    create_task(&database);

    let mut process = CoordinatorProcess::start(&database);
    let ready = process.read_ready();
    let surface_credentials = enroll(&ready.endpoint, surface_token).await;
    let mut surface = authenticate(&ready.endpoint, surface_credentials).await;
    let node_credentials = enroll(&ready.endpoint, node_token).await;
    let mut node = authenticate(&ready.endpoint, node_credentials).await;

    send(&mut surface, &ClientMessage::ListTasks { request_id: 1 }).await;
    assert_eq!(
        receive(&mut surface).await,
        ServerMessage::TaskList {
            request_id: 1,
            tasks: vec![TaskSummary {
                task_id: TaskId::from_uuid(Uuid::from_u128(3)),
                target: TargetRef::new("workspace:service-test"),
            }],
        }
    );

    let command_id = CommandId::from_uuid(Uuid::from_u128(4));
    let input = CommandInput::Text {
        text: "continue".to_owned(),
    };
    send(
        &mut surface,
        &ClientMessage::Submit {
            request_id: 2,
            task_id: TaskId::from_uuid(Uuid::from_u128(3)),
            command_id,
            input: input.clone(),
        },
    )
    .await;
    assert_eq!(
        receive(&mut surface).await,
        ServerMessage::CommandAccepted {
            request_id: 2,
            command_id,
        }
    );
    assert_eq!(
        receive(&mut node).await,
        ServerMessage::Execute {
            task_id: TaskId::from_uuid(Uuid::from_u128(3)),
            command: CommandEnvelope {
                command_id,
                principal_id: PrincipalId::from_uuid(Uuid::from_u128(1)),
                surface: SurfaceRef::new("service_test"),
                target: TargetRef::new("workspace:service-test"),
                input,
            },
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

async fn enroll(endpoint: &str, token: EnrollmentToken) -> DeviceCredentials {
    let (mut socket, _) = connect_async(endpoint)
        .await
        .expect("connect for enrollment");
    send(
        &mut socket,
        &ClientMessage::Enroll {
            version: JSON_WS_VERSION,
            token,
        },
    )
    .await;
    let ServerMessage::Enrolled { credentials, .. } = receive(&mut socket).await else {
        panic!("coordinator did not enroll the peer");
    };
    credentials
}

async fn authenticate(endpoint: &str, credentials: DeviceCredentials) -> Socket {
    let (mut socket, _) = connect_async(endpoint)
        .await
        .expect("connect authenticated peer");
    send(
        &mut socket,
        &ClientMessage::Authenticate {
            version: JSON_WS_VERSION,
            credentials,
        },
    )
    .await;
    assert_eq!(
        receive(&mut socket).await,
        ServerMessage::Authenticated {
            version: JSON_WS_VERSION,
        }
    );
    socket
}

async fn send(socket: &mut Socket, message: &ClientMessage) {
    socket
        .send(Message::Text(
            serde_json::to_string(message)
                .expect("serialize client message")
                .into(),
        ))
        .await
        .expect("send client message");
}

async fn receive(socket: &mut Socket) -> ServerMessage {
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
