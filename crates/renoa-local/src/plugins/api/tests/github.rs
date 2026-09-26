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
        commit: COMMIT.to_owned(),
        path: Some(selected.to_owned()),
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
