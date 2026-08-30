use std::{
    io::{BufRead as _, BufReader},
    path::Path,
    process::{Child, Command, Stdio},
};

#[cfg(unix)]
use std::{process::ExitStatus, thread, time::Duration, time::Instant};

#[cfg(unix)]
use renoa_registry::Registry;
use renoa_registry_protocol::RegistryStatus;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ready {
    endpoint: String,
}

struct RegistryProcess(Child);

impl RegistryProcess {
    fn start(state: &Path) -> Self {
        let child = Command::new(
            std::env::var_os("CARGO_BIN_EXE_renoa-registry")
                .expect("Cargo did not build the renoa-registry binary"),
        )
        .arg("serve")
        .arg(state)
        .arg("0")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("start registry process");
        Self(child)
    }

    fn read_ready(&mut self) -> Ready {
        let stdout = self.0.stdout.take().expect("registry stdout");
        let mut line = String::new();
        let bytes = BufReader::new(stdout)
            .read_line(&mut line)
            .expect("read registry readiness");
        assert_ne!(bytes, 0, "registry exited before reporting readiness");
        serde_json::from_str(&line).expect("parse registry readiness")
    }

    #[cfg(unix)]
    fn terminate(&mut self) -> ExitStatus {
        let signal = Command::new("kill")
            .arg("-TERM")
            .arg(self.0.id().to_string())
            .status()
            .expect("send registry termination signal");
        assert!(signal.success(), "failed to signal registry");

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(status) = self.0.try_wait().expect("read registry status") {
                return status;
            }
            assert!(Instant::now() < deadline, "registry did not stop");
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for RegistryProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn registry_executable_serves_loopback_and_preserves_state_on_termination() {
    let directory = tempfile::tempdir().expect("temporary registry state");
    let mut process = RegistryProcess::start(directory.path());
    let ready = process.read_ready();
    assert!(
        ready.endpoint.starts_with("http://127.0.0.1:"),
        "registry must advertise only IPv4 loopback"
    );
    let status = reqwest::get(format!("{}/status", ready.endpoint))
        .await
        .expect("request registry status")
        .error_for_status()
        .expect("successful registry status")
        .json::<RegistryStatus>()
        .await
        .expect("decode registry status");
    assert_eq!(status.current_revision(), 0);

    #[cfg(unix)]
    {
        assert!(process.terminate().success());
        Registry::open(directory.path()).expect("reopen terminated registry");
    }
}
