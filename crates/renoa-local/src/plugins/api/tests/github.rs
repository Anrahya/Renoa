use super::*;
use flate2::{Compression, write::GzEncoder};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

const COMMIT: &str = "1234567890123456789012345678901234567890";

#[tokio::test]
async fn github_skill_intake_crosses_the_api_and_replays_without_another_download() {
    let root = tempfile::tempdir().expect("fixture");
    let (mut manager, skills) = manager(root.path());
    let skill = format!(
        "---\nname: review\ndescription: Review code.\nlicense: {}\n---\nRead references/details.md.\n",
        "é".repeat(600)
    );
    let selected = "plugins/review #1";
    let bytes = skill_archive(&skill, selected);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listen");
    manager.github_source = crate::plugins::intake::GithubSourceClient::fixture(format!(
        "http://{}",
        listener.local_addr().expect("address")
    ));
    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().await.expect("request");
            let mut request = [0; 4096];
            let count = stream.read(&mut request).await.expect("read");
            assert!(
                String::from_utf8_lossy(&request[..count])
                    .starts_with(&format!("GET /owner/repo/tar.gz/{COMMIT} "))
            );
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        bytes.len()
                    )
                    .as_bytes(),
                )
                .await
                .expect("headers");
            stream.write_all(&bytes).await.expect("body");
        }
    });
    let source = PluginSource::Github {
        repository: "https://github.com/owner/repo".to_owned(),
        commit: Some(COMMIT.to_owned()),
        r#ref: None,
        path: Some(selected.to_owned()),
        skill: None,
    };
    let digest = inspect(&manager, root.path(), source.clone()).await;
    assert!(
        manager
            .list()
            .await
            .expect("inspect is read only")
            .is_empty()
    );
    let request = || PluginRequest::Add {
        source: source.clone(),
        expected_digest: Some(digest.clone()),
        server: None,
        connection: None,
        credential: None,
        replace: false,
    };
    let PluginOutcome::Added(added) = invoke(&manager, root.path(), request())
        .await
        .expect("add remote skill")
    else {
        panic!("added")
    };
    server.await.expect("two immutable downloads");
    assert_eq!(
        added.installed.metadata().repository(),
        Some(format!("https://github.com/owner/repo/tree/{COMMIT}/plugins/review%20%231").as_str())
    );
    assert_eq!(added.skills.accepted(), ["review"]);
    assert_eq!(
        fs::read_to_string(
            root.path()
                .join("plugins")
                .join(&digest)
                .join("skills/review/SKILL.md")
        )
        .expect("original license and instructions"),
        skill
    );
    invoke(&manager, root.path(), request())
        .await
        .expect("retry works with source offline");
    assert_eq!(
        manager.list().await.expect("one immutable revision").len(),
        1
    );
    assert_eq!(
        skills
            .summaries(&test_agent_id(1).to_string(), root.path())
            .expect("enabled skill")
            .len(),
        1
    );
}

fn skill_archive(skill: &str, selected: &str) -> Vec<u8> {
    let mut archive = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    for (name, bytes) in [
        ("SKILL.md", skill.as_bytes()),
        (
            "references/details.md",
            b"Inspect implementation.".as_slice(),
        ),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o644);
        header.set_size(bytes.len() as u64);
        header.set_cksum();
        archive
            .append_data(
                &mut header,
                format!("Repo-{COMMIT}/{selected}/{name}"),
                bytes,
            )
            .expect("file");
    }
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_mode(0o777);
    header.set_size(0);
    header.set_cksum();
    archive
        .append_link(
            &mut header,
            format!("Repo-{COMMIT}/unrelated-link"),
            "/outside",
        )
        .expect("unrelated symlink");
    archive.into_inner().expect("tar").finish().expect("gzip")
}

#[tokio::test]
async fn github_multi_server_intake_requires_local_host_review_before_publication() {
    let root = tempfile::tempdir().unwrap();
    let (mut manager, _) = manager(root.path());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    manager.github_source = crate::plugins::intake::GithubSourceClient::fixture(format!(
        "http://{}",
        listener.local_addr().unwrap()
    ));
    let mut archive = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    for (name, bytes) in [
        (
            "plugin.json",
            serde_json::json!({"$schema":crate::plugins::inspect::PLUGIN_SCHEMA,"name":"cloud"})
                .to_string(),
        ),
        (
            "mcp.json",
            serde_json::json!({"$schema":crate::plugins::inspect::MCP_SCHEMA,"mcpServers":{
                "api":{"type":"streamable-http","url":"https://api.cloud.example/mcp"},
                "radar":{"type":"streamable-http","url":"https://radar.cloud.example/mcp"}
            }})
            .to_string(),
        ),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o644);
        header.set_size(bytes.len() as u64);
        header.set_cksum();
        archive
            .append_data(
                &mut header,
                format!("repo-{COMMIT}/{name}"),
                bytes.as_bytes(),
            )
            .unwrap();
    }
    let bytes = archive.into_inner().unwrap().finish().unwrap();
    let server = tokio::spawn(async move {
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            let count = stream.read(&mut request).await.unwrap();
            assert!(
                String::from_utf8_lossy(&request[..count])
                    .starts_with(&format!("GET /owner/repo/tar.gz/{COMMIT} "))
            );
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        bytes.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            stream.write_all(&bytes).await.unwrap();
        }
    });
    let source = PluginSource::Github {
        repository: "https://github.com/owner/repo".into(),
        commit: Some(COMMIT.into()),
        r#ref: None,
        path: None,
        skill: None,
    };
    let digest = inspect(&manager, root.path(), source.clone()).await;
    let request = || PluginRequest::Install {
        source: source.clone(),
        expected_digest: digest.clone(),
    };
    assert!(matches!(
        invoke(&manager, root.path(), request()).await,
        Err(PluginError::Invalid(_))
    ));
    assert!(!root.path().join("plugins").join(&digest).exists());
    assert!(manager.list().await.unwrap().is_empty());
    crate::plugins::coherence::define(
        &root.path().join("host.sqlite3"),
        &crate::plugins::PluginProviderFamily {
            family: "cloud".into(),
            origins: vec![
                "https://api.cloud.example".into(),
                "https://radar.cloud.example".into(),
            ],
        },
    )
    .unwrap();
    invoke(&manager, root.path(), request())
        .await
        .expect("retry after Host review");
    server.await.unwrap();
    assert_eq!(manager.list().await.unwrap().len(), 1);
}

#[derive(Clone)]
struct FixtureRepo {
    default_branch: String,
    refs: std::collections::BTreeMap<String, String>,
    archives: std::collections::BTreeMap<String, Vec<u8>>,
}

async fn start_fixture(
    manager: &mut crate::plugins::PluginManager,
    fixture: FixtureRepo,
) -> (
    std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listen");
    manager.github_source = crate::plugins::intake::GithubSourceClient::fixture(format!(
        "http://{}",
        listener.local_addr().expect("address")
    ));
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = seen.clone();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let fixture = fixture.clone();
            let recorded = recorded.clone();
            tokio::spawn(async move {
                let mut buf = [0_u8; 4096];
                let count = stream.read(&mut buf).await.expect("read");
                let text = String::from_utf8_lossy(&buf[..count]);
                let target = text
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or_default()
                    .to_owned();
                recorded.lock().expect("lock").push(target.clone());
                let (status, content_type, body) =
                    if let Some(encoded_ref) = target.strip_prefix("/repos/owner/repo/commits/") {
                        let decoded = encoded_ref.replace("%2F", "/");
                        let key = if decoded == "HEAD" {
                            fixture.default_branch.as_str()
                        } else {
                            decoded.as_str()
                        };
                        if let Some(sha) = fixture.refs.get(key) {
                            (
                                "200 OK",
                                "application/json",
                                serde_json::json!({"sha": sha}).to_string().into_bytes(),
                            )
                        } else {
                            ("404 Not Found", "application/json", b"{}".to_vec())
                        }
                    } else if let Some(sha) = target.strip_prefix("/owner/repo/tar.gz/") {
                        if let Some(archive) = fixture.archives.get(sha) {
                            ("200 OK", "application/gzip", archive.clone())
                        } else {
                            ("404 Not Found", "text/plain", b"missing".to_vec())
                        }
                    } else {
                        ("404 Not Found", "text/plain", b"unexpected".to_vec())
                    };
                let headers = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(headers.as_bytes()).await;
                let _ = stream.write_all(&body).await;
            });
        }
    });
    (seen, handle)
}

fn repo_archive(commit: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut archive = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    for (name, bytes) in files {
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o644);
        header.set_size(bytes.len() as u64);
        header.set_cksum();
        archive
            .append_data(&mut header, format!("repo-{commit}/{name}"), *bytes)
            .expect("append");
    }
    archive.into_inner().expect("tar").finish().expect("gzip")
}

async fn start_main_fixture(
    manager: &mut crate::plugins::PluginManager,
    archive: Vec<u8>,
) -> (
    std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    tokio::task::JoinHandle<()>,
) {
    start_fixture(
        manager,
        FixtureRepo {
            default_branch: "main".to_owned(),
            refs: std::collections::BTreeMap::from([("main".to_owned(), COMMIT.to_owned())]),
            archives: std::collections::BTreeMap::from([(COMMIT.to_owned(), archive)]),
        },
    )
    .await
}

fn github_source(
    commit: Option<&str>,
    reference: Option<&str>,
    path: Option<&str>,
    skill: Option<&str>,
) -> PluginSource {
    PluginSource::Github {
        repository: "https://github.com/owner/repo".to_owned(),
        commit: commit.map(str::to_owned),
        r#ref: reference.map(str::to_owned),
        path: path.map(str::to_owned),
        skill: skill.map(str::to_owned),
    }
}

async fn inspect_full(
    manager: &crate::plugins::PluginManager,
    workspace: &Path,
    source: PluginSource,
) -> crate::plugins::PluginInspection {
    let PluginOutcome::Inspected(inspection) =
        invoke(manager, workspace, PluginRequest::Inspect { source })
            .await
            .expect("inspect succeeds")
    else {
        panic!("expected inspected outcome");
    };
    inspection
}

#[tokio::test]
async fn inspect_branch_and_default_branch_resolve_to_commit_and_install_pins_sha() {
    let root = tempfile::tempdir().expect("fixture");
    let (mut manager, skills) = manager(root.path());
    let skill_md = "---\nname: review\ndescription: Review code.\n---\nFollow instructions.\n";
    let branch_commit = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let archive = repo_archive(branch_commit, &[("SKILL.md", skill_md.as_bytes())]);
    let default_archive = repo_archive(COMMIT, &[("SKILL.md", skill_md.as_bytes())]);
    let (seen, _server) = start_fixture(
        &mut manager,
        FixtureRepo {
            default_branch: "main".to_owned(),
            refs: std::collections::BTreeMap::from([
                ("main".to_owned(), COMMIT.to_owned()),
                ("release/v1".to_owned(), branch_commit.to_owned()),
            ]),
            archives: std::collections::BTreeMap::from([
                (branch_commit.to_owned(), archive),
                (COMMIT.to_owned(), default_archive),
            ]),
        },
    )
    .await;

    let branch_source = PluginSource::Github {
        repository: "https://github.com/owner/repo".to_owned(),
        commit: None,
        r#ref: Some("release/v1".to_owned()),
        path: None,
        skill: None,
    };
    let branch_inspection = inspect_full(&manager, root.path(), branch_source.clone()).await;
    let resolved_branch = branch_inspection
        .source()
        .expect("resolved pinned source")
        .clone();
    assert_eq!(
        resolved_branch,
        PluginSource::Github {
            repository: "https://github.com/owner/repo".to_owned(),
            commit: Some(branch_commit.to_owned()),
            r#ref: None,
            path: None,
            skill: None,
        }
    );

    // Unpinned refs are refused in add and install without touching the network.
    let before_count = seen.lock().expect("lock").len();
    for request in [
        PluginRequest::Add {
            source: branch_source.clone(),
            expected_digest: Some(branch_inspection.digest().to_owned()),
            server: None,
            connection: None,
            credential: None,
            replace: false,
        },
        PluginRequest::Install {
            source: branch_source,
            expected_digest: branch_inspection.digest().to_owned(),
        },
    ] {
        let err = invoke(&manager, root.path(), request)
            .await
            .err()
            .expect("unpinned add/install must fail");
        let text = err.to_string();
        assert!(text.contains("\"action\":\"inspect\""), "{text}");
    }
    assert_eq!(seen.lock().expect("lock").len(), before_count);

    // Installing the pinned source returned by inspect pins the commit SHA, never the branch.
    let PluginOutcome::Added(added) = invoke(
        &manager,
        root.path(),
        PluginRequest::Add {
            source: resolved_branch,
            expected_digest: Some(branch_inspection.digest().to_owned()),
            server: None,
            connection: None,
            credential: None,
            replace: false,
        },
    )
    .await
    .expect("add pinned branch commit") else {
        panic!("added");
    };
    assert_eq!(
        added.installed.metadata().repository(),
        Some(format!("https://github.com/owner/repo/tree/{branch_commit}").as_str())
    );
    assert_eq!(
        skills
            .summaries(&test_agent_id(1).to_string(), root.path())
            .expect("skills")
            .len(),
        1
    );
    let requests = seen.lock().expect("lock");
    assert!(
        requests
            .iter()
            .any(|path| path == "/repos/owner/repo/commits/release%2Fv1")
    );
}

#[tokio::test]
async fn inspect_omitted_ref_resolves_default_branch_to_commit() {
    let root = tempfile::tempdir().expect("fixture");
    let (mut manager, _skills) = manager(root.path());
    let skill_md = "---\nname: review\ndescription: Review code.\n---\nFollow instructions.\n";
    let default_archive = repo_archive(COMMIT, &[("SKILL.md", skill_md.as_bytes())]);
    let (seen, _server) = start_fixture(
        &mut manager,
        FixtureRepo {
            default_branch: "main".to_owned(),
            refs: std::collections::BTreeMap::from([("main".to_owned(), COMMIT.to_owned())]),
            archives: std::collections::BTreeMap::from([(COMMIT.to_owned(), default_archive)]),
        },
    )
    .await;

    let default_inspection = inspect_full(
        &manager,
        root.path(),
        PluginSource::Github {
            repository: "https://github.com/owner/repo".to_owned(),
            commit: None,
            r#ref: None,
            path: None,
            skill: None,
        },
    )
    .await;
    assert_eq!(
        default_inspection.source(),
        Some(&PluginSource::Github {
            repository: "https://github.com/owner/repo".to_owned(),
            commit: Some(COMMIT.to_owned()),
            r#ref: None,
            path: None,
            skill: None,
        })
    );
    let requests = seen.lock().expect("lock");
    assert!(
        requests
            .iter()
            .any(|path| path == "/repos/owner/repo/commits/HEAD")
    );
    assert!(
        !requests.iter().any(|path| path == "/repos/owner/repo"),
        "omitted ref should resolve in a single commits/HEAD call: {requests:?}"
    );
}

#[tokio::test]
async fn multi_skill_repo_lists_skills_and_skill_selector_picks_one() {
    let root = tempfile::tempdir().expect("fixture");
    let (mut manager, skills) = manager(root.path());
    let frontmatter_alpha = "---\nname: alpha\ndescription: Alpha skill.\n---\nAlpha body.\n";
    let beta_skill = "---\nname: beta\ndescription: Beta skill.\n---\nBeta body.\n";
    let fallback_skill = "No YAML frontmatter here; fallback name comes from folder.\n";
    let archive = repo_archive(
        COMMIT,
        &[
            ("packages/custom-dir/SKILL.md", frontmatter_alpha.as_bytes()),
            ("packages/beta/SKILL.md", beta_skill.as_bytes()),
            ("packages/folder-only/SKILL.md", fallback_skill.as_bytes()),
        ],
    );
    let _fixture = start_main_fixture(&mut manager, archive).await;

    let err = invoke(
        &manager,
        root.path(),
        PluginRequest::Inspect {
            source: github_source(None, Some("main"), None, None),
        },
    )
    .await
    .err()
    .expect("multi-skill inspect without skill must list skills");
    let text = err.to_string();
    assert!(
        text.contains("\"name\":\"alpha\",\"path\":\"packages/custom-dir\"")
            && text.contains("\"name\":\"beta\",\"path\":\"packages/beta\"")
            && text.contains("\"name\":\"folder-only\",\"path\":\"packages/folder-only\""),
        "{text}"
    );
    assert!(text.contains("\"action\":\"inspect\""), "{text}");

    let inspection = inspect_full(
        &manager,
        root.path(),
        github_source(Some(COMMIT), None, None, Some("alpha")),
    )
    .await;
    let selected_source = inspection.source().expect("selected source").clone();
    assert_eq!(
        selected_source,
        github_source(Some(COMMIT), None, Some("packages/custom-dir"), None)
    );

    let PluginOutcome::Added(added) = invoke(
        &manager,
        root.path(),
        PluginRequest::Add {
            source: selected_source,
            expected_digest: Some(inspection.digest().to_owned()),
            server: None,
            connection: None,
            credential: None,
            replace: false,
        },
    )
    .await
    .expect("install chosen skill") else {
        panic!("added");
    };
    assert_eq!(added.skills.accepted(), ["alpha"]);
    assert_eq!(
        added.installed.metadata().repository(),
        Some(format!("https://github.com/owner/repo/tree/{COMMIT}/packages/custom-dir").as_str())
    );
    assert_eq!(
        skills
            .summaries(&test_agent_id(1).to_string(), root.path())
            .expect("skills")
            .len(),
        1
    );
}

#[tokio::test]
async fn multi_skill_repo_rejects_ambiguous_skill_selection_with_paths() {
    let root = tempfile::tempdir().expect("fixture");
    let (mut manager, _skills) = manager(root.path());
    let beta_skill = "---\nname: beta\ndescription: Beta skill.\n---\nBeta body.\n";
    let archive = repo_archive(
        COMMIT,
        &[
            ("packages/dup-a/SKILL.md", beta_skill.as_bytes()),
            ("packages/dup-b/SKILL.md", beta_skill.as_bytes()),
        ],
    );
    let _fixture = start_fixture(
        &mut manager,
        FixtureRepo {
            default_branch: "main".to_owned(),
            refs: std::collections::BTreeMap::from([("main".to_owned(), COMMIT.to_owned())]),
            archives: std::collections::BTreeMap::from([(COMMIT.to_owned(), archive)]),
        },
    )
    .await;

    let dup_err = invoke(
        &manager,
        root.path(),
        PluginRequest::Inspect {
            source: PluginSource::Github {
                repository: "https://github.com/owner/repo".to_owned(),
                commit: Some(COMMIT.to_owned()),
                r#ref: None,
                path: None,
                skill: Some("beta".to_owned()),
            },
        },
    )
    .await
    .err()
    .expect("ambiguous skill selector must list matching paths");
    let dup_text = dup_err.to_string();
    assert!(
        dup_text.contains("\"path\":\"packages/dup-a\"")
            && dup_text.contains("\"path\":\"packages/dup-b\""),
        "{dup_text}"
    );
    assert!(dup_text.contains("\"action\":\"inspect\""), "{dup_text}");
}

#[tokio::test]
async fn skills_sh_link_installs_the_named_skill_after_inspect() {
    let root = tempfile::tempdir().expect("fixture");
    let (mut manager, skills) = manager(root.path());
    let alpha_md = "---\nname: alpha\ndescription: Alpha skill.\n---\nAlpha body.\n";
    let beta_md = "---\nname: beta\ndescription: Beta skill.\n---\nBeta body.\n";
    let archive = repo_archive(
        COMMIT,
        &[
            ("skills/alpha/SKILL.md", alpha_md.as_bytes()),
            ("skills/beta/SKILL.md", beta_md.as_bytes()),
        ],
    );
    let (seen, _fixture) = start_fixture(
        &mut manager,
        FixtureRepo {
            default_branch: "main".to_owned(),
            refs: std::collections::BTreeMap::from([("main".to_owned(), COMMIT.to_owned())]),
            archives: std::collections::BTreeMap::from([(COMMIT.to_owned(), archive)]),
        },
    )
    .await;

    let skills_sh_source = PluginSource::SkillsSh {
        url: "https://skills.sh/owner/repo/alpha".to_owned(),
    };
    for request in [
        PluginRequest::Add {
            source: skills_sh_source.clone(),
            expected_digest: Some("0".repeat(64)),
            server: None,
            connection: None,
            credential: None,
            replace: false,
        },
        PluginRequest::Install {
            source: skills_sh_source.clone(),
            expected_digest: "0".repeat(64),
        },
    ] {
        let err = invoke(&manager, root.path(), request)
            .await
            .err()
            .expect("direct skills_sh add/install must be rejected");
        assert!(err.to_string().contains("\"action\":\"inspect\""));
    }
    assert!(seen.lock().expect("lock").is_empty());

    let inspection = inspect_full(&manager, root.path(), skills_sh_source).await;
    let pinned_source = inspection.source().expect("pinned source").clone();
    assert_eq!(
        pinned_source,
        PluginSource::Github {
            repository: "https://github.com/owner/repo".to_owned(),
            commit: Some(COMMIT.to_owned()),
            r#ref: None,
            path: Some("skills/alpha".to_owned()),
            skill: None,
        }
    );

    let PluginOutcome::Added(added) = invoke(
        &manager,
        root.path(),
        PluginRequest::Add {
            source: pinned_source,
            expected_digest: Some(inspection.digest().to_owned()),
            server: None,
            connection: None,
            credential: None,
            replace: false,
        },
    )
    .await
    .expect("install skill resolved from skills.sh") else {
        panic!("added");
    };
    assert_eq!(added.skills.accepted(), ["alpha"]);
    assert_eq!(
        added.installed.metadata().repository(),
        Some(format!("https://github.com/owner/repo/tree/{COMMIT}/skills/alpha").as_str())
    );
    assert_eq!(
        skills
            .summaries(&test_agent_id(1).to_string(), root.path())
            .expect("skills")
            .len(),
        1
    );
}

#[tokio::test]
async fn skills_sh_site_link_is_refused_without_network_access() {
    let root = tempfile::tempdir().expect("fixture");
    let (mut manager, _skills) = manager(root.path());
    let (seen, _fixture) = start_fixture(
        &mut manager,
        FixtureRepo {
            default_branch: "main".to_owned(),
            refs: std::collections::BTreeMap::new(),
            archives: std::collections::BTreeMap::new(),
        },
    )
    .await;

    for url in [
        "https://skills.sh/site/docs",
        "https://skills.sh/site/guides/intro",
    ] {
        let err = invoke(
            &manager,
            root.path(),
            PluginRequest::Inspect {
                source: PluginSource::SkillsSh {
                    url: url.to_owned(),
                },
            },
        )
        .await
        .err()
        .expect("/site/ skills.sh URL must be refused");
        let text = err.to_string();
        assert!(text.contains("not hosted on GitHub"), "{text}");
        assert!(text.contains("\"action\":\"inspect\""), "{text}");
    }
    assert!(seen.lock().expect("lock").is_empty());
}

#[tokio::test]
async fn inspect_rejects_commit_with_ref_and_pinned_ops_reject_skill_or_missing_commit() {
    let root = tempfile::tempdir().expect("fixture");
    let (mut manager, _skills) = manager(root.path());
    let (seen, _fixture) = start_fixture(
        &mut manager,
        FixtureRepo {
            default_branch: "main".to_owned(),
            refs: std::collections::BTreeMap::new(),
            archives: std::collections::BTreeMap::new(),
        },
    )
    .await;

    let err = invoke(
        &manager,
        root.path(),
        PluginRequest::Inspect {
            source: github_source(Some(COMMIT), Some("main"), None, None),
        },
    )
    .await
    .err()
    .expect("commit + ref must fail");
    assert!(matches!(err, PluginError::Invalid(_)), "{err:?}");

    for source in [
        github_source(Some(COMMIT), None, None, Some("alpha")),
        github_source(None, Some("main"), None, None),
        github_source(Some(COMMIT), Some("main"), None, None),
    ] {
        for req in [
            PluginRequest::Add {
                source: source.clone(),
                expected_digest: Some("0".repeat(64)),
                server: None,
                connection: None,
                credential: None,
                replace: false,
            },
            PluginRequest::Install {
                source,
                expected_digest: "0".repeat(64),
            },
        ] {
            let err = invoke(&manager, root.path(), req)
                .await
                .err()
                .expect("pinned operation rejection");
            assert!(matches!(err, PluginError::Invalid(_)), "{err:?}");
        }
    }
    assert!(seen.lock().expect("lock").is_empty());
}

#[tokio::test]
async fn pathless_repo_with_symlink_outside_chosen_skill_inspects_and_installs() {
    let root = tempfile::tempdir().expect("fixture");
    let (mut manager, skills) = manager(root.path());
    let alpha = "---\nname: alpha\ndescription: Alpha skill.\n---\nAlpha body.\n";
    let other = "---\nname: other\ndescription: Other skill.\n---\nOther body.\n";
    let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    for (name, bytes) in [
        ("skills/alpha/SKILL.md", alpha.as_bytes()),
        ("skills/other/SKILL.md", other.as_bytes()),
    ] {
        let mut h = tar::Header::new_gnu();
        h.set_mode(0o644);
        h.set_size(bytes.len() as u64);
        h.set_cksum();
        builder
            .append_data(&mut h, format!("repo-{COMMIT}/{name}"), bytes)
            .expect("file");
    }
    for link_path in ["README-link", "skills/other/bad-link"] {
        let mut sym = tar::Header::new_gnu();
        sym.set_entry_type(tar::EntryType::Symlink);
        sym.set_size(0);
        sym.set_mode(0o777);
        sym.set_cksum();
        builder
            .append_link(
                &mut sym,
                format!("repo-{COMMIT}/{link_path}"),
                "../etc/passwd",
            )
            .expect("symlink");
    }
    let archive = builder.into_inner().expect("tar").finish().expect("gzip");
    let _fixture = start_main_fixture(&mut manager, archive).await;

    let other_err = invoke(
        &manager,
        root.path(),
        PluginRequest::Inspect {
            source: github_source(Some(COMMIT), None, None, Some("other")),
        },
    )
    .await
    .err()
    .expect("symlink inside chosen folder must fail");
    assert!(other_err.to_string().contains("symlink"), "{other_err}");

    let inspection = inspect_full(
        &manager,
        root.path(),
        github_source(Some(COMMIT), None, None, Some("alpha")),
    )
    .await;
    let pinned = inspection.source().expect("pinned source").clone();
    assert_eq!(
        pinned,
        github_source(Some(COMMIT), None, Some("skills/alpha"), None)
    );

    let PluginOutcome::Added(added) = invoke(
        &manager,
        root.path(),
        PluginRequest::Add {
            source: pinned,
            expected_digest: Some(inspection.digest().to_owned()),
            server: None,
            connection: None,
            credential: None,
            replace: false,
        },
    )
    .await
    .expect("install chosen skill ignoring outside symlinks") else {
        panic!("added");
    };
    assert_eq!(added.skills.accepted(), ["alpha"]);
    assert_eq!(
        skills
            .summaries(&test_agent_id(1).to_string(), root.path())
            .expect("skills")
            .len(),
        1
    );
}

#[tokio::test]
async fn discovery_filters_invalid_names_walks_past_root_mismatch_and_lists_on_zero_matches() {
    let root = tempfile::tempdir().expect("fixture");
    let (mut manager, _skills) = manager(root.path());
    let root_skill = "---\nname: root-only\ndescription: Root skill.\n---\nRoot.\n";
    let padded_skill = "---\nname: \"  spaced  \"\ndescription: Padded frontmatter.\n---\nBody.\n";
    let nested_skill = "---\nname: target-skill\ndescription: Nested skill.\n---\nNested.\n";
    let bad_dir_skill = "---\ndescription: Missing valid name.\n---\nBody.\n";
    let archive = repo_archive(
        COMMIT,
        &[
            ("SKILL.md", root_skill.as_bytes()),
            ("skills/folder-fallback/SKILL.md", padded_skill.as_bytes()),
            ("skills/nested/SKILL.md", nested_skill.as_bytes()),
            ("skills/Bad Dir!/SKILL.md", bad_dir_skill.as_bytes()),
        ],
    );
    let _fixture = start_main_fixture(&mut manager, archive).await;

    let zero_err = invoke(
        &manager,
        root.path(),
        PluginRequest::Inspect {
            source: github_source(Some(COMMIT), None, None, Some("missing")),
        },
    )
    .await
    .err()
    .expect("missing selector returns filtered candidate list");
    let text = zero_err.to_string();
    assert!(
        text.contains("\"name\":\"root-only\"") && !text.contains("\"path\":\"\""),
        "{text}"
    );
    assert!(text.contains("\"name\":\"folder-fallback\""), "{text}");
    assert!(text.contains("\"name\":\"target-skill\""), "{text}");
    assert!(
        !text.contains("spaced") && !text.contains("Bad Dir!"),
        "{text}"
    );

    let inspection = inspect_full(
        &manager,
        root.path(),
        github_source(Some(COMMIT), None, None, Some("target-skill")),
    )
    .await;
    assert_eq!(
        inspection.source(),
        Some(&github_source(
            Some(COMMIT),
            None,
            Some("skills/nested"),
            None
        ))
    );
}

#[tokio::test]
async fn discovery_with_skill_finds_match_past_512_entries_and_caps_error_length() {
    let root = tempfile::tempdir().expect("fixture");
    let (mut manager, _skills) = manager(root.path());
    let files: Vec<(String, Vec<u8>)> = (0..520)
        .map(|i| {
            let name = if i == 518 {
                "needle-skill".to_owned()
            } else {
                format!("skill-{i:04}")
            };
            let body = format!("---\nname: {name}\ndescription: Desc {i}.\n---\nBody {i}.\n");
            (format!("skills/{name}/SKILL.md"), body.into_bytes())
        })
        .collect();
    let refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(p, b)| (p.as_str(), b.as_slice()))
        .collect();
    let archive = repo_archive(COMMIT, &refs);
    let _fixture = start_main_fixture(&mut manager, archive).await;

    let inspection = inspect_full(
        &manager,
        root.path(),
        github_source(Some(COMMIT), None, None, Some("needle-skill")),
    )
    .await;
    assert_eq!(
        inspection.source(),
        Some(&github_source(
            Some(COMMIT),
            None,
            Some("skills/needle-skill"),
            None
        ))
    );

    let list_err = invoke(
        &manager,
        root.path(),
        PluginRequest::Inspect {
            source: github_source(Some(COMMIT), None, None, None),
        },
    )
    .await
    .err()
    .expect("many candidates error is capped");
    let err_text = list_err.to_string();
    assert!(
        err_text.len() <= 4_300,
        "unexpectedly huge error ({}): {err_text}",
        err_text.len()
    );
    assert!(err_text.contains("list truncated"), "{err_text}");
}
