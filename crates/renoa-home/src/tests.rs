use super::*;

#[test]
fn an_installation_has_one_owner_for_state_plugins_documents_and_workspaces() {
    let temp = tempfile::tempdir().expect("fixture");
    let home = RenoaHome::at(temp.path().join("installation")).expect("home");
    home.initialize().expect("layout");
    home.initialize().expect("adopt layout");
    let agent = uuid::Uuid::new_v4();
    assert_eq!(home.host_database(), home.path().join("state/host.sqlite3"));
    assert_eq!(
        home.agent_workspace(&agent.to_string()).expect("workspace"),
        home.path().join(format!("agents/{agent}/workspace"))
    );
    assert_eq!(
        home.model_credentials(),
        home.path().join("credentials/models.sqlite3")
    );
    assert!(
        home.surface_directory("discord")
            .expect("surface")
            .starts_with(home.path().join("state"))
    );
    assert!(home.surface_directory("../escape").is_err());
}
#[cfg(unix)]
#[test]
fn a_linked_owned_directory_is_refused_before_creating_any_layout() {
    let temp = tempfile::tempdir().expect("fixture");
    let root = temp.path().join("installation");
    fs::create_dir(&root).expect("root");
    let elsewhere = temp.path().join("elsewhere");
    fs::create_dir(&elsewhere).expect("elsewhere");
    std::os::unix::fs::symlink(&elsewhere, root.join("credentials")).expect("link");
    assert!(RenoaHome::at(&root).is_err());
    assert_eq!(fs::read_dir(&root).expect("root").count(), 1);
    assert_eq!(fs::read_dir(elsewhere).expect("elsewhere").count(), 0);
}

#[test]
fn home_resolution_uses_the_default_environment_and_explicit_precedence() {
    let directory = tempfile::tempdir().expect("fixture");
    let probe = |override_home: Option<&Path>, explicit: Option<&Path>, expected: &Path| {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "tests::resolution_probe"])
            .env_remove("RENOA_HOME")
            .env_remove("RENOA_HOME_TEST_EXPLICIT")
            .env("RENOA_HOME_TEST_EXPECTED", expected)
            .env("HOME", directory.path())
            .env("USERPROFILE", directory.path());
        if let Some(path) = override_home {
            command.env("RENOA_HOME", path);
        }
        if let Some(path) = explicit {
            command.env("RENOA_HOME_TEST_EXPLICIT", path);
        }
        let output = command.output().expect("probe");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    };
    probe(None, None, &directory.path().join(".renoa"));
    let service = directory.path().join("service");
    probe(Some(&service), None, &service);
    let explicit = directory.path().join("explicit");
    probe(Some(&service), Some(&explicit), &explicit);
    assert_eq!(
        fs::read_dir(directory.path()).unwrap().count(),
        0,
        "resolution is read-only"
    );
}

#[test]
fn resolution_probe() {
    let Some(expected) = env::var_os("RENOA_HOME_TEST_EXPECTED") else {
        return;
    };
    let explicit = env::var_os("RENOA_HOME_TEST_EXPLICIT").map(PathBuf::from);
    assert_eq!(
        RenoaHome::resolve(explicit).unwrap().path(),
        Path::new(&expected)
    );
}

#[cfg(unix)]
#[test]
fn agent_workspaces_refuse_links_and_new_owned_directories_are_private() {
    use std::os::unix::fs::PermissionsExt as _;
    let directory = tempfile::tempdir().expect("fixture");
    let home = RenoaHome::at(directory.path().join("home")).unwrap();
    home.initialize().unwrap();
    for name in DIRECTORIES {
        assert_eq!(
            fs::metadata(home.path().join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    let elsewhere = directory.path().join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    fs::create_dir(elsewhere.join("workspace")).unwrap();
    let agent = uuid::Uuid::new_v4();
    std::os::unix::fs::symlink(
        &elsewhere,
        home.path().join("agents").join(agent.to_string()),
    )
    .unwrap();
    assert!(home.initialize_agent_workspace(&agent.to_string()).is_err());
    assert_eq!(fs::read_dir(elsewhere).unwrap().count(), 1);
    let workspace = home
        .initialize_agent_workspace(&uuid::Uuid::new_v4().to_string())
        .unwrap();
    assert_eq!(
        fs::metadata(workspace).unwrap().permissions().mode() & 0o777,
        0o700
    );
}
