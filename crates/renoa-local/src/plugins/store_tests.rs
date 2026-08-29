use std::{fs, path::Path};

use serde_json::Value;
use tempfile::tempdir;

use super::{PluginError, store::PluginStore, tests::write_exa_plugin};
use crate::host::catalog;

#[test]
fn a_pre_v7_missing_homepage_is_recovered_from_the_immutable_package() {
    let directory = tempdir().expect("temporary legacy plugin store");
    let database = directory.path().join("host.sqlite3");
    catalog::initialize(&database).expect("initialize Host catalog");
    let packages = directory.path().join("packages");
    let source = directory.path().join("source");
    fs::create_dir(&source).expect("create plugin source");
    write_exa_plugin(&source, "https://mcp.exa.ai/mcp");
    let manifest_path = source.join("plugin.json");
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(&manifest_path).expect("read source manifest"))
            .expect("decode source manifest");
    manifest["homepage"] = Value::String("https://exa.ai/docs".to_owned());
    fs::write(
        &manifest_path,
        serde_json::to_vec(&manifest).expect("encode source manifest"),
    )
    .expect("add fixture homepage");
    let store =
        PluginStore::initialize(database.clone(), packages).expect("initialize plugin store");
    let inspected = super::inspect::inspect(&source)
        .expect("inspect source")
        .inspection;
    store
        .install(&source, inspected.digest())
        .expect("install source with homepage");
    let connection = rusqlite::Connection::open(&database).expect("open legacy metadata fixture");
    connection
        .execute(
            "UPDATE installed_plugins SET homepage = NULL WHERE plugin_digest = ?1",
            [inspected.digest()],
        )
        .expect("reproduce pre-v7 missing homepage");

    let loaded = store
        .load(inspected.digest())
        .expect("recover homepage from immutable package");

    assert_eq!(loaded.metadata().homepage(), Some("https://exa.ai/docs"));
    assert_eq!(
        connection
            .query_row(
                "SELECT homepage FROM installed_plugins WHERE plugin_digest = ?1",
                [inspected.digest()],
                |row| row.get::<_, String>(0),
            )
            .expect("read recovered durable homepage"),
        "https://exa.ai/docs"
    );

    connection
        .execute(
            "UPDATE installed_plugins SET homepage = 'https://wrong.example' \
             WHERE plugin_digest = ?1",
            [inspected.digest()],
        )
        .expect("inject a non-legacy metadata conflict");
    assert!(matches!(
        store.load(inspected.digest()),
        Err(PluginError::Conflict(_))
    ));
}

#[test]
fn installation_is_content_addressed_idempotent_and_source_bound() {
    let directory = tempdir().expect("temporary plugin store");
    let database = directory.path().join("host.sqlite3");
    catalog::initialize(&database).expect("initialize Host catalog");
    let packages = directory.path().join("packages");
    let source = directory.path().join("source");
    fs::create_dir(&source).expect("create plugin source");
    write_exa_plugin(&source, "https://mcp.exa.ai/mcp");
    let store =
        PluginStore::initialize(database, packages.clone()).expect("initialize plugin store");
    let inspected = super::inspect::inspect(&source)
        .expect("inspect source")
        .inspection;
    let first = store
        .install(&source, inspected.digest())
        .expect("install exact source");
    let second = store
        .install(&source, inspected.digest())
        .expect("repeat exact install");
    assert_eq!(first, second);
    let installed = store.list().expect("list installed packages");
    assert_eq!(installed.as_slice(), std::slice::from_ref(&first));

    let mut manifest: Value = serde_json::from_slice(
        &fs::read(source.join("plugin.json")).expect("read source manifest"),
    )
    .expect("decode source manifest");
    manifest["version"] = Value::String("3.4.2".to_owned());
    fs::write(
        source.join("plugin.json"),
        serde_json::to_vec(&manifest).expect("encode changed manifest"),
    )
    .expect("change source after inspection");
    assert_eq!(
        store
            .install(&source, inspected.digest())
            .expect("an installed digest is authoritative over later source changes"),
        first
    );

    let installed_manifest = packages.join(inspected.digest()).join("plugin.json");
    make_writable(&packages.join(inspected.digest()), &installed_manifest);
    fs::write(&installed_manifest, b"{}").expect("tamper with installed content");
    assert!(matches!(
        store.load(inspected.digest()),
        Err(PluginError::Conflict(_) | PluginError::Invalid(_))
    ));
    let report = store
        .list_report()
        .expect("one corrupt package does not hide the rest of the list");
    assert!(report.installed.is_empty());
    assert_eq!(report.rejected.len(), 1);
    assert_eq!(report.rejected[0].package_digest, inspected.digest());
}

#[cfg(unix)]
#[test]
fn an_added_symlink_makes_an_installed_package_fail_closed() {
    use std::os::unix::fs::symlink;

    let directory = tempdir().expect("temporary plugin store");
    let database = directory.path().join("host.sqlite3");
    catalog::initialize(&database).expect("initialize Host catalog");
    let packages = directory.path().join("packages");
    let source = directory.path().join("source");
    fs::create_dir(&source).expect("create plugin source");
    write_exa_plugin(&source, "https://mcp.exa.ai/mcp");
    let store =
        PluginStore::initialize(database, packages.clone()).expect("initialize plugin store");
    let inspected = super::inspect::inspect(&source)
        .expect("inspect source")
        .inspection;
    store
        .install(&source, inspected.digest())
        .expect("install source");

    let installed = packages.join(inspected.digest());
    make_directory_writable(&installed);
    symlink(
        directory.path().join("outside"),
        installed.join("added-link"),
    )
    .expect("tamper with installed package");
    assert!(matches!(
        store.load(inspected.digest()),
        Err(PluginError::Conflict(_))
    ));
}

#[cfg(unix)]
fn make_writable(directory: &Path, file: &Path) {
    use std::os::unix::fs::PermissionsExt as _;

    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
        .expect("make installed directory writable for corruption fixture");
    fs::set_permissions(file, fs::Permissions::from_mode(0o600))
        .expect("make installed file writable for corruption fixture");
}

#[cfg(unix)]
fn make_directory_writable(directory: &Path) {
    use std::os::unix::fs::PermissionsExt as _;

    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
        .expect("make installed directory writable for corruption fixture");
}

#[cfg(not(unix))]
fn make_writable(_directory: &Path, file: &Path) {
    let mut permissions = fs::metadata(file)
        .expect("inspect installed file")
        .permissions();
    permissions.set_readonly(false);
    fs::set_permissions(file, permissions).expect("make installed file writable");
}
