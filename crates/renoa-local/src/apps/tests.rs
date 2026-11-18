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
        let dir = tempfile::Builder::new()
            .prefix("renoa-apps-test-")
            .tempdir_in("/var/tmp")
            .or_else(|_| tempfile::tempdir())
            .expect("tempdir");
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

    let hang_wake = apps.ensure_running("stale").await;
    assert!(
        matches!(hang_wake, Err(AppError::StartupFailed { .. })),
        "{hang_wake:?}"
    );
    let after_hang = apps
        .status_for("stale", None)
        .await
        .expect("status after timeout");
    assert_eq!(
        after_hang.state,
        AppState::Stopped,
        "cold-start timeout must keep stopped state so next request can retry"
    );

    fs::write(
        f.workspace.join("stale/server.py"),
        "import sys\nprint('crash', flush=True)\nsys.exit(1)\n",
    )
    .expect("overwrite with crashing script");

    let crash_apps = f.apps();
    let first_wake = crash_apps.ensure_running("stale").await;
    assert!(
        matches!(first_wake, Err(AppError::Failed { .. })),
        "{first_wake:?}"
    );
    let after_crash = crash_apps
        .status_for("stale", None)
        .await
        .expect("status after crash");
    assert_eq!(after_crash.state, AppState::Failed);

    let t0 = Instant::now();
    let second_wake = crash_apps.ensure_running("stale").await;
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
            "passwd": try_read("/etc/passwd"),
            "shadow": try_read("/etc/shadow"),
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
        probes["passwd"]
            .as_str()
            .is_some_and(|s| s.starts_with("ERR:")),
        "{probes}"
    );
    assert!(
        probes["shadow"]
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
        .expect_err("failing command update must fail staging");
    assert!(matches!(err, AppError::StartupFailed { .. }), "{err:?}");

    let still_live = apps
        .status_for("updatable", None)
        .await
        .expect("running app stays running after staging failure");
    assert_eq!(still_live.state, AppState::Running);
    assert_eq!(still_live.command, "python3 -u server.py");
    let still_resp = get_json(format!("http://127.0.0.1:{}/still-live", started.port)).await;
    assert_eq!(still_resp["base_path"], "/apps/updatable");

    apps.stop_for("updatable", None)
        .await
        .expect("stop updatable before failing stopped start");
    let stopped_err = apps
        .start(
            f.agent,
            None,
            &f.workspace,
            "updatable",
            "updatable",
            "python3 nonexistent.py",
        )
        .await
        .expect_err("failing stopped update must fail");
    assert!(
        matches!(stopped_err, AppError::StartupFailed { .. }),
        "{stopped_err:?}"
    );

    let status = apps
        .status_for("updatable", None)
        .await
        .expect("status after failed update");
    assert_eq!(status.state, AppState::Failed);
    assert_eq!(status.command, "python3 -u server.py");
    assert!(status.failure_reason.is_some());
    assert_eq!(unit_main_pid_sync(&f.database, "updatable"), 0);
}

#[tokio::test]
async fn command_supports_braced_vars_and_literals_while_rejecting_workspace_root_folder() {
    let f = Fixture::new();
    let script = r#"import http.server, json, os, sys

port = int(sys.argv[1])
base_arg = sys.argv[2]
tag = sys.argv[3]

class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        body = json.dumps({
            "port": port,
            "base": base_arg,
            "tag": tag,
            "env_port": os.environ["PORT"],
        }).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, format, *args):
        pass

http.server.ThreadingHTTPServer(("127.0.0.1", port), H).serve_forever()
"#;
    f.write_app("braced", script);
    f.cleanup.register("braced");
    let apps = f.apps();

    let dot_err = apps
        .start(
            f.agent,
            None,
            &f.workspace,
            "rootrefused",
            ".",
            "python3 server.py",
        )
        .await
        .expect_err("workspace root `.` must be refused as app folder");
    assert!(matches!(dot_err, AppError::InvalidFolder(_)), "{dot_err:?}");
    assert_eq!(f.count_rows(), 0);

    let started = apps
        .start(
            f.agent,
            None,
            &f.workspace,
            "braced",
            "braced",
            r#"python3 server.py "${PORT}" "${BASE_PATH}" '$100'"#,
        )
        .await
        .expect("start with braced vars and literal dollar");
    let resp = get_json(format!("http://127.0.0.1:{}/", started.port)).await;
    assert_eq!(resp["port"], started.port);
    assert_eq!(resp["base"], "/apps/braced");
    assert_eq!(resp["tag"], "$100");
    assert_eq!(resp["env_port"], started.port.to_string());
}

#[test]
fn host_addresses_come_from_fib_trie_without_a_netlink_socket() {
    let sample = "Main:
  +-- 0.0.0.0/0 3 0 5
     |-- 0.0.0.0
        /0 universe UNICAST
     +-- 192.168.1.0/24 2 0 2
        |-- 192.168.1.0
           /24 link UNICAST
        |-- 192.168.1.7
           /32 host LOCAL
Local:
  +-- 100.64.0.0/10 4 0 11
     |-- 100.78.152.5
        /32 host LOCAL
     |-- 100.100.100.100
        /32 link UNICAST
";
    let found: Vec<String> = super::sandbox::local_addresses(sample)
        .into_iter()
        .map(|ip| ip.to_string())
        .collect();
    assert_eq!(found, ["100.78.152.5", "192.168.1.7"]);

    // On this machine the trie names the same addresses getifaddrs does.
    let trie = fs::read_to_string("/proc/net/fib_trie").expect("fib_trie");
    let from_trie = super::sandbox::local_addresses(&trie);
    let from_interfaces: std::collections::BTreeSet<_> = nix::ifaddrs::getifaddrs()
        .expect("getifaddrs")
        .filter_map(|interface| {
            interface
                .address?
                .as_sockaddr_in()
                .map(nix::sys::socket::SockaddrIn::ip)
        })
        .collect();
    assert_eq!(from_trie, from_interfaces);
}

/// One IPv4 address the host holds outside loopback, for egress probes.
fn host_non_loopback_ipv4() -> Option<std::net::Ipv4Addr> {
    nix::ifaddrs::getifaddrs()
        .expect("getifaddrs")
        .filter_map(|interface| {
            interface
                .address?
                .as_sockaddr_in()
                .map(nix::sys::socket::SockaddrIn::ip)
        })
        .find(|ip| !ip.is_loopback())
}

#[tokio::test]
async fn sandbox_blocks_host_and_private_egress_and_inherits_no_descriptors() {
    let f = Fixture::new();
    let host_listener = std::net::TcpListener::bind("0.0.0.0:0").expect("bind host listener");
    let host_port = host_listener.local_addr().expect("local addr").port();
    let host_ip = host_non_loopback_ipv4().expect("host has a non-loopback IPv4 address");

    let net_script = format!(
        r#"import http.server, json, os, socket

PORT = int(os.environ["PORT"])
# Before this process opens anything of its own: every inherited descriptor.
def target(fd):
    try:
        return os.readlink(f"/proc/self/fd/{{fd}}")
    except FileNotFoundError:
        return "/proc/listing"  # the descriptor listdir used, closed since
INHERITED = {{fd: target(fd) for fd in os.listdir("/proc/self/fd")}}

def probe(host, port):
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.settimeout(0.5)
    try:
        s.connect((host, port))
        return "CONNECTED"
    except OSError as e:
        return "ERR:" + type(e).__name__ + ":" + str(e)
    finally:
        s.close()

class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        body = json.dumps({{
            "inherited": INHERITED,
            "host_loopback": probe("127.0.0.1", {host_port}),
            "slirp_host_gw": probe("10.0.2.2", {host_port}),
            "host_address": probe("{host_ip}", {host_port}),
            "env_host": os.environ.get("HOST"),
        }}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, format, *args):
        pass

extra = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
extra.bind(("127.0.0.1", PORT + 1))
extra.listen(1)
http.server.ThreadingHTTPServer(("0.0.0.0", PORT), H).serve_forever()
"#
    );
    f.write_app("netiso", &net_script);
    f.cleanup.register("netiso");
    let apps = f.apps();

    let started = apps
        .start(
            f.agent,
            None,
            &f.workspace,
            "netiso",
            "netiso",
            "python3 server.py",
        )
        .await
        .expect("an app may bind 0.0.0.0 and a second port inside its own namespace");
    let probes = get_json(format!("http://127.0.0.1:{}/", started.port)).await;
    drop(host_listener);
    for (probe, what) in [
        ("host_loopback", "host 127.0.0.1 services"),
        ("slirp_host_gw", "host loopback through slirp's 10.0.2.2"),
        ("host_address", "the host's own non-loopback address"),
    ] {
        assert!(
            probes[probe]
                .as_str()
                .is_some_and(|s| s.starts_with("ERR:")),
            "sandbox must not reach {what}: {probes}"
        );
    }
    let inherited = probes["inherited"].as_object().expect("inherited map");
    let leaked: Vec<_> = inherited
        .iter()
        .filter(|(fd, target)| {
            !matches!(fd.as_str(), "0" | "1" | "2")
                && !target.as_str().is_some_and(|t| t.starts_with("/proc/"))
        })
        .collect();
    assert!(
        leaked.is_empty(),
        "app inherited descriptors: {inherited:?}"
    );
    assert_eq!(probes["env_host"], "127.0.0.1");
}

/// Opens `count` idle connections to `port` and returns them.
fn idle_connections(port: u16, count: usize) -> Vec<std::net::TcpStream> {
    (0..count)
        .map(|_| std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect"))
        .collect()
}

#[tokio::test]
async fn a_connection_flood_does_not_take_the_app_down() {
    let f = Fixture::new();
    f.write_app("flood", TEST_HTTP_SERVER);
    f.cleanup.register("flood");
    let apps = f.apps();
    let started = apps
        .start(
            f.agent,
            None,
            &f.workspace,
            "flood",
            "flood",
            "python3 server.py",
        )
        .await
        .expect("start flood");
    let main_pid = unit_main_pid_sync(&f.database, "flood");

    let flood = idle_connections(started.port, 2 * super::model::MAX_APP_CONNECTIONS as usize);
    sleep(Duration::from_secs(1)).await;
    drop(flood);

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(response) = client()
            .get(format!("http://127.0.0.1:{}/", started.port))
            .send()
            .await
        {
            assert!(response.status().is_success());
            break;
        }
        assert!(
            Instant::now() < deadline,
            "app stopped answering after a flood"
        );
        sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        unit_main_pid_sync(&f.database, "flood"),
        main_pid,
        "the runner must survive a flood without a restart"
    );
}

#[tokio::test]
async fn a_client_that_stops_reading_does_not_grow_the_bridge() {
    let f = Fixture::new();
    let script = r#"import http.server, os
PORT = int(os.environ["PORT"])
CHUNK = b"x" * (1024 * 1024)
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Length", str(len(CHUNK) * 2048))
        self.end_headers()
        for sent in range(2048):
            self.wfile.write(CHUNK)
            with open("/tmp/sent", "w") as f:
                f.write(str(sent + 1))
    def log_message(self, format, *args):
        pass
http.server.ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
"#;
    f.write_app("hose", script);
    f.cleanup.register("hose");
    let apps = f.apps();
    let started = apps
        .start(
            f.agent,
            None,
            &f.workspace,
            "hose",
            "hose",
            "python3 server.py",
        )
        .await
        .expect("start hose");
    let main_pid = unit_main_pid_sync(&f.database, "hose");

    let mut stalled = std::net::TcpStream::connect(("127.0.0.1", started.port)).expect("connect");
    std::io::Write::write_all(&mut stalled, b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").expect("send");
    sleep(Duration::from_secs(3)).await;

    let sent_path = f.state_app_dirs()[0].join("scratch/sent");
    let sent_mib: u64 = fs::read_to_string(&sent_path)
        .expect("progress file")
        .parse()
        .expect("progress count");
    assert!(
        sent_mib < 64,
        "the app wrote {sent_mib} MiB to a client that reads nothing"
    );
    assert_eq!(unit_main_pid_sync(&f.database, "hose"), main_pid);
    drop(stalled);
}

#[test]
fn every_apps_handle_for_one_database_shares_its_locks() {
    let f = Fixture::new();
    let tool = Apps::from_database(&f.database);
    let gateway = f.apps();
    assert!(std::sync::Arc::ptr_eq(
        &tool.lock_for("a"),
        &gateway.lock_for("a")
    ));
    let _stream = gateway.acquire_stream("a");
    assert_eq!(tool.active_stream_count("a"), 1);
}

#[test]
fn staging_units_name_their_owner_and_cannot_collide_with_an_app() {
    let f = Fixture::new();
    let prefix = super::unit_names::unit_prefix_for(&f.database);
    let stem = |unit: &str| {
        unit.strip_prefix(&prefix)
            .and_then(|rest| rest.strip_suffix(".service"))
            .expect("unit stem")
            .to_owned()
    };
    let staging = stem(&super::unit_names::staging_unit_name(&f.database, "x", 2));
    let owner = super::unit_names::unit_owner(&staging);
    assert_eq!((owner.name, owner.staging), ("x", true));

    let lookalike = stem(&super::unit_names::unit_name(&f.database, "x-stage-1-2"));
    let owner = super::unit_names::unit_owner(&lookalike);
    assert_eq!((owner.name, owner.staging), ("x-stage-1-2", false));
}

#[test]
fn sweeping_orphan_dirs_keeps_only_catalogued_apps() {
    let f = Fixture::new();
    let apps_root = f.home.apps();
    let kept_id = Uuid::new_v4().to_string();
    let record = super::LiveApp {
        name: "kept".to_owned(),
        agent_id: f.agent.to_string(),
        source_folder: "kept".to_owned(),
        command: "true".to_owned(),
        port: 20_000,
        state: AppState::Stopped,
        failure_reason: None,
        updated_at_ms: 0,
        last_active_ms: 0,
    };
    super::store::insert_new_app(&f.database, &record, None, &kept_id).expect("catalogue app");
    fs::create_dir_all(apps_root.join(&kept_id).join("scratch")).expect("kept dir");
    fs::create_dir_all(apps_root.join(".stage-left-over/scratch")).expect("stage dir");
    fs::create_dir_all(apps_root.join("no-such-app")).expect("orphan dir");
    assert_eq!(f.apps().sweep_orphan_dirs().expect("sweep"), 2);
    assert_eq!(f.state_app_dirs(), vec![apps_root.join(&kept_id)]);
}

#[tokio::test]
async fn port_squatting_detection_rejects_external_host_listener() {
    let f = Fixture::new();
    f.write_app("legit", TEST_HTTP_SERVER);
    f.cleanup.register("legit");
    let apps = f.apps();
    apps.start(
        f.agent,
        None,
        &f.workspace,
        "legit",
        "legit",
        "python3 server.py",
    )
    .await
    .expect("start legit");

    let main_pid = unit_main_pid_sync(&f.database, "legit");
    assert_ne!(main_pid, 0);
    let squatter = std::net::TcpListener::bind("127.0.0.1:0").expect("bind squatter");
    let squatted_port = squatter.local_addr().expect("squatter addr").port();

    let verdict = super::health::check_unit_listener(main_pid, squatted_port);
    assert_eq!(
        verdict,
        Err(super::health::ListenerViolation::PortSquatted(
            squatted_port
        ))
    );
}

#[tokio::test]
async fn inactive_unit_transitions_running_app_to_stopped_not_failed() {
    let f = Fixture::new();
    f.write_app("graceful", TEST_HTTP_SERVER);
    f.cleanup.register("graceful");
    let apps = f.apps();
    apps.start(
        f.agent,
        None,
        &f.workspace,
        "graceful",
        "graceful",
        "python3 server.py",
    )
    .await
    .expect("start graceful");

    stop_unit_sync(&f.database, "graceful");
    assert_eq!(unit_main_pid_sync(&f.database, "graceful"), 0);

    let status = apps
        .status_for("graceful", None)
        .await
        .expect("status after inactive unit");
    assert_eq!(status.state, AppState::Stopped);
    assert_eq!(status.failure_reason, None);
}
