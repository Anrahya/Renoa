use std::{cell::RefCell, fs, path::PathBuf, time::Duration};

use renoa_home::RenoaHome;
use renoa_kernel::AgentId;
use rusqlite::Connection;
use tokio::time::{Instant, sleep};
use uuid::Uuid;

use super::{
    AppError, AppState, Apps, IDLE_TIMEOUT, restore_schema_55, stop_unit_sync, unit_main_pid_sync,
};
use crate::host::{catalog, reset_host_data_root};

const TEST_HTTP_SERVER: &str = r#"import http.server, json, os, sys

PORT = int(os.environ["PORT"])
BASE_PATH = os.environ.get("BASE_PATH", "")

print(f"boot port={PORT} base={BASE_PATH}", flush=True)

if os.path.exists("/app/crash_loop"):
    print("fatal boom on startup", flush=True)
    sys.exit(1)

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/crash-once":
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"crashing")
            os._exit(1)
        body = json.dumps({"base_path": BASE_PATH, "path": self.path, "pid": os.getpid()}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, format, *args):
        pass

http.server.ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
"#;

struct UnitCleanup {
    database: PathBuf,
    names: RefCell<Vec<String>>,
}

impl UnitCleanup {
    fn register(&self, name: &str) {
        let mut names = self.names.borrow_mut();
        if !names.iter().any(|n| n == name) {
            names.push(name.to_owned());
        }
    }
}

impl Drop for UnitCleanup {
    fn drop(&mut self) {
        for name in self.names.borrow().iter() {
            stop_unit_sync(&self.database, name);
        }
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    home: RenoaHome,
    database: PathBuf,
    workspace: PathBuf,
    agent: AgentId,
    cleanup: UnitCleanup,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().canonicalize().expect("canonical");
        let home = RenoaHome::at(root.join("home")).expect("home");
        home.initialize().expect("init home");
        let database = home.host_database();
        catalog::initialize(&database).expect("catalog");
        let agent_uuid = Uuid::new_v4();
        crate::test_agents::insert_agent(&database, &agent_uuid.to_string());
        let workspace = root.join("workspace");
        fs::create_dir(&workspace).expect("workspace");
        Self {
            _dir: dir,
            home,
            database: database.clone(),
            workspace,
            agent: AgentId::from_uuid(agent_uuid),
            cleanup: UnitCleanup {
                database,
                names: RefCell::new(Vec::new()),
            },
        }
    }

    fn write_app(&self, folder: &str, script: &str) -> PathBuf {
        let dir = self.workspace.join(folder);
        fs::create_dir_all(&dir).expect("app dir");
        fs::write(dir.join("server.py"), script).expect("server.py");
        dir
    }

    fn apps(&self) -> Apps {
        Apps::open(&self.home)
    }

    fn count_rows(&self) -> i64 {
        let conn = Connection::open(&self.database).expect("db");
        conn.query_row("SELECT COUNT(*) FROM host_apps", [], |r| r.get(0))
            .expect("count")
    }

    fn state_app_dirs(&self) -> Vec<PathBuf> {
        fs::read_dir(self.home.apps())
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .collect()
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .expect("client")
}

async fn get_json(url: String) -> serde_json::Value {
    let body = client()
        .get(url)
        .send()
        .await
        .expect("http get")
        .text()
        .await
        .expect("body text");
    serde_json::from_str(&body).expect("json")
}

#[tokio::test]
async fn start_status_logs_stop_and_restart_lifecycle() {
    let f = Fixture::new();
    f.write_app("demo", TEST_HTTP_SERVER);
    f.cleanup.register("demo");
    let apps = f.apps();

    let started = apps
        .start(
            f.agent,
            None,
            &f.workspace,
            "demo",
            "demo",
            "python3 server.py",
        )
        .await
        .expect("start app");
    assert_eq!(started.state, AppState::Running);
    assert!((20_000..=29_999).contains(&started.port));

    let resp = get_json(format!("http://127.0.0.1:{}/hello", started.port)).await;
    assert_eq!(resp["base_path"], "/apps/demo");

    let status = apps.status_for("demo", None).await.expect("status");
    assert_eq!(status.state, AppState::Running);
    assert_eq!(status.port, started.port);

    let logs = apps.logs_for("demo", None).await.expect("logs");
    assert!(
        logs.logs
            .contains(&format!("boot port={} base=/apps/demo", started.port)),
        "logs={}",
        logs.logs
    );

    let stopped = apps.stop_for("demo", None).await.expect("stop");
    assert_eq!(stopped.state, AppState::Stopped);
    assert!(
        client()
            .get(format!("http://127.0.0.1:{}/", started.port))
            .send()
            .await
            .is_err(),
        "stopped app port must no longer accept connections"
    );

    let restarted = apps.restart("demo", None).await.expect("restart");
    assert_eq!(restarted.state, AppState::Running);
    assert_eq!(restarted.port, started.port);
    let after = get_json(format!("http://127.0.0.1:{}/", started.port)).await;
    assert_eq!(after["base_path"], "/apps/demo");
}

#[tokio::test]
async fn on_demand_wake_and_idle_stop() {
    let f = Fixture::new();
    f.write_app("wake", TEST_HTTP_SERVER);
    f.cleanup.register("wake");
    let apps = f
        .apps()
        .with_timeouts(Duration::from_millis(500), Duration::from_secs(10));

    let started = apps
        .start(
            f.agent,
            None,
            &f.workspace,
            "wake",
            "wake",
            "python3 server.py",
        )
        .await
        .expect("start");
    assert_eq!(started.state, AppState::Running);

    sleep(Duration::from_millis(650)).await;
    let idled = apps.stop_idle().await.expect("stop_idle");
    assert_eq!(idled, vec!["wake"]);
    assert_eq!(
        apps.status_for("wake", None).await.expect("status").state,
        AppState::Stopped
    );

    let woken = apps.ensure_running("wake").await.expect("wake");
    assert_eq!(woken.state, AppState::Running);
    assert_eq!(woken.port, started.port);
    let body = client()
        .get(format!("http://127.0.0.1:{}/", woken.port))
        .send()
        .await
        .expect("woken http")
        .text()
        .await
        .expect("text");
    assert!(body.contains("/apps/wake"), "{body}");
}

#[tokio::test]
async fn restarts_on_single_crash_then_gives_up_on_crash_loop_keeping_logs() {
    let f = Fixture::new();
    let app_dir = f.write_app("crash", TEST_HTTP_SERVER);
    f.cleanup.register("crash");
    let apps = f.apps();

    let started = apps
        .start(
            f.agent,
            None,
            &f.workspace,
            "crash",
            "crash",
            "python3 server.py",
        )
        .await
        .expect("start");
    let pid_before = unit_main_pid_sync(&f.database, "crash");
    assert_ne!(pid_before, 0);

    // Trigger one crash; systemd Restart=on-failure must bring it back cleanly.
    let _ = client()
        .get(format!("http://127.0.0.1:{}/crash-once", started.port))
        .send()
        .await;

    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        if let Ok(r) = client()
            .get(format!("http://127.0.0.1:{}/", started.port))
            .send()
            .await
            && r.status().is_success()
        {
            let pid_after = unit_main_pid_sync(&f.database, "crash");
            if pid_after != 0 && pid_after != pid_before {
                break;
            }
        }
        assert!(
            Instant::now() < deadline,
            "app did not recover from single crash"
        );
        sleep(Duration::from_millis(100)).await;
    }

    // Arm persistent crash loop and crash the running instance.
    fs::write(app_dir.join("crash_loop"), "1").expect("crash loop flag");
    let _ = client()
        .get(format!("http://127.0.0.1:{}/crash-once", started.port))
        .send()
        .await;

    let crash_deadline = Instant::now() + Duration::from_secs(12);
    loop {
        let status = apps.status_for("crash", None).await.expect("status");
        if status.state == AppState::Failed {
            break;
        }
        assert!(
            Instant::now() < crash_deadline,
            "crash loop did not give up: {status:?}"
        );
        sleep(Duration::from_millis(150)).await;
    }

    let logs = apps.logs_for("crash", None).await.expect("logs");
    assert_eq!(logs.app.state, AppState::Failed);
    assert!(
        logs.logs.contains("fatal boom on startup"),
        "logs={}",
        logs.logs
    );

    // Gateway wake must report failure immediately without retrying forever.
    let err = apps
        .ensure_running("crash")
        .await
        .expect_err("failed app refuses wake");
    assert!(matches!(err, AppError::Failed { .. }), "{err:?}");
}

#[tokio::test]
async fn unresponsive_port_is_reported_without_residue_or_infinite_retry() {
    let f = Fixture::new();
    f.write_app("hang", "import time\ntime.sleep(30)\n");
    f.cleanup.register("hang");
    let apps = f
        .apps()
        .with_timeouts(IDLE_TIMEOUT, Duration::from_millis(900));

    let err = apps
        .start(
            f.agent,
            None,
            &f.workspace,
            "hang",
            "hang",
            "python3 server.py",
        )
        .await
        .expect_err("hanging app must fail startup");
    assert!(
        matches!(err, AppError::StartupFailed { ref reason } if reason.contains("did not answer HTTP")),
        "{err:?}"
    );
    assert_eq!(f.count_rows(), 0, "failed initial start leaves no DB row");
    assert!(
        f.state_app_dirs().is_empty(),
        "failed initial start leaves no state/apps directory"
    );

    // Existing stopped app whose code later hangs must mark Failed and not retry forever.
    f.cleanup.register("stale");
    f.write_app("stale", TEST_HTTP_SERVER);
    apps.start(
        f.agent,
        None,
        &f.workspace,
        "stale",
        "stale",
        "python3 server.py",
    )
    .await
    .expect("initial good start");
    apps.stop_for("stale", None).await.expect("stop");

    fs::write(
        f.workspace.join("stale/server.py"),
        "import time\nprint('silent hang', flush=True)\ntime.sleep(30)\n",
    )
    .expect("overwrite with hanging script");

    let first_wake = apps.ensure_running("stale").await;
    assert!(
        matches!(first_wake, Err(AppError::Failed { .. })),
        "{first_wake:?}"
    );

    let t0 = Instant::now();
    let second_wake = apps.ensure_running("stale").await;
    assert!(
        matches!(second_wake, Err(AppError::Failed { .. })),
        "{second_wake:?}"
    );
    assert!(
        t0.elapsed() < Duration::from_millis(250),
        "second wake retried instead of reporting immediately: {:?}",
        t0.elapsed()
    );
}

fn bwrap_probe_script(f: &Fixture, other_dir: &std::path::Path) -> String {
    format!(
        r#"import http.server, json, os

PORT = int(os.environ["PORT"])

def try_read(path):
    try:
        with open(path, "r") as fh:
            return "READ:" + fh.read()
    except OSError as e:
        return "ERR:" + e.strerror

def try_write(path, content):
    try:
        with open(path, "w") as fh:
            fh.write(content)
        return "OK"
    except OSError as e:
        return "ERR:" + e.strerror

class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        data = {{
            "cred": try_read({cred:?}),
            "state": try_read({state:?}),
            "other": try_read({other:?}),
            "etc_write": try_write("/etc/evil", "x"),
            "usr_write": try_write("/usr/evil", "x"),
            "root_write": try_write("/root_evil", "x"),
            "app_write": try_write("/app/app_wrote.txt", "ok-app"),
            "tmp_write": try_write("/tmp/scratch_wrote.txt", "ok-tmp"),
            "tmp_log": try_read("/tmp/app.log"),
        }}
        body = json.dumps(data).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, format, *args):
        pass

http.server.ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
"#,
        cred = f
            .home
            .path()
            .join("credentials/secret.txt")
            .display()
            .to_string(),
        state = f
            .home
            .path()
            .join("state/host-secret.txt")
            .display()
            .to_string(),
        other = other_dir.join("secret.txt").display().to_string(),
    )
}

#[tokio::test]
async fn bwrap_sandbox_isolates_credentials_state_other_apps_and_root() {
    let f = Fixture::new();
    fs::write(f.home.path().join("credentials/secret.txt"), "top-secret").expect("secret");
    fs::write(f.home.path().join("state/host-secret.txt"), "state-secret").expect("state secret");
    let other_dir = f.workspace.join("other-app");
    fs::create_dir(&other_dir).expect("other app");
    fs::write(other_dir.join("secret.txt"), "other-app-secret").expect("other secret");

    let probe_script = bwrap_probe_script(&f, &other_dir);

    let app_dir = f.write_app("sandboxed", &probe_script);
    f.cleanup.register("sandboxed");
    let apps = f.apps();

    let started = apps
        .start(
            f.agent,
            None,
            &f.workspace,
            "sandboxed",
            "sandboxed",
            "python3 server.py",
        )
        .await
        .expect("start sandboxed");

    let probes = get_json(format!("http://127.0.0.1:{}/", started.port)).await;

    assert!(
        probes["cred"]
            .as_str()
            .is_some_and(|s| s.starts_with("ERR:")),
        "{probes}"
    );
    assert!(
        probes["state"]
            .as_str()
            .is_some_and(|s| s.starts_with("ERR:")),
        "{probes}"
    );
    assert!(
        probes["other"]
            .as_str()
            .is_some_and(|s| s.starts_with("ERR:")),
        "{probes}"
    );
    assert!(
        probes["tmp_log"]
            .as_str()
            .is_some_and(|s| s.starts_with("ERR:")),
        "{probes}"
    );
    assert!(
        probes["etc_write"]
            .as_str()
            .is_some_and(|s| s.starts_with("ERR:")),
        "{probes}"
    );
    assert!(
        probes["usr_write"]
            .as_str()
            .is_some_and(|s| s.starts_with("ERR:")),
        "{probes}"
    );
    assert!(
        probes["root_write"]
            .as_str()
            .is_some_and(|s| s.starts_with("ERR:")),
        "{probes}"
    );
    assert_eq!(probes["app_write"], "OK");
    assert_eq!(probes["tmp_write"], "OK");
    assert_eq!(
        fs::read_to_string(app_dir.join("app_wrote.txt")).unwrap(),
        "ok-app"
    );
    let scratch_files: Vec<_> = f
        .state_app_dirs()
        .into_iter()
        .filter(|d| d.join("scratch/scratch_wrote.txt").exists())
        .collect();
    assert_eq!(scratch_files.len(), 1);
}

#[tokio::test]
async fn non_loopback_bind_is_refused_without_residue() {
    let f = Fixture::new();
    let script = r#"import http.server, os
PORT = int(os.environ["PORT"])
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"public")
    def log_message(self, format, *args):
        pass
http.server.ThreadingHTTPServer(("0.0.0.0", PORT), H).serve_forever()
"#;
    f.write_app("publicbind", script);
    f.cleanup.register("publicbind");

    let err = f
        .apps()
        .start(
            f.agent,
            None,
            &f.workspace,
            "publicbind",
            "publicbind",
            "python3 server.py",
        )
        .await
        .expect_err("0.0.0.0 bind must be refused");
    assert!(
        matches!(err, AppError::StartupFailed { ref reason } if reason.contains("127.0.0.1 only")),
        "{err:?}"
    );
    assert_eq!(f.count_rows(), 0);
    assert!(f.state_app_dirs().is_empty());
    assert_eq!(unit_main_pid_sync(&f.database, "publicbind"), 0);
}

#[tokio::test]
async fn ipv6_bind_is_refused() {
    let f = Fixture::new();
    let script = r#"import http.server, os, socket
PORT = int(os.environ["PORT"])
v6 = socket.socket(socket.AF_INET6, socket.SOCK_STREAM)
v6.bind(("::1", PORT))
v6.listen(5)
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"v6")
    def log_message(self, format, *args):
        pass
http.server.ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
"#;
    f.write_app("ipv6bind", script);
    f.cleanup.register("ipv6bind");

    let err = f
        .apps()
        .start(
            f.agent,
            None,
            &f.workspace,
            "ipv6bind",
            "ipv6bind",
            "python3 server.py",
        )
        .await
        .expect_err("::1 bind must be refused");
    assert!(
        matches!(err, AppError::StartupFailed { ref reason } if reason.contains("127.0.0.1 only")),
        "{err:?}"
    );
    assert_eq!(f.count_rows(), 0);
    assert!(f.state_app_dirs().is_empty());
    assert_eq!(unit_main_pid_sync(&f.database, "ipv6bind"), 0);
}

#[tokio::test]
async fn ownership_blocks_other_principal_and_keeps_unowned_shared() {
    let f = Fixture::new();
    let alice = Uuid::new_v4();
    let bob = Uuid::new_v4();
    f.write_app("owned", TEST_HTTP_SERVER);
    f.write_app("shared", TEST_HTTP_SERVER);
    f.cleanup.register("owned");
    f.cleanup.register("shared");
    let apps = f.apps();

    apps.start(
        f.agent,
        Some(alice),
        &f.workspace,
        "owned",
        "owned",
        "python3 server.py",
    )
    .await
    .expect("alice starts");

    assert!(matches!(
        apps.start(
            f.agent,
            Some(bob),
            &f.workspace,
            "owned",
            "owned",
            "python3 server.py"
        )
        .await,
        Err(AppError::PermissionDenied)
    ));
    assert!(matches!(
        apps.stop_for("owned", Some(bob)).await,
        Err(AppError::PermissionDenied)
    ));
    assert!(matches!(
        apps.restart("owned", Some(bob)).await,
        Err(AppError::PermissionDenied)
    ));
    assert!(matches!(
        apps.status_for("owned", Some(bob)).await,
        Err(AppError::PermissionDenied)
    ));
    assert!(matches!(
        apps.logs_for("owned", Some(bob)).await,
        Err(AppError::PermissionDenied)
    ));
    assert!(apps.list(Some(bob)).await.expect("bob list").is_empty());
    assert_eq!(apps.list(Some(alice)).await.expect("alice list").len(), 1);

    // Unowned app stays unowned even when replaced by a signed-in principal.
    apps.start(
        f.agent,
        None,
        &f.workspace,
        "shared",
        "shared",
        "python3 server.py",
    )
    .await
    .expect("unowned start");
    apps.start(
        f.agent,
        Some(alice),
        &f.workspace,
        "shared",
        "shared",
        "python3 server.py",
    )
    .await
    .expect("alice updates shared");
    assert_eq!(
        apps.status_for("shared", Some(bob))
            .await
            .expect("bob reads shared")
            .name,
        "shared"
    );
}

#[tokio::test]
async fn reconcile_on_startup_keeps_running_pid_and_restarts_inactive_unit() {
    let f = Fixture::new();
    f.write_app("studio", TEST_HTTP_SERVER);
    f.cleanup.register("studio");
    let apps = f.apps();

    let started = apps
        .start(
            f.agent,
            None,
            &f.workspace,
            "studio",
            "studio",
            "python3 server.py",
        )
        .await
        .expect("start studio");
    let initial_pid = unit_main_pid_sync(&f.database, "studio");
    assert_ne!(initial_pid, 0);

    // Simulate renoa-node restart: fresh Apps handle reconciles without restarting live unit.
    let restarted_node_apps = Apps::open(&f.home);
    restarted_node_apps
        .reconcile_on_startup()
        .await
        .expect("reconcile node restart");
    assert_eq!(
        unit_main_pid_sync(&f.database, "studio"),
        initial_pid,
        "running app PID must survive renoa-node restart"
    );

    // Simulate host reboot: stop systemd unit while DB retains state='running'.
    stop_unit_sync(&f.database, "studio");
    assert_eq!(unit_main_pid_sync(&f.database, "studio"), 0);

    restarted_node_apps
        .reconcile_on_startup()
        .await
        .expect("reconcile after reboot");
    let rebooted_pid = unit_main_pid_sync(&f.database, "studio");
    assert_ne!(rebooted_pid, 0, "missing running unit must be restarted");
    assert_ne!(rebooted_pid, initial_pid);
    let body = client()
        .get(format!("http://127.0.0.1:{}/", started.port))
        .send()
        .await
        .expect("http after reboot reconcile")
        .text()
        .await
        .expect("text");
    assert!(body.contains("/apps/studio"), "{body}");
}

#[tokio::test]
async fn schema_56_upgrades_schema_55_and_reset_clears_apps_leaving_workspace() {
    let f = Fixture::new();
    let db = Connection::open(&f.database).expect("db");
    restore_schema_55(&db);
    db.execute_batch(
        "UPDATE host_metadata SET schema_version = 55 WHERE singleton = 1;
         PRAGMA user_version = 55;",
    )
    .expect("downgrade to 55");
    assert!(db.prepare("SELECT count(*) FROM host_apps").is_err());
    drop(db);

    catalog::initialize(&f.database).expect("upgrade schema 55 -> 56");
    catalog::initialize(&f.database).expect("reopen schema 56");
    assert_eq!(f.count_rows(), 0);

    let ws_dir = f.write_app("keepws", TEST_HTTP_SERVER);
    f.cleanup.register("keepws");
    f.apps()
        .start(
            f.agent,
            None,
            &f.workspace,
            "keepws",
            "keepws",
            "python3 server.py",
        )
        .await
        .expect("start app before reset");
    assert_eq!(f.count_rows(), 1);
    assert_eq!(f.state_app_dirs().len(), 1);

    reset_host_data_root(f.home.path()).expect("reset host");

    assert!(ws_dir.join("server.py").exists(), "workspace preserved");
    assert!(f.state_app_dirs().is_empty(), "state/apps cleared");
    assert_eq!(
        unit_main_pid_sync(&f.database, "keepws"),
        0,
        "unit stopped on reset"
    );
    assert!(
        !f.database.exists() || {
            catalog::initialize(&f.database).expect("reinit");
            f.count_rows() == 0
        }
    );
}

#[tokio::test]
async fn existing_app_command_update_succeeds_and_failure_marks_failed() {
    let f = Fixture::new();
    f.write_app("updatable", TEST_HTTP_SERVER);
    f.cleanup.register("updatable");
    let apps = f.apps().with_timeouts(IDLE_TIMEOUT, Duration::from_secs(4));

    let started = apps
        .start(
            f.agent,
            None,
            &f.workspace,
            "updatable",
            "updatable",
            "python3 server.py",
        )
        .await
        .expect("initial start");
    assert_eq!(started.state, AppState::Running);

    let updated = apps
        .start(
            f.agent,
            None,
            &f.workspace,
            "updatable",
            "updatable",
            "python3 -u server.py",
        )
        .await
        .expect("update command on running app");
    assert_eq!(updated.state, AppState::Running);
    assert_eq!(updated.port, started.port);
    assert_eq!(updated.command, "python3 -u server.py");
    let resp = get_json(format!("http://127.0.0.1:{}/hello", updated.port)).await;
    assert_eq!(resp["base_path"], "/apps/updatable");

    let err = apps
        .start(
            f.agent,
            None,
            &f.workspace,
            "updatable",
            "updatable",
            "python3 nonexistent.py",
        )
        .await
        .expect_err("failing command update must fail");
    assert!(matches!(err, AppError::StartupFailed { .. }), "{err:?}");

    let status = apps
        .status_for("updatable", None)
        .await
        .expect("status after failed update");
    assert_eq!(status.state, AppState::Failed);
    assert_eq!(status.command, "python3 nonexistent.py");
    assert!(status.failure_reason.is_some());
    assert_eq!(unit_main_pid_sync(&f.database, "updatable"), 0);
}
