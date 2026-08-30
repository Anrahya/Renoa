use std::{
    fs,
    path::{Path, PathBuf},
};

use renoa_local::{
    LocalHost, LocalHostAdapters, LocalModelConfiguration, ModelProvider, alpha_profile,
};
use renoa_registry::Registry;
use renoa_registry_protocol::RegistryStatus;
use tokio::{net::TcpListener, task::JoinHandle};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn two_live_hosts_converge_on_one_durable_plugin_library() {
    let root = tempfile::tempdir().expect("temporary test root");
    let registry_state = root.path().join("registry");
    let server = RunningRegistry::start(&registry_state).await;
    let host_a = local_host(root.path().join("host-a"), server.origin());
    let host_b_data = root.path().join("host-b");
    let host_b = local_host(host_b_data.clone(), server.origin());
    host_b
        .synchronize_shared_plugins()
        .await
        .expect("bind empty Host B without restarting it");

    let source = root.path().join("source");
    write_plugin(&source);
    let inspection = host_a
        .inspect_plugin(&source)
        .await
        .expect("inspect plugin");
    let digest = inspection.digest().to_owned();
    host_a
        .install_plugin(&source, &digest)
        .await
        .expect("install and publish from Host A");
    assert_eq!(server.status().await.current_revision(), 1);

    host_b
        .synchronize_shared_plugins()
        .await
        .expect("hot-load into the already-running Host B");
    let installed = host_b
        .installed_plugins()
        .await
        .expect("list Host B plugins");
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].digest(), digest);
    assert_eq!(
        fs::read(host_b_data.join("plugins").join(&digest).join("README.md"))
            .expect("read synchronized package"),
        b"shared package\n"
    );

    host_a
        .synchronize_shared_plugins()
        .await
        .expect("duplicate publication converges");
    host_b
        .synchronize_shared_plugins()
        .await
        .expect("duplicate pull converges");
    assert_eq!(server.status().await.current_revision(), 1);

    server.stop().await;
    let offline = local_host(host_b_data.clone(), "http://127.0.0.1:9/");
    assert!(offline.synchronize_shared_plugins().await.is_err());

    let offline_source = root.path().join("offline-source");
    write_named_plugin(&offline_source, "offline-proof");
    let offline_inspection = offline
        .inspect_plugin(&offline_source)
        .await
        .expect("inspect offline plugin");
    let error = offline
        .install_plugin(&offline_source, offline_inspection.digest())
        .await
        .expect_err("offline registry prevents complete reconciliation");
    assert!(error.to_string().contains("is installed locally"));
    drop(offline);
    let restarted = RunningRegistry::start(&registry_state).await;
    let reopened = local_host(host_b_data, restarted.origin());
    reopened
        .synchronize_shared_plugins()
        .await
        .expect("resume at the durable cursor after server and Host restart");
    assert_eq!(restarted.status().await.current_revision(), 2);
    restarted.stop().await;
}

#[tokio::test]
async fn a_host_refuses_a_different_registry_identity_without_losing_packages() {
    let root = tempfile::tempdir().expect("temporary test root");
    let first = RunningRegistry::start(&root.path().join("registry-a")).await;
    let data = root.path().join("host");
    let host = local_host(data.clone(), first.origin());
    host.synchronize_shared_plugins()
        .await
        .expect("bind first registry");
    first.stop().await;

    let second = RunningRegistry::start(&root.path().join("registry-b")).await;
    let rebound = local_host(data, second.origin());
    let error = rebound
        .synchronize_shared_plugins()
        .await
        .expect_err("registry identity change must fail closed");
    assert!(error.to_string().contains("bound to shared registry"));
    second.stop().await;
}

fn local_host(data: PathBuf, registry: &str) -> LocalHost {
    LocalHost::new(
        data,
        LocalModelConfiguration::new(
            "/unused/model-bridge",
            vec![ModelProvider::Xai],
            ModelProvider::Xai,
            "fixture-model",
            "/unused/credentials",
        ),
        vec![alpha_profile()],
        LocalHostAdapters::default().with_shared_plugin_registry(Some(registry)),
    )
    .expect("assemble local Host")
}

fn write_plugin(path: &Path) {
    write_named_plugin(path, "shared-proof");
}

fn write_named_plugin(path: &Path, name: &str) {
    fs::create_dir_all(path).expect("create plugin source");
    fs::write(
        path.join("plugin.json"),
        format!(
            r#"{{
          "$schema":"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
          "name":"{name}",
          "version":"1.0.0",
          "description":"Two-Host synchronization proof"
        }}"#
        ),
    )
    .expect("write plugin manifest");
    fs::write(path.join("README.md"), "shared package\n").expect("write package content");
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

#[cfg(unix)]
#[tokio::test]
async fn executable_files_survive_the_shared_package_round_trip() {
    use std::os::unix::fs::PermissionsExt as _;

    let root = tempfile::tempdir().expect("temporary test root");
    let server = RunningRegistry::start(&root.path().join("registry")).await;
    let source = root.path().join("source");
    write_plugin(&source);
    fs::write(source.join("run.sh"), "#!/bin/sh\n").expect("write executable");
    fs::set_permissions(source.join("run.sh"), fs::Permissions::from_mode(0o700))
        .expect("mark executable");
    let host_a = local_host(root.path().join("host-a"), server.origin());
    let inspection = host_a
        .inspect_plugin(&source)
        .await
        .expect("inspect plugin");
    host_a
        .install_plugin(&source, inspection.digest())
        .await
        .expect("publish executable package");
    let host_b_data = root.path().join("host-b");
    let host_b = local_host(host_b_data.clone(), server.origin());
    host_b
        .synchronize_shared_plugins()
        .await
        .expect("pull executable package");
    let mode = fs::metadata(
        host_b_data
            .join("plugins")
            .join(inspection.digest())
            .join("run.sh"),
    )
    .expect("inspect installed executable")
    .permissions()
    .mode();
    assert_ne!(mode & 0o111, 0);
    server.stop().await;
}
