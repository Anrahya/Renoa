use super::*;
use flate2::{Compression, write::GzEncoder};

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
            1024
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
