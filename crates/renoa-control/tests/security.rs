use std::{
    path::Path,
    time::{Duration, SystemTime},
};

use futures_util::{SinkExt, StreamExt};
use renoa_control::{
    ClientMessage, Coordinator, DeviceCredentials, ErrorCode, JSON_WS_VERSION, NodeId,
    PeerIdentity, ServerMessage, TaskId, TaskSpec,
};
use renoa_protocol::{CommandId, CommandInput, PrincipalId, SurfaceRef, TargetRef};
use rusqlite::Connection;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::time::timeout;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

mod security_behaviors {
    use super::*;

    #[test]
    fn a_pre_identity_database_is_rejected_without_inventing_task_owners() {
        let files = TempDir::new().expect("temporary directory");
        let database = files.path().join("control.sqlite");
        let connection = Connection::open(&database).expect("open legacy database");
        connection
            .execute_batch(
                "CREATE TABLE tasks (
                task_id TEXT PRIMARY KEY,
                node_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                target_json TEXT NOT NULL,
                agent_json TEXT NOT NULL,
                next_sequence INTEGER NOT NULL
            );",
            )
            .expect("create legacy task schema");
        drop(connection);

        let Err(error) = Coordinator::open(database) else {
            panic!("legacy ownership cannot be inferred");
        };
        assert_eq!(
            error.to_string(),
            "control database predates task ownership and cannot be opened safely"
        );
    }

    #[test]
    fn a_v1_database_is_migrated_to_the_current_control_schema() {
        let files = TempDir::new().expect("temporary directory");
        let database = files.path().join("control.sqlite");
        let coordinator = Coordinator::open(&database).expect("create current database");
        drop(coordinator);
        let connection = Connection::open(&database).expect("open current database");
        connection
            .execute_batch(
                "DROP TABLE pending_executions;
             DROP TABLE execution_event_streams;
             PRAGMA user_version = 1;",
            )
            .expect("simulate schema v1");
        drop(connection);

        let coordinator = Coordinator::open(&database).expect("migrate schema v1");
        drop(coordinator);
        let connection = Connection::open(database).expect("inspect migrated database");
        let version = connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .expect("read schema version");
        let pending_table_exists = connection
            .query_row(
                "SELECT EXISTS(
                SELECT 1 FROM sqlite_master
                WHERE type = 'table' AND name = 'pending_executions'
             )",
                [],
                |row| row.get::<_, bool>(0),
            )
            .expect("inspect pending execution table");
        let stream_table_exists = connection
            .query_row(
                "SELECT EXISTS(
                SELECT 1 FROM sqlite_master
                WHERE type = 'table' AND name = 'execution_event_streams'
             )",
                [],
                |row| row.get::<_, bool>(0),
            )
            .expect("inspect execution event stream table");
        let browser_identity_tables = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name IN (
                    'passkey_bootstraps',
                    'passkey_registration_ceremonies',
                    'passkeys',
                    'passkey_authentication_ceremonies',
                    'browser_connection_tickets'
                 )",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("inspect browser identity tables");
        let oauth_relay_table_exists = connection
            .query_row(
                "SELECT EXISTS(
                SELECT 1 FROM sqlite_master
                WHERE type = 'table' AND name = 'oauth_callback_relays'
             )",
                [],
                |row| row.get::<_, bool>(0),
            )
            .expect("inspect OAuth callback relay table");
        let credential_relay_table_exists = connection
            .query_row(
                "SELECT EXISTS(
                SELECT 1 FROM sqlite_master
                WHERE type = 'table' AND name = 'credential_relays'
             )",
                [],
                |row| row.get::<_, bool>(0),
            )
            .expect("inspect credential relay table");
        assert_eq!(version, 9);
        assert!(pending_table_exists);
        assert!(stream_table_exists);
        assert_eq!(browser_identity_tables, 5);
        assert!(oauth_relay_table_exists);
        assert!(credential_relay_table_exists);
    }

    #[test]
    fn a_v3_run_event_is_migrated_without_losing_durable_history() {
        let files = TempDir::new().expect("temporary directory");
        let database = files.path().join("control.sqlite");
        let connection = Connection::open(&database).expect("open v3 database");
        connection
            .execute_batch(
                "CREATE TABLE tasks (
                    task_id TEXT PRIMARY KEY,
                    principal_id TEXT NOT NULL,
                    node_id TEXT NOT NULL,
                    agent_id TEXT NOT NULL,
                    target_json TEXT NOT NULL,
                    agent_json TEXT NOT NULL,
                    next_sequence INTEGER NOT NULL
                );
                CREATE TABLE commands (
                    command_id TEXT PRIMARY KEY,
                    task_id TEXT NOT NULL REFERENCES tasks(task_id),
                    command_json TEXT NOT NULL,
                    agent_json TEXT NOT NULL
                );
                CREATE TABLE task_events (
                    event_id TEXT PRIMARY KEY,
                    task_id TEXT NOT NULL REFERENCES tasks(task_id),
                    sequence INTEGER NOT NULL,
                    source_id TEXT NOT NULL UNIQUE,
                    kind_json TEXT NOT NULL,
                    UNIQUE(task_id, sequence)
                );
                CREATE TABLE run_event_streams (
                    command_id TEXT PRIMARY KEY REFERENCES commands(command_id),
                    run_id TEXT NOT NULL UNIQUE,
                    next_sequence INTEGER NOT NULL,
                    terminal INTEGER NOT NULL
                );
                PRAGMA user_version = 3;
                INSERT INTO tasks VALUES (
                    '00000000-0000-0000-0000-000000000001',
                    '00000000-0000-0000-0000-000000000002',
                    '00000000-0000-0000-0000-000000000003',
                    '00000000-0000-0000-0000-000000000004',
                    '\"workspace:renoa\"', '{}', 1
                );
                INSERT INTO commands VALUES (
                    '00000000-0000-0000-0000-000000000005',
                    '00000000-0000-0000-0000-000000000001',
                    '{\"commandId\":\"00000000-0000-0000-0000-000000000005\",\"agentId\":\"00000000-0000-0000-0000-000000000004\",\"principalId\":\"00000000-0000-0000-0000-000000000002\",\"surface\":\"mac\",\"target\":\"workspace:renoa\",\"input\":{\"type\":\"text\",\"text\":\"continue\"}}',
                    '{}'
                );
                INSERT INTO task_events VALUES (
                    '00000000-0000-0000-0000-000000000006',
                    '00000000-0000-0000-0000-000000000001', 0,
                    'run:00000000-0000-0000-0000-000000000007',
                    '{\"type\":\"run_event\",\"event\":{\"eventId\":\"00000000-0000-0000-0000-000000000007\",\"runId\":\"00000000-0000-0000-0000-000000000008\",\"sequence\":0,\"recordedAtMs\":9,\"kind\":{\"type\":\"run_started\",\"command\":{},\"agent\":{}}}}'
                );
                INSERT INTO run_event_streams VALUES (
                    '00000000-0000-0000-0000-000000000005',
                    '00000000-0000-0000-0000-000000000008', 1, 0
                );",
            )
            .expect("seed v3 history");
        drop(connection);

        let coordinator = Coordinator::open(&database).expect("migrate v3 history");
        drop(coordinator);
        let connection = Connection::open(database).expect("inspect migrated history");
        let (source_id, kind_json) = connection
            .query_row("SELECT source_id, kind_json FROM task_events", [], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .expect("load migrated event");
        let execution_id = connection
            .query_row(
                "SELECT execution_id FROM execution_event_streams",
                [],
                |row| row.get::<_, String>(0),
            )
            .expect("load migrated stream");
        assert_eq!(source_id, "execution:00000000-0000-0000-0000-000000000007");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&kind_json).expect("parse migrated event"),
            serde_json::json!({
                "type": "execution_event",
                "commandId": "00000000-0000-0000-0000-000000000005",
                "event": {
                    "eventId": "00000000-0000-0000-0000-000000000007",
                    "executionId": "00000000-0000-0000-0000-000000000008",
                    "sequence": 0,
                    "recordedAtMs": 9,
                    "kind": { "type": "execution_started" }
                }
            })
        );
        assert_eq!(execution_id, "00000000-0000-0000-0000-000000000008");
    }

    #[tokio::test]
    async fn a_v4_database_removes_harness_configuration_without_breaking_command_retry() {
        let files = TempDir::new().expect("temporary directory");
        let database = files.path().join("control.sqlite");
        seed_v4_database(&database);

        let coordinator = Coordinator::open(&database).expect("migrate v4 database");
        assert_harness_configuration_removed(&database);
        let server = TestServer::start(&coordinator).await;
        retry_migrated_command(&coordinator, &server.url).await;
        server.stop().await;
    }

    fn seed_v4_database(database: &Path) {
        let connection = Connection::open(database).expect("open v4 database");
        connection
            .execute_batch(
                r#"CREATE TABLE tasks (
                    task_id TEXT PRIMARY KEY,
                    principal_id TEXT NOT NULL,
                    node_id TEXT NOT NULL,
                    agent_id TEXT NOT NULL,
                    target_json TEXT NOT NULL,
                    agent_json TEXT NOT NULL,
                    next_sequence INTEGER NOT NULL
                );
                CREATE TABLE commands (
                    command_id TEXT PRIMARY KEY,
                    task_id TEXT NOT NULL REFERENCES tasks(task_id),
                    command_json TEXT NOT NULL,
                    agent_json TEXT NOT NULL
                );
                CREATE TABLE task_events (
                    event_id TEXT PRIMARY KEY,
                    task_id TEXT NOT NULL REFERENCES tasks(task_id),
                    sequence INTEGER NOT NULL,
                    source_id TEXT NOT NULL UNIQUE,
                    kind_json TEXT NOT NULL,
                    UNIQUE(task_id, sequence)
                );
                PRAGMA user_version = 4;
                INSERT INTO tasks VALUES (
                    '00000000-0000-0000-0000-000000000001',
                    '00000000-0000-0000-0000-000000000002',
                    '00000000-0000-0000-0000-000000000003',
                    '00000000-0000-0000-0000-000000000004',
                    '"workspace:renoa"',
                    '{"instructions":"old","capabilityGrants":[]}',
                    1
                );
                INSERT INTO commands VALUES (
                    '00000000-0000-0000-0000-000000000005',
                    '00000000-0000-0000-0000-000000000001',
                    '{"commandId":"00000000-0000-0000-0000-000000000005","agentId":"00000000-0000-0000-0000-000000000004","principalId":"00000000-0000-0000-0000-000000000002","surface":"mac","target":"workspace:renoa","input":{"type":"text","text":"continue"}}',
                    '{"instructions":"old","capabilityGrants":[]}'
                );
                INSERT INTO task_events VALUES (
                    '00000000-0000-0000-0000-000000000006',
                    '00000000-0000-0000-0000-000000000001',
                    0,
                    'command:00000000-0000-0000-0000-000000000005',
                    '{"type":"command_submitted","command":{"commandId":"00000000-0000-0000-0000-000000000005","agentId":"00000000-0000-0000-0000-000000000004","principalId":"00000000-0000-0000-0000-000000000002","surface":"mac","target":"workspace:renoa","input":{"type":"text","text":"continue"}}}'
                );"#,
            )
            .expect("seed v4 data");
    }

    fn assert_harness_configuration_removed(database: &Path) {
        let connection = Connection::open(database).expect("inspect migrated database");
        let command_json = connection
            .query_row("SELECT command_json FROM commands", [], |row| {
                row.get::<_, String>(0)
            })
            .expect("load migrated command");
        let event_json = connection
            .query_row("SELECT kind_json FROM task_events", [], |row| {
                row.get::<_, String>(0)
            })
            .expect("load migrated task event");
        assert!(!command_json.contains("agentId"));
        assert!(!event_json.contains("agentId"));
        for table in ["tasks", "commands"] {
            let mut columns = connection
                .prepare(&format!("PRAGMA table_info({table})"))
                .expect("inspect table columns");
            let columns = columns
                .query_map([], |row| row.get::<_, String>(1))
                .expect("read table columns")
                .collect::<Result<Vec<_>, _>>()
                .expect("collect table columns");
            assert!(!columns.iter().any(|column| column.starts_with("agent")));
        }
    }

    async fn retry_migrated_command(coordinator: &Coordinator, url: &str) {
        let credentials = enroll_peer(
            coordinator,
            url,
            PeerIdentity::Surface {
                principal_id: PrincipalId::from_uuid(
                    "00000000-0000-0000-0000-000000000002"
                        .parse()
                        .expect("principal id"),
                ),
                surface: SurfaceRef::new("mac"),
            },
        )
        .await;
        let mut surface = authenticate(url, &credentials).await;
        let command_id = CommandId::from_uuid(
            "00000000-0000-0000-0000-000000000005"
                .parse()
                .expect("command id"),
        );
        send(
            &mut surface,
            &ClientMessage::Submit {
                request_id: 1,
                task_id: TaskId::from_uuid(
                    "00000000-0000-0000-0000-000000000001"
                        .parse()
                        .expect("task id"),
                ),
                command_id,
                input: CommandInput::Text {
                    text: "continue".to_owned(),
                },
            },
        )
        .await;
        assert_eq!(
            receive(&mut surface).await,
            ServerMessage::CommandAccepted {
                request_id: 1,
                command_id,
            }
        );
    }

    #[tokio::test]
    async fn an_enrolled_device_authenticates_after_a_coordinator_restart() {
        let files = TempDir::new().expect("temporary directory");
        let database = files.path().join("control.sqlite");
        let coordinator = Coordinator::open(&database).expect("open coordinator store");
        let first_server = TestServer::start(&coordinator).await;
        let credentials = enroll_peer(
            &coordinator,
            &first_server.url,
            PeerIdentity::Surface {
                principal_id: PrincipalId::new(),
                surface: SurfaceRef::new("phone"),
            },
        )
        .await;
        first_server.stop().await;
        drop(coordinator);

        let reopened = Coordinator::open(database).expect("reopen coordinator store");
        let second_server = TestServer::start(&reopened).await;
        let _socket = authenticate(&second_server.url, &credentials).await;
        second_server.stop().await;
    }

    #[tokio::test]
    async fn the_previous_json_websocket_version_is_rejected() {
        let files = TempDir::new().expect("temporary directory");
        let coordinator =
            Coordinator::open(files.path().join("control.sqlite")).expect("open coordinator store");
        let server = TestServer::start(&coordinator).await;
        let credentials = enroll_peer(
            &coordinator,
            &server.url,
            PeerIdentity::Surface {
                principal_id: PrincipalId::new(),
                surface: SurfaceRef::new("mac"),
            },
        )
        .await;
        let (mut socket, _) = connect_async(&server.url)
            .await
            .expect("connect old binding client");

        send(
            &mut socket,
            &ClientMessage::Authenticate {
                version: JSON_WS_VERSION - 1,
                credentials,
            },
        )
        .await;
        assert!(matches!(
            receive(&mut socket).await,
            ServerMessage::Error {
                request_id: None,
                code: ErrorCode::VersionMismatch,
                ..
            }
        ));

        server.stop().await;
    }

    #[tokio::test]
    async fn json_binding_rejects_integers_that_javascript_cannot_preserve() {
        let files = TempDir::new().expect("temporary directory");
        let coordinator =
            Coordinator::open(files.path().join("control.sqlite")).expect("open coordinator store");
        let server = TestServer::start(&coordinator).await;
        let credentials = enroll_peer(
            &coordinator,
            &server.url,
            PeerIdentity::Surface {
                principal_id: PrincipalId::new(),
                surface: SurfaceRef::new("typescript"),
            },
        )
        .await;
        let mut socket = authenticate(&server.url, &credentials).await;
        let unsafe_integer = 9_007_199_254_740_992_u64;
        let messages = [
            serde_json::json!({
                "type": "list_tasks",
                "request_id": unsafe_integer,
            }),
            serde_json::json!({
                "type": "attach",
                "request_id": 1,
                "task_id": TaskId::new(),
                "after_sequence": unsafe_integer,
            }),
            serde_json::json!({
                "type": "publish_execution_events",
                "task_id": TaskId::new(),
                "command_id": CommandId::new(),
                "events": [{
                    "eventId": uuid::Uuid::new_v4(),
                    "executionId": uuid::Uuid::new_v4(),
                    "sequence": 0,
                    "recordedAtMs": unsafe_integer,
                    "kind": { "type": "execution_started" },
                }],
            }),
        ];
        for message in messages {
            socket
                .send(Message::Text(message.to_string().into()))
                .await
                .expect("send non-interoperable integer");
            assert!(matches!(
                receive(&mut socket).await,
                ServerMessage::Error {
                    request_id: None,
                    code: ErrorCode::InvalidMessage,
                    ..
                }
            ));
        }

        send(&mut socket, &ClientMessage::ListTasks { request_id: 2 }).await;
        assert!(matches!(
            receive(&mut socket).await,
            ServerMessage::TaskList { request_id: 2, .. }
        ));
        server.stop().await;
    }

    #[tokio::test]
    async fn json_binding_never_emits_an_integer_that_javascript_cannot_preserve() {
        let files = TempDir::new().expect("temporary directory");
        let database = files.path().join("control.sqlite");
        let coordinator = Coordinator::open(&database).expect("open coordinator store");
        let principal_id = PrincipalId::new();
        let task_id = TaskId::new();
        coordinator
            .create_task(TaskSpec {
                task_id,
                principal_id,
                node_id: NodeId::new(),
                target: TargetRef::new("workspace:integer-boundary"),
            })
            .await
            .expect("create task");
        Connection::open(&database)
            .expect("open control database")
            .execute(
                "UPDATE tasks SET next_sequence = ?1 WHERE task_id = ?2",
                rusqlite::params![9_007_199_254_740_993_i64, task_id.to_string()],
            )
            .expect("place sequence beyond JSON interoperable range");

        let server = TestServer::start(&coordinator).await;
        let credentials = enroll_peer(
            &coordinator,
            &server.url,
            PeerIdentity::Surface {
                principal_id,
                surface: SurfaceRef::new("typescript"),
            },
        )
        .await;
        let mut socket = authenticate(&server.url, &credentials).await;
        send(
            &mut socket,
            &ClientMessage::Attach {
                request_id: 1,
                task_id,
                after_sequence: None,
            },
        )
        .await;
        assert_eq!(
            receive(&mut socket).await,
            ServerMessage::Error {
                request_id: None,
                code: ErrorCode::Internal,
                message: "internal coordinator error".to_owned(),
            }
        );
        expect_closed(&mut socket).await;
        server.stop().await;
    }

    #[tokio::test]
    async fn a_client_cannot_self_declare_its_identity() {
        let files = TempDir::new().expect("temporary directory");
        let coordinator =
            Coordinator::open(files.path().join("control.sqlite")).expect("open coordinator store");
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind coordinator");
        let address = listener.local_addr().expect("coordinator address");
        let shutdown = CancellationToken::new();
        let server_shutdown = shutdown.clone();
        let server = tokio::spawn(async move {
            coordinator
                .serve(listener, server_shutdown)
                .await
                .expect("serve coordinator");
        });
        let (mut socket, _) = connect_async(format!("ws://{address}/connect"))
            .await
            .expect("connect peer");
        let forged = serde_json::json!({
            "type": "hello",
            "version": JSON_WS_VERSION,
            "peer": {
                "role": "surface",
                "principal_id": PrincipalId::new(),
                "surface": "attacker"
            }
        });
        socket
            .send(Message::Text(forged.to_string().into()))
            .await
            .expect("send forged identity");
        assert!(matches!(
            receive(&mut socket).await,
            ServerMessage::Error {
                code: ErrorCode::AuthenticationFailed,
                ..
            }
        ));

        shutdown.cancel();
        server.await.expect("server task");
    }

    #[tokio::test]
    async fn authenticated_roles_limit_available_operations() {
        let files = TempDir::new().expect("temporary directory");
        let coordinator =
            Coordinator::open(files.path().join("control.sqlite")).expect("open coordinator store");
        let server = TestServer::start(&coordinator).await;
        let surface_credentials = enroll_peer(
            &coordinator,
            &server.url,
            PeerIdentity::Surface {
                principal_id: PrincipalId::new(),
                surface: SurfaceRef::new("mac"),
            },
        )
        .await;
        let node_credentials = enroll_peer(
            &coordinator,
            &server.url,
            PeerIdentity::Node {
                node_id: NodeId::new(),
            },
        )
        .await;
        let task_id = TaskId::new();
        let command_id = CommandId::new();

        let mut surface = authenticate(&server.url, &surface_credentials).await;
        send(
            &mut surface,
            &ClientMessage::AcknowledgeExecution {
                task_id,
                command_id,
            },
        )
        .await;
        assert!(matches!(
            receive(&mut surface).await,
            ServerMessage::Error {
                request_id: None,
                code: ErrorCode::InvalidRole,
                ..
            }
        ));

        let mut node = authenticate(&server.url, &node_credentials).await;
        send(
            &mut node,
            &ClientMessage::Attach {
                request_id: 17,
                task_id,
                after_sequence: None,
            },
        )
        .await;
        assert!(matches!(
            receive(&mut node).await,
            ServerMessage::Error {
                request_id: Some(17),
                code: ErrorCode::InvalidRole,
                ..
            }
        ));

        server.stop().await;
    }

    #[tokio::test]
    async fn a_surface_cannot_access_another_principals_task() {
        let files = TempDir::new().expect("temporary directory");
        let coordinator =
            Coordinator::open(files.path().join("control.sqlite")).expect("open coordinator store");
        let owner = PrincipalId::new();
        let task_id = TaskId::new();
        coordinator
            .create_task(TaskSpec {
                task_id,
                principal_id: owner,
                node_id: NodeId::new(),
                target: TargetRef::new("workspace:private"),
            })
            .await
            .expect("create owned task");
        let server = TestServer::start(&coordinator).await;
        let credentials = enroll_peer(
            &coordinator,
            &server.url,
            PeerIdentity::Surface {
                principal_id: PrincipalId::new(),
                surface: SurfaceRef::new("attacker"),
            },
        )
        .await;
        let mut socket = authenticate(&server.url, &credentials).await;
        send(
            &mut socket,
            &ClientMessage::Attach {
                request_id: 7,
                task_id,
                after_sequence: None,
            },
        )
        .await;
        assert!(matches!(
            receive(&mut socket).await,
            ServerMessage::Error {
                request_id: Some(7),
                code: ErrorCode::NotFound,
                ..
            }
        ));
        send(
            &mut socket,
            &ClientMessage::Submit {
                request_id: 8,
                task_id,
                command_id: CommandId::new(),
                input: CommandInput::Text {
                    text: "steal this task".to_owned(),
                },
            },
        )
        .await;
        assert!(matches!(
            receive(&mut socket).await,
            ServerMessage::Error {
                request_id: Some(8),
                code: ErrorCode::NotFound,
                ..
            }
        ));

        server.stop().await;
    }

    #[tokio::test]
    async fn revoking_a_device_ends_its_session_and_rejects_reconnection() {
        let files = TempDir::new().expect("temporary directory");
        let database = files.path().join("control.sqlite");
        let coordinator = Coordinator::open(&database).expect("open coordinator store");
        let server = TestServer::start(&coordinator).await;
        let credentials = enroll_peer(
            &coordinator,
            &server.url,
            PeerIdentity::Surface {
                principal_id: PrincipalId::new(),
                surface: SurfaceRef::new("lost-phone"),
            },
        )
        .await;
        let mut active_socket = authenticate(&server.url, &credentials).await;
        let mut second_socket = authenticate(&server.url, &credentials).await;

        coordinator
            .revoke_device(credentials.device_id)
            .await
            .expect("revoke device");
        expect_closed(&mut active_socket).await;
        expect_closed(&mut second_socket).await;
        server.stop().await;
        drop(coordinator);

        let reopened = Coordinator::open(database).expect("reopen coordinator store");
        let restarted_server = TestServer::start(&reopened).await;
        let (mut reconnected, _) = connect_async(&restarted_server.url)
            .await
            .expect("reconnect revoked device");
        send(
            &mut reconnected,
            &ClientMessage::Authenticate {
                version: JSON_WS_VERSION,
                credentials,
            },
        )
        .await;
        assert!(matches!(
            receive(&mut reconnected).await,
            ServerMessage::Error {
                code: ErrorCode::AuthenticationFailed,
                ..
            }
        ));
        restarted_server.stop().await;
    }

    #[tokio::test]
    async fn concurrent_enrollment_claims_create_one_device() {
        let files = TempDir::new().expect("temporary directory");
        let coordinator =
            Coordinator::open(files.path().join("control.sqlite")).expect("open coordinator store");
        let token = coordinator
            .create_enrollment(
                PeerIdentity::Surface {
                    principal_id: PrincipalId::new(),
                    surface: SurfaceRef::new("phone"),
                },
                SystemTime::now() + Duration::from_mins(1),
            )
            .await
            .expect("create enrollment");
        let server = TestServer::start(&coordinator).await;

        let (first, second) = tokio::join!(
            claim_enrollment(&server.url, token.clone()),
            claim_enrollment(&server.url, token),
        );
        let enrolled = [&first, &second]
            .into_iter()
            .filter(|message| matches!(message, ServerMessage::Enrolled { .. }))
            .count();
        let rejected = [&first, &second]
            .into_iter()
            .filter(|message| {
                matches!(
                    message,
                    ServerMessage::Error {
                        code: ErrorCode::AuthenticationFailed,
                        ..
                    }
                )
            })
            .count();
        assert_eq!((enrolled, rejected), (1, 1));

        server.stop().await;
    }

    #[tokio::test]
    async fn a_credential_is_bound_to_its_device_id() {
        let files = TempDir::new().expect("temporary directory");
        let coordinator =
            Coordinator::open(files.path().join("control.sqlite")).expect("open coordinator store");
        let server = TestServer::start(&coordinator).await;
        let peer = PeerIdentity::Surface {
            principal_id: PrincipalId::new(),
            surface: SurfaceRef::new("phone"),
        };
        let first = enroll_peer(&coordinator, &server.url, peer.clone()).await;
        let second = enroll_peer(&coordinator, &server.url, peer).await;
        let forged = DeviceCredentials {
            device_id: first.device_id,
            credential: second.credential,
        };
        let (mut socket, _) = connect_async(&server.url)
            .await
            .expect("connect forged device");
        send(
            &mut socket,
            &ClientMessage::Authenticate {
                version: JSON_WS_VERSION,
                credentials: forged,
            },
        )
        .await;
        assert!(matches!(
            receive(&mut socket).await,
            ServerMessage::Error {
                code: ErrorCode::AuthenticationFailed,
                ..
            }
        ));

        server.stop().await;
    }

    #[tokio::test]
    async fn an_expired_enrollment_cannot_be_claimed() {
        let files = TempDir::new().expect("temporary directory");
        let coordinator =
            Coordinator::open(files.path().join("control.sqlite")).expect("open coordinator store");
        let token = coordinator
            .create_enrollment(
                PeerIdentity::Surface {
                    principal_id: PrincipalId::new(),
                    surface: SurfaceRef::new("phone"),
                },
                SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            )
            .await
            .expect("create expired enrollment");
        let server = TestServer::start(&coordinator).await;

        assert!(matches!(
            claim_enrollment(&server.url, token).await,
            ServerMessage::Error {
                code: ErrorCode::AuthenticationFailed,
                ..
            }
        ));

        server.stop().await;
    }

    #[tokio::test]
    async fn a_storage_failure_is_not_reported_as_invalid_client_input() {
        let files = TempDir::new().expect("temporary directory");
        let database = files.path().join("control.sqlite");
        let coordinator = Coordinator::open(&database).expect("open coordinator store");
        let principal_id = PrincipalId::new();
        let task_id = TaskId::new();
        coordinator
            .create_task(TaskSpec {
                task_id,
                principal_id,
                node_id: NodeId::new(),
                target: TargetRef::new("workspace:store-failure"),
            })
            .await
            .expect("create task");
        let server = TestServer::start(&coordinator).await;
        let credentials = enroll_peer(
            &coordinator,
            &server.url,
            PeerIdentity::Surface {
                principal_id,
                surface: SurfaceRef::new("test"),
            },
        )
        .await;
        let mut socket = authenticate(&server.url, &credentials).await;
        let connection = Connection::open(database).expect("open coordinator database");
        connection
            .execute("DROP TABLE task_events", [])
            .expect("simulate storage failure");
        drop(connection);

        send(
            &mut socket,
            &ClientMessage::Attach {
                request_id: 91,
                task_id,
                after_sequence: None,
            },
        )
        .await;
        assert!(matches!(
            receive(&mut socket).await,
            ServerMessage::Error {
                request_id: Some(91),
                code: ErrorCode::Internal,
                message,
            } if message == "internal coordinator error"
        ));

        server.stop().await;
    }

    #[tokio::test]
    async fn the_plaintext_coordinator_refuses_public_interfaces() {
        let files = TempDir::new().expect("temporary directory");
        let coordinator =
            Coordinator::open(files.path().join("control.sqlite")).expect("open coordinator store");
        let listener = TcpListener::bind("0.0.0.0:0")
            .await
            .expect("bind public interface");

        let error = coordinator
            .serve(listener, CancellationToken::new())
            .await
            .expect_err("plaintext coordinator must reject public interfaces");
        assert_eq!(
            error.to_string(),
            "the plaintext coordinator is loopback-only"
        );
    }
}

struct TestServer {
    url: String,
    shutdown: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl TestServer {
    async fn start(coordinator: &Coordinator) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind coordinator");
        let address = listener.local_addr().expect("coordinator address");
        let shutdown = CancellationToken::new();
        let server_shutdown = shutdown.clone();
        let server_coordinator = coordinator.clone();
        let task = tokio::spawn(async move {
            server_coordinator
                .serve(listener, server_shutdown)
                .await
                .expect("serve coordinator");
        });
        Self {
            url: format!("ws://{address}/connect"),
            shutdown,
            task,
        }
    }

    async fn stop(self) {
        self.shutdown.cancel();
        self.task.await.expect("server task");
    }
}

async fn claim_enrollment(url: &str, token: renoa_control::EnrollmentToken) -> ServerMessage {
    let (mut socket, _) = connect_async(url).await.expect("connect for enrollment");
    send(
        &mut socket,
        &ClientMessage::Enroll {
            version: JSON_WS_VERSION,
            token,
        },
    )
    .await;
    receive(&mut socket).await
}

async fn enroll_peer(
    coordinator: &Coordinator,
    url: &str,
    peer: PeerIdentity,
) -> DeviceCredentials {
    let token = coordinator
        .create_enrollment(peer, SystemTime::now() + Duration::from_mins(1))
        .await
        .expect("create enrollment");
    let ServerMessage::Enrolled { credentials, .. } = claim_enrollment(url, token).await else {
        panic!("server should enroll device");
    };
    credentials
}

async fn authenticate(url: &str, credentials: &DeviceCredentials) -> Socket {
    let (mut socket, _) = connect_async(url).await.expect("connect device");
    send(
        &mut socket,
        &ClientMessage::Authenticate {
            version: JSON_WS_VERSION,
            credentials: credentials.clone(),
        },
    )
    .await;
    assert_eq!(
        receive(&mut socket).await,
        ServerMessage::Authenticated {
            version: JSON_WS_VERSION
        }
    );
    socket
}

async fn expect_closed(socket: &mut Socket) {
    let closed = timeout(Duration::from_secs(1), socket.next())
        .await
        .expect("revocation should close active session");
    assert!(matches!(
        closed,
        None | Some(Ok(Message::Close(_)) | Err(_))
    ));
}

async fn send<S>(socket: &mut tokio_tungstenite::WebSocketStream<S>, message: &ClientMessage)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let value = serde_json::to_string(message).expect("serialize client message");
    socket
        .send(Message::Text(value.into()))
        .await
        .expect("send client message");
}

async fn receive<S>(socket: &mut tokio_tungstenite::WebSocketStream<S>) -> ServerMessage
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let message = socket
        .next()
        .await
        .expect("server message")
        .expect("valid websocket message");
    let Message::Text(value) = message else {
        panic!("expected text websocket message");
    };
    serde_json::from_str(&value).expect("deserialize server message")
}
