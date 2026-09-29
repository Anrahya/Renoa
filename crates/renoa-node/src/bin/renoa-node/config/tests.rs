use serde_json::json;
use uuid::Uuid;

use super::*;

#[cfg(unix)]
fn private(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .expect("protect test file");
}

#[test]
fn config_is_versioned_strict_and_accepts_an_oauth_relay() {
    let files = tempfile::tempdir().expect("temporary directory");
    let bridge = files.path().join("bridge.mjs");
    let credential_store = files.path().join("model.sqlite");
    let relay_device = files.path().join("oauth-relay-device");
    std::fs::write(&bridge, "").expect("write bridge");
    std::fs::write(&credential_store, "").expect("write model store");
    std::fs::write(&relay_device, "").expect("write relay device credential");
    let base = json!({
        "schemaVersion": 5,
        "endpoint": "ws://127.0.0.1:9/connect",
        "automationCredentials": files.path().join("automations.json"),
        "model": {
            "bridge": bridge,
            "credentialStore": credential_store,
            "providers": ["xai"],
            "defaultProvider": "xai",
            "defaultModel": "fixture-model"
        },
        "adapters": {
            "oauthRelay": {"origin": "https://renoa.live", "credentials": relay_device}
        }
    });
    let path = files.path().join("node.json");
    std::fs::write(&path, serde_json::to_vec(&base).expect("encode config")).expect("write config");
    #[cfg(unix)]
    private(&path);
    let decoded = decode_config(&path).expect("decode strict config");
    validate_adapters(&decoded.adapters).expect("the relay credential exists");

    for (field, value) in [
        ("unexpected", json!(true)),
        ("targets", json!([])),
        ("schemaVersion", json!(1)),
        ("automationCredentials", json!(null)),
    ] {
        let mut changed = base.clone();
        changed[field] = value;
        std::fs::write(&path, serde_json::to_vec(&changed).expect("encode config"))
            .expect("write changed config");
        assert!(decode_config(&path).is_err(), "{field} must be refused");
    }
}

#[test]
fn an_earlier_config_document_is_refused_by_version_not_by_a_malformed_field() {
    let files = tempfile::tempdir().expect("temporary directory");
    let path = files.path().join("node.json");
    let legacy = json!({
        "schemaVersion": 1,
        "endpoint": "ws://127.0.0.1:9/connect",
        "model": {
            "bridge": "/opt/renoa/adapters/model-provider-node/dist/src/main.js",
            "credentialStore": "/var/lib/renoa-node/model-auth.sqlite",
            "providers": ["opencode-go"],
            "defaultProvider": "opencode-go",
            "defaultModel": "fixture-model"
        },
        "targets": [{
            "target": "workspace:example",
            "profile": "renoa.coding.alpha.v3",
            "sessionId": Uuid::new_v4(),
            "workspace": "/srv/renoa/node-workspaces/example"
        }]
    });
    std::fs::write(&path, serde_json::to_vec(&legacy).expect("encode config"))
        .expect("write legacy config");
    #[cfg(unix)]
    private(&path);

    let error = decode_config(&path)
        .err()
        .expect("an earlier document is refused");
    let message = error.to_string();
    assert!(
        message.contains("unsupported node config schema 1"),
        "{message}"
    );
    assert!(message.contains("expected 5"), "{message}");
    assert!(
        message.contains("profile") && message.contains("agentId"),
        "{message}"
    );
}

#[test]
fn a_document_with_static_targets_is_told_to_remove_them() {
    let files = tempfile::tempdir().expect("temporary directory");
    let path = files.path().join("node.json");
    let legacy = json!({
        "schemaVersion": 3,
        "endpoint": "ws://127.0.0.1:9/connect",
        "model": {
            "bridge": "/opt/renoa/adapters/model-provider-node/dist/src/main.js",
            "credentialStore": "/var/lib/renoa-node/model-auth.sqlite",
            "providers": ["opencode-go"],
            "defaultProvider": "opencode-go",
            "defaultModel": "fixture-model"
        },
        "targets": [{
            "target": "workspace:example",
            "agentId": Uuid::new_v4(),
            "workspace": "/srv/renoa/node-workspaces/example"
        }]
    });
    std::fs::write(&path, serde_json::to_vec(&legacy).expect("encode config"))
        .expect("write legacy config");
    #[cfg(unix)]
    private(&path);

    let message = decode_config(&path)
        .err()
        .expect("a version 3 document is refused")
        .to_string();
    assert!(
        message.contains("unsupported node config schema 3"),
        "{message}"
    );
    assert!(message.contains("targets"), "{message}");
}

#[test]
fn a_document_without_an_automation_surface_is_told_to_enroll_one() {
    let files = tempfile::tempdir().expect("temporary directory");
    let path = files.path().join("node.json");
    let legacy = json!({
        "schemaVersion": 4,
        "endpoint": "ws://127.0.0.1:9/connect",
        "model": {
            "bridge": "/opt/renoa/adapters/model-provider-node/dist/src/main.js",
            "credentialStore": "/var/lib/renoa-node/model-auth.sqlite",
            "providers": ["opencode-go"],
            "defaultProvider": "opencode-go",
            "defaultModel": "fixture-model"
        }
    });
    std::fs::write(&path, serde_json::to_vec(&legacy).expect("encode config"))
        .expect("write legacy config");
    #[cfg(unix)]
    private(&path);

    let message = decode_config(&path)
        .err()
        .expect("a version 4 document is refused")
        .to_string();
    assert!(
        message.contains("unsupported node config schema 4"),
        "{message}"
    );
    assert!(
        message.contains("automations") && message.contains("automationCredentials"),
        "{message}"
    );
}

#[cfg(unix)]
#[test]
fn the_node_credential_cannot_also_submit_automations() {
    let files = tempfile::tempdir().expect("temporary directory");
    let bridge = files.path().join("bridge.mjs");
    let credential_store = files.path().join("model.sqlite");
    let config = files.path().join("node.json");
    let credentials = files.path().join("device.json");
    let state = files.path().join("uncreated-state");
    std::fs::write(&bridge, "").expect("write bridge");
    std::fs::write(&credential_store, "").expect("write model store");
    std::fs::write(
        &config,
        serde_json::to_vec(&json!({
            "schemaVersion": 5,
            "endpoint": "ws://127.0.0.1:9/connect",
            "automationCredentials": credentials,
            "model": {
                "bridge": bridge,
                "credentialStore": credential_store,
                "providers": ["xai"],
                "defaultProvider": "xai",
                "defaultModel": "fixture-model"
            }
        }))
        .expect("encode config"),
    )
    .expect("write config");
    write_credentials(&credentials);
    private(&config);

    let error = load(&config, &credentials, &state)
        .err()
        .expect("one credential for both roles must be refused");
    assert!(
        error.to_string().contains("automationCredentials"),
        "{error}"
    );
    assert!(
        !state.exists(),
        "a refused config leaves no state directory"
    );
}

fn write_credentials(path: &Path) {
    std::fs::write(
        path,
        serde_json::to_vec(&json!({
            "deviceId": Uuid::new_v4(),
            "credential": "00".repeat(32)
        }))
        .expect("encode credentials"),
    )
    .expect("write credentials");
    #[cfg(unix)]
    private(path);
}

#[test]
fn credential_document_rejects_unknown_fields() {
    let files = tempfile::tempdir().expect("temporary directory");
    let path = files.path().join("device.json");
    let credential = json!({
        "deviceId": Uuid::new_v4(),
        "credential": "00".repeat(32),
        "unexpected": true
    });
    std::fs::write(
        &path,
        serde_json::to_vec(&credential).expect("encode credential"),
    )
    .expect("write credential");
    #[cfg(unix)]
    private(&path);

    assert!(decode_credentials(&path).is_err());
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn wrong_code_mode_worker_is_refused_before_state_directory_creation() {
    use std::os::unix::fs::PermissionsExt as _;

    let files = tempfile::tempdir().expect("temporary directory");
    let bridge = files.path().join("bridge.mjs");
    let credential_store = files.path().join("model.sqlite");
    let worker = files.path().join("wrong-monty");
    let config = files.path().join("node.json");
    let credentials = files.path().join("device.json");
    let automations = files.path().join("automations.json");
    let state = files.path().join("uncreated-state");
    std::fs::write(&bridge, "").expect("write bridge");
    std::fs::write(&credential_store, "").expect("write model store");
    std::fs::write(&worker, "not the pinned worker").expect("write wrong worker");
    std::fs::set_permissions(&worker, std::fs::Permissions::from_mode(0o755))
        .expect("make worker executable");
    std::fs::write(
        &config,
        serde_json::to_vec(&json!({
            "schemaVersion": 5,
            "endpoint": "ws://127.0.0.1:9/connect",
            "automationCredentials": automations,
            "model": {
                "bridge": bridge,
                "credentialStore": credential_store,
                "providers": ["xai"],
                "defaultProvider": "xai",
                "defaultModel": "fixture-model"
            },
            "adapters": {"codeModeWorker": worker}
        }))
        .expect("encode config"),
    )
    .expect("write config");
    write_credentials(&credentials);
    write_credentials(&automations);
    private(&config);
    let error = load(&config, &credentials, &state)
        .err()
        .expect("wrong worker must be refused");
    assert!(error.to_string().contains("hash mismatch"), "{error}");
    assert!(
        !state.exists(),
        "invalid worker must leave no state directory"
    );
}

#[cfg(unix)]
#[test]
fn state_directory_symlink_is_rejected_without_changing_its_target() {
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    let files = tempfile::tempdir().expect("temporary directory");
    let real = files.path().join("real");
    let linked = files.path().join("linked");
    std::fs::create_dir(&real).expect("create real state directory");
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o755))
        .expect("set initial mode");
    symlink(&real, &linked).expect("link state directory");

    assert!(prepare_state_directory(&linked).is_err());
    assert_eq!(
        std::fs::metadata(&real)
            .expect("real directory metadata")
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
}
