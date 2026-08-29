use std::{fs, path::Path, process::Command};

use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use super::{McpConnectionAuth, McpCredentialError, McpCredentialResolver};

#[tokio::test]
async fn gh_token_resolution_uses_the_exact_stored_account() {
    let directory = tempdir().expect("temporary credential fixture");
    let arguments = directory.path().join("arguments.txt");
    let executable = compile_fixture(
        directory.path(),
        &format!(
            r#"
fn main() {{
    let arguments = std::env::args().skip(1).collect::<Vec<_>>().join("\n");
    std::fs::write({arguments:?}, arguments).expect("write arguments");
    println!("fixture-secret-token");
}}
"#
        ),
    );
    let resolver = McpCredentialResolver::with_gh_executable(executable);
    let reference = McpConnectionAuth::gh_cli("github.com", "Anrahya").expect("valid reference");

    let authorization = resolver
        .resolve(&reference, CancellationToken::new())
        .await
        .expect("resolve fixture credential")
        .expect("gh reference resolves authorization");

    assert_eq!(authorization.bearer(), "fixture-secret-token");
    assert_eq!(
        fs::read_to_string(arguments).expect("read exact gh arguments"),
        "auth\ntoken\n--hostname\ngithub.com\n--user\nAnrahya"
    );
}

#[tokio::test]
async fn failed_gh_lookup_never_returns_its_output() {
    let directory = tempdir().expect("temporary credential fixture");
    let executable = compile_fixture(
        directory.path(),
        r#"
fn main() {
    println!("must-not-leak");
    eprintln!("also-must-not-leak");
    std::process::exit(7);
}
"#,
    );
    let resolver = McpCredentialResolver::with_gh_executable(executable);
    let reference = McpConnectionAuth::gh_cli("github.com", "Anrahya").expect("valid reference");

    let Err(error) = resolver.resolve(&reference, CancellationToken::new()).await else {
        panic!("failed gh lookup must be typed");
    };
    let message = error.to_string();

    assert!(matches!(error, McpCredentialError::Unavailable { .. }));
    assert!(!message.contains("must-not-leak"));
    assert!(message.contains("gh auth status --hostname github.com"));
}

#[tokio::test]
async fn secret_service_resolution_uses_only_the_exact_credential_reference() {
    let directory = tempdir().expect("temporary credential fixture");
    let arguments = directory.path().join("arguments.txt");
    let executable = compile_fixture(
        directory.path(),
        &format!(
            r#"
fn main() {{
    let arguments = std::env::args().skip(1).collect::<Vec<_>>().join("\n");
    std::fs::write({arguments:?}, arguments).expect("write arguments");
    println!("exa-fixture-secret");
}}
"#
        ),
    );
    let resolver =
        McpCredentialResolver::with_executables(directory.path().join("unused-gh"), executable);
    let reference = McpConnectionAuth::secret_service_bearer("exa.default")
        .expect("valid Secret Service reference");

    let authorization = resolver
        .resolve(&reference, CancellationToken::new())
        .await
        .expect("resolve fixture credential")
        .expect("Secret Service reference resolves authorization");

    assert_eq!(authorization.bearer(), "exa-fixture-secret");
    assert_eq!(
        fs::read_to_string(arguments).expect("read exact secret-tool arguments"),
        "lookup\napplication\nrenoa\ncredential\nexa.default"
    );
}

#[tokio::test]
async fn cancelling_gh_lookup_stops_the_credential_process_before_returning() {
    let directory = tempdir().expect("temporary credential fixture");
    let started = directory.path().join("started");
    let executable = compile_fixture(
        directory.path(),
        &format!(
            r#"
fn main() {{
    std::fs::write({started:?}, std::process::id().to_string()).expect("write pid");
    loop {{ std::thread::sleep(std::time::Duration::from_secs(60)); }}
}}
"#
        ),
    );
    let resolver = McpCredentialResolver::with_gh_executable(executable);
    let reference = McpConnectionAuth::gh_cli("github.com", "Anrahya").expect("valid reference");
    let cancellation = CancellationToken::new();
    let running_cancellation = cancellation.clone();
    let running =
        tokio::spawn(async move { resolver.resolve(&reference, running_cancellation).await });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !started.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("credential process did not start");

    cancellation.cancel();
    let result = tokio::time::timeout(std::time::Duration::from_secs(3), running)
        .await
        .expect("credential cancellation must settle promptly")
        .expect("join credential task");
    let Err(error) = result else {
        panic!("cancelled credential lookup must fail");
    };

    assert!(matches!(error, McpCredentialError::Cancelled));
}

#[test]
fn stored_credential_references_fail_closed() {
    assert!(McpConnectionAuth::from_stored("none", None, None, None, None).is_ok());
    assert!(
        McpConnectionAuth::from_stored(
            "gh_cli",
            Some("github.com".to_owned()),
            Some("Anrahya".to_owned()),
            None,
            None,
        )
        .is_ok()
    );
    assert!(
        McpConnectionAuth::from_stored("gh_cli", Some("github.com".to_owned()), None, None, None,)
            .is_err()
    );
    assert!(
        McpConnectionAuth::from_stored(
            "secret_service_bearer",
            None,
            None,
            Some("exa.default".to_owned()),
            None,
        )
        .is_ok()
    );
    assert!(McpConnectionAuth::gh_cli("github.com/bad", "Anrahya").is_err());

    let oauth = McpConnectionAuth::oauth(
        "search.default",
        "https://first.example/mcp",
        super::McpOAuthRegistration::dynamic(),
    )
    .expect("create endpoint-bound OAuth reference");
    assert!(
        oauth
            .validate_oauth_binding("search.default", "https://first.example/mcp")
            .is_ok()
    );
    assert!(
        oauth
            .validate_oauth_binding("search.default", "https://second.example/mcp")
            .is_err()
    );
}

fn compile_fixture(directory: &Path, source: &str) -> std::path::PathBuf {
    let source_path = directory.join("fake-gh.rs");
    let executable = directory.join(if cfg!(windows) {
        "fake-gh.exe"
    } else {
        "fake-gh"
    });
    fs::write(&source_path, source).expect("write fake gh source");
    let status = Command::new("rustc")
        .args(["--edition", "2024"])
        .arg(&source_path)
        .arg("-o")
        .arg(&executable)
        .status()
        .expect("compile fake gh");
    assert!(status.success(), "fake gh compilation failed: {status}");
    executable
}
