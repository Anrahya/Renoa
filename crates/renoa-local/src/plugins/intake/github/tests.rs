use std::{fs::File, io::Read, path::PathBuf};

use super::*;
use flate2::{Compression, read::GzDecoder, write::GzEncoder};

const COMMIT: &str = "1234567890123456789012345678901234567890";

fn archive(files: &[(&str, &[u8])]) -> (TempDir, PathBuf) {
    let directory = scratch().expect("archive fixture");
    let path = directory.path().join("archive.tar.gz");
    let encoder = GzEncoder::new(
        File::create(&path).expect("create archive"),
        Compression::default(),
    );
    let mut archive = tar::Builder::new(encoder);
    for (name, bytes) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        archive
            .append_data(&mut header, *name, *bytes)
            .expect("append file");
    }
    archive
        .into_inner()
        .expect("finish tar")
        .finish()
        .expect("finish gzip");
    (directory, path)
}

#[test]
fn pinned_repository_and_selected_directory_are_enforced() {
    let (directory, path) = archive(&[
        (
            &format!("repo-{COMMIT}/plugins/selected/SKILL.md"),
            b"selected",
        ),
        (&format!("repo-{COMMIT}/plugins/other/SKILL.md"), b"other"),
    ]);
    let destination = directory.path().join("extracted");
    extract(
        &path,
        &destination,
        &format!("repo-{COMMIT}"),
        Some("plugins/selected"),
        &CancellationToken::new(),
    )
    .expect("extract only selected directory");
    assert_eq!(
        fs::read(destination.join("SKILL.md")).expect("selected skill"),
        b"selected"
    );
    assert_eq!(fs::read_dir(&destination).expect("source files").count(), 1);
    assert!(
        extract(
            &path,
            &directory.path().join("wrong"),
            &format!("repo-{}", "a".repeat(40)),
            None,
            &CancellationToken::new()
        )
        .is_err()
    );
}

#[test]
fn untrusted_source_locators_are_rejected_before_downloading() {
    for repository in [
        "http://github.com/owner/repo",
        "https://github.com/owner/repo.git",
        "https://user:secret@github.com/owner/repo",
        "https://github.com/owner/repo?token=secret",
        "https://example.com/owner/repo",
        "https://github.com/owner/repo/tree/main",
    ] {
        assert!(validate(repository, COMMIT, None).is_err(), "{repository}");
    }
    for commit in ["main", "v1", "1234"] {
        assert!(validate("https://github.com/owner/repo", commit, None).is_err());
    }
    for path in ["../outside", "a/../b", "/root", "a//b", ".", "a\\b"] {
        assert!(
            validate("https://github.com/owner/repo", COMMIT, Some(path)).is_err(),
            "{path}"
        );
    }
}

#[test]
fn symlinks_duplicates_missing_directories_and_corrupt_gzip_fail() {
    let (directory, path) = archive(&[
        (&format!("repo-{COMMIT}/SKILL.md"), b"one"),
        (&format!("repo-{COMMIT}/SKILL.md"), b"two"),
    ]);
    assert!(
        extract(
            &path,
            &directory.path().join("duplicate"),
            &format!("repo-{COMMIT}"),
            None,
            &CancellationToken::new()
        )
        .is_err()
    );
    let (directory, path) = archive(&[(&format!("repo-{COMMIT}/SKILL.md"), b"one")]);
    assert!(
        extract(
            &path,
            &directory.path().join("missing"),
            &format!("repo-{COMMIT}"),
            Some("missing"),
            &CancellationToken::new()
        )
        .is_err()
    );
    let mut bytes = fs::read(&path).expect("gzip bytes");
    let index = bytes.len() - 8;
    bytes[index] ^= 1;
    fs::write(&path, bytes).expect("corrupt checksum");
    assert!(
        extract(
            &path,
            &directory.path().join("corrupt"),
            &format!("repo-{COMMIT}"),
            None,
            &CancellationToken::new()
        )
        .is_err()
    );

    let directory = scratch().expect("symlink fixture");
    let path = directory.path().join("link.tar.gz");
    let mut archive = tar::Builder::new(GzEncoder::new(
        File::create(&path).expect("create archive"),
        Compression::default(),
    ));
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_size(0);
    header.set_mode(0o777);
    header.set_cksum();
    archive
        .append_link(&mut header, format!("repo-{COMMIT}/link"), "/outside")
        .expect("append symlink");
    archive
        .into_inner()
        .expect("finish tar")
        .finish()
        .expect("finish gzip");
    assert!(
        extract(
            &path,
            &directory.path().join("link"),
            &format!("repo-{COMMIT}"),
            None,
            &CancellationToken::new()
        )
        .is_err()
    );
}

#[tokio::test]
async fn a_download_yields_only_the_pinned_source_and_removes_its_archive() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (directory, path) = archive(&[(&format!("repo-{COMMIT}/SKILL.md"), b"one")]);
    let bytes = fs::read(path).expect("archive bytes");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listen");
    let endpoint = format!("http://{}/source", listener.local_addr().expect("address"));
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept");
        let mut request = [0_u8; 4096];
        assert!(stream.read(&mut request).await.expect("read request") > 0);
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
    });
    let source = download_from(&endpoint, "repo", COMMIT, None, CancellationToken::new())
        .await
        .expect("download pinned archive");
    server.await.expect("server");
    assert_eq!(
        fs::read(source.path().join("source/SKILL.md")).expect("downloaded skill"),
        b"one"
    );
    assert!(!source.path().join("source.tar.gz").exists());
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    assert!(matches!(
        download_from(&endpoint, "repo", COMMIT, None, cancellation).await,
        Err(PluginError::Cancelled)
    ));
    drop(directory);
}

#[tokio::test]
async fn an_interrupted_download_and_rejected_responses_remove_their_staging() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    for mode in ["cancel", "oversize", "redirect"] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listen");
        let endpoint = format!("http://{}/source", listener.local_addr().expect("address"));
        let (received, ready) = tokio::sync::oneshot::channel();
        let (release, held) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut request = [0; 4096];
            assert!(stream.read(&mut request).await.expect("request") > 0);
            let response = match mode {
                "cancel" => "HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nx".to_owned(),
                "oversize" => format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", MAX_ARCHIVE_BYTES + 1),
                _ => "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:9/never-follow\r\nContent-Length: 0\r\n\r\n".to_owned(),
            };
            stream
                .write_all(response.as_bytes())
                .await
                .expect("headers and partial body");
            received.send(()).expect("signal boundary");
            let _ = held.await;
        });
        let staging = scratch().expect("owned staging");
        let staging_path = staging.path().to_path_buf();
        let cancellation = CancellationToken::new();
        let running = tokio::spawn({
            let cancellation = cancellation.clone();
            async move { download_into(&endpoint, "repo", COMMIT, None, cancellation, staging).await }
        });
        ready.await.expect("download active");
        if mode == "cancel" {
            cancellation.cancel();
        }
        let result = tokio::time::timeout(Duration::from_secs(2), running)
            .await
            .expect("bounded completion")
            .expect("download task");
        match mode {
            "cancel" => assert!(matches!(result, Err(PluginError::Cancelled))),
            "oversize" => assert!(
                matches!(result,Err(PluginError::Invalid(message)) if message.contains("128 MiB"))
            ),
            _ => assert!(
                matches!(result,Err(PluginError::Unavailable(message)) if message.contains("HTTP 302"))
            ),
        }
        assert!(
            !staging_path.exists(),
            "failed intake leaves no scratch files"
        );
        let _ = release.send(());
        server.await.expect("server");
    }
}

#[test]
fn hidden_gnu_metadata_is_bounded_before_tar_allocates_it() {
    let directory = scratch().expect("fixture");
    let path = directory.path().join("metadata.tar.gz");
    let mut archive = tar::Builder::new(GzEncoder::new(
        File::create(&path).expect("file"),
        Compression::default(),
    ));
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::GNULongName);
    // Long enough to cross the 1024-byte bound inside the metadata entry, and
    // short enough for the valid extraction to fit macOS's 1024-byte PATH_MAX.
    let nested = std::iter::repeat_n("a".repeat(180), 3)
        .collect::<Vec<_>>()
        .join("/");
    let name = format!("repo-{COMMIT}/{nested}/SKILL.md");
    let mut metadata = name.as_bytes().to_vec();
    metadata.push(0);
    header.set_size(metadata.len() as u64);
    header.set_cksum();
    archive
        .append_data(&mut header, "././@LongLink", metadata.as_slice())
        .expect("hidden metadata");
    let mut header = tar::Header::new_gnu();
    header.set_mode(0o644);
    header.set_size(3);
    header.set_cksum();
    archive
        .append_data(&mut header, "short-name", b"one".as_slice())
        .expect("file");
    archive.into_inner().expect("tar").finish().expect("gzip");
    let destination = directory.path().join("source");
    assert!(matches!(
        extract_limited(
            &path,
            &destination,
            &format!("repo-{COMMIT}"),
            None,
            &CancellationToken::new(),
            1024,
            false
        ),
        Err(PluginError::Invalid(_))
    ));
    assert!(
        !destination.exists(),
        "metadata must fail before file publication"
    );
    extract(
        &path,
        &destination,
        &format!("repo-{COMMIT}"),
        None,
        &CancellationToken::new(),
    )
    .expect("fixture is otherwise valid");
    assert_eq!(
        fs::read(destination.join(nested).join("SKILL.md")).expect("valid file"),
        b"one"
    );
}

#[test]
fn cancellation_during_a_tar_file_read_is_non_retryable() {
    struct CancelAfter<R> {
        reader: R,
        cancellation: CancellationToken,
    }
    impl<R: Read> Read for CancelAfter<R> {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            let count = self.reader.read(bytes)?;
            self.cancellation.cancel();
            Ok(count)
        }
    }
    let (_directory, path) = archive(&[(&format!("repo-{COMMIT}/SKILL.md"), b"one")]);
    let token = CancellationToken::new();
    let decoder = GzDecoder::new(File::open(path).expect("archive"));
    let mut archive = tar::Archive::new(BoundedReader {
        reader: decoder,
        remaining: 4096,
        cancellation: token.clone(),
    });
    let mut entries = archive.entries().expect("entries");
    let mut entry = entries.next().expect("entry").expect("header");
    let mut reader = BoundedReader {
        reader: CancelAfter {
            reader: &mut entry,
            cancellation: token.clone(),
        },
        remaining: 4096,
        cancellation: token,
    };
    let mut output = Vec::new();
    assert_eq!(
        std::io::copy(&mut reader, &mut output)
            .expect_err("copy must stop without retrying Interrupted")
            .kind(),
        std::io::ErrorKind::Other
    );
    assert_eq!(output, b"one");
}

#[test]
fn tar_metadata_cannot_read_past_the_decompressed_boundary() {
    let cancellation = CancellationToken::new();
    let mut reader = BoundedReader {
        reader: std::io::Cursor::new(vec![0; 1024]),
        remaining: 16,
        cancellation: cancellation.clone(),
    };
    let mut output = [0; 32];
    assert_eq!(reader.read(&mut output).expect("bounded read"), 16);
    assert_eq!(
        reader
            .read(&mut output)
            .expect_err("no byte past boundary")
            .kind(),
        std::io::ErrorKind::InvalidData
    );
    let mut reader = BoundedReader {
        reader: std::io::Cursor::new(vec![0; 1024]),
        remaining: 1024,
        cancellation: cancellation.clone(),
    };
    cancellation.cancel();
    assert_eq!(
        reader.read(&mut output).expect_err("cancelled read").kind(),
        std::io::ErrorKind::Other
    );
}

#[test]
fn validate_ref_rejects_malformed_refs_and_accepts_normal_branches() {
    for valid in ["main", "release/v1", "v1.0.0", "feature_1-test"] {
        assert!(resolve::validate_ref(valid).is_ok(), "expected {valid} ok");
    }
    let too_long = "a".repeat(257);
    for bad in [
        "",
        ".",
        "/main",
        "main/",
        "a//b",
        "a..b",
        "main branch",
        "main\n",
        "main?a=1",
        "main#frag",
        "a\\b",
        "a:b",
        too_long.as_str(),
    ] {
        assert!(
            matches!(resolve::validate_ref(bad), Err(PluginError::Invalid(_))),
            "expected {bad:?} rejected"
        );
    }
}

#[test]
fn skills_sh_odd_urls_and_selectors_are_rejected() {
    use crate::plugins::intake::skills_sh;
    let parsed =
        skills_sh::parse("https://skills.sh/owner/repo/my-skill").expect("valid skills.sh url");
    assert_eq!(parsed.repository, "https://github.com/owner/repo");
    assert_eq!(parsed.skill, "my-skill");

    for odd in [
        "http://skills.sh/owner/repo/skill",
        "https://example.com/owner/repo/skill",
        "https://skills.sh/owner/repo/skill/",
        "https://skills.sh/owner/repo/skill?ref=main",
        "https://skills.sh/owner/repo/skill#top",
        "https://skills.sh/owner/repo",
        "https://skills.sh/owner/repo/",
        "https://skills.sh/owner/repo/a/b",
        "https://user@skills.sh/owner/repo/skill",
        "https://skills.sh:8443/owner/repo/skill",
        "https://skills.sh/site/docs",
        "https://skills.sh/owner/repo.git/skill",
        "https://skills.sh/owner/repo/.",
        "https://skills.sh/owner/repo/..",
        "https://skills.sh/owner/repo/bad\\skill",
    ] {
        assert!(
            matches!(skills_sh::parse(odd), Err(PluginError::Invalid(_))),
            "expected {odd:?} rejected"
        );
    }
}

#[tokio::test]
async fn resolve_commit_handles_http_errors_bad_sha_and_oversized_responses() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let cases: &[(&str, Vec<u8>)] = &[
        (
            "301 Moved Permanently\r\nLocation: https://api.github.com/repos/new/repo/commits/HEAD",
            b"{}".to_vec(),
        ),
        ("403 Forbidden", b"{}".to_vec()),
        ("429 Too Many Requests", b"{}".to_vec()),
        ("404 Not Found", b"{}".to_vec()),
        ("200 OK", br#"{"sha":"not-a-sha"}"#.to_vec()),
        ("200 OK", vec![b' '; 1_048_577]),
    ];
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let origin = format!("http://{}", listener.local_addr().expect("addr"));
    let expected_cases = cases
        .iter()
        .map(|(status, body)| ((*status).to_owned(), body.clone()))
        .collect::<Vec<_>>();
    let server = tokio::spawn(async move {
        for (status, body) in expected_cases {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut req = [0_u8; 1024];
            let _ = stream.read(&mut req).await.expect("read");
            let headers = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(headers.as_bytes()).await.expect("hdrs");
            let _ = stream.write_all(&body).await;
        }
    });

    let err_301 = resolve::resolve_commit(&origin, "owner", "repo", None, CancellationToken::new())
        .await
        .expect_err("301");
    assert!(matches!(err_301, PluginError::Invalid(_)), "{err_301:?}");
    assert!(
        err_301.to_string().contains("moved or redirected"),
        "{err_301}"
    );

    for code in ["403", "429"] {
        let err = resolve::resolve_commit(&origin, "owner", "repo", None, CancellationToken::new())
            .await
            .expect_err(code);
        assert!(matches!(err, PluginError::Unavailable(_)), "{err:?}");
        let text = err.to_string();
        assert!(
            text.contains("rate limit") && text.contains("\"commit\""),
            "{text}"
        );
    }

    for kind in ["404", "bad sha", "oversized"] {
        let err = resolve::resolve_commit(&origin, "owner", "repo", None, CancellationToken::new())
            .await
            .expect_err(kind);
        assert!(matches!(err, PluginError::Invalid(_)), "{kind}: {err:?}");
    }
    server.await.expect("server");
}

#[test]
fn archive_limits_advise_path_and_discovery_policy_records_skipped_symlinks() {
    use std::io::Write as _;

    let dest = scratch().expect("dest");
    let big_dir = scratch().expect("big tar");
    let big_archive = big_dir.path().join("big.tar.gz");
    let mut gz = GzEncoder::new(
        File::create(&big_archive).expect("create"),
        Compression::default(),
    );
    let mut big_hdr = tar::Header::new_gnu();
    big_hdr
        .set_path(format!("repo-{COMMIT}/big.bin"))
        .expect("path");
    big_hdr.set_mode(0o644);
    big_hdr.set_size(33 * 1_024 * 1_024);
    big_hdr.set_cksum();
    gz.write_all(big_hdr.as_bytes()).expect("hdr");
    gz.write_all(&[0_u8; 1024]).expect("eof");
    gz.finish().expect("finish");

    let err = archive::extract_limited(
        &big_archive,
        dest.path(),
        &format!("repo-{COMMIT}"),
        None,
        &CancellationToken::new(),
        64 * 1_024 * 1_024,
        false,
    )
    .expect_err("over byte limit");
    assert!(err.to_string().contains("\"path\""), "{err}");

    let symlink_dir = scratch().expect("symlink tar");
    let symlink_path = symlink_dir.path().join("sym.tar.gz");
    let encoder = GzEncoder::new(
        File::create(&symlink_path).expect("create"),
        Compression::default(),
    );
    let mut builder = tar::Builder::new(encoder);
    let mut file_hdr = tar::Header::new_gnu();
    let skill_bytes = b"---\nname: ok\ndescription: OK.\n---\nBody.\n";
    file_hdr.set_mode(0o644);
    file_hdr.set_size(skill_bytes.len() as u64);
    file_hdr.set_cksum();
    builder
        .append_data(
            &mut file_hdr,
            format!("repo-{COMMIT}/skills/ok/SKILL.md"),
            &skill_bytes[..],
        )
        .expect("append skill");
    let mut sym = tar::Header::new_gnu();
    sym.set_entry_type(tar::EntryType::Symlink);
    sym.set_size(0);
    sym.set_mode(0o777);
    sym.set_cksum();
    builder
        .append_link(&mut sym, format!("repo-{COMMIT}/other/link"), "../target")
        .expect("append symlink");
    builder.into_inner().expect("tar").finish().expect("gzip");

    let strict_dest = scratch().expect("strict");
    assert!(matches!(
        archive::extract_limited(
            &symlink_path,
            strict_dest.path(),
            &format!("repo-{COMMIT}"),
            None,
            &CancellationToken::new(),
            1024 * 1024,
            false,
        ),
        Err(PluginError::Invalid(_))
    ));
    let discovery_dest = scratch().expect("discovery");
    let skipped = archive::extract_limited(
        &symlink_path,
        discovery_dest.path(),
        &format!("repo-{COMMIT}"),
        None,
        &CancellationToken::new(),
        1024 * 1024,
        true,
    )
    .expect("tolerate mode skips outside links");
    assert_eq!(skipped, vec![PathBuf::from("other/link")]);
}
