use std::path::Path;

use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::TcpListener,
};

use super::{Release, Version, beside, newer, version};

fn release(version_name: &str) -> Release {
    Release {
        version_name: version_name.to_owned(),
        zip: format!("renoa-mac-{version_name}.zip"),
        sha256: "a".repeat(64),
        size: 10,
    }
}

#[test]
fn a_version_is_three_short_numbers() {
    assert_eq!(version("0.23.1"), Some(Version(0, 23, 1)));
    for refused in [
        "0.23", "0.23.1.4", "0.2x.1", "1000.0.0", "0..1", "+1.0.0", "0.²3.1", "",
    ] {
        assert_eq!(version(refused), None, "{refused}");
    }
    assert!(
        version("0.10.0") > version("0.9.9"),
        "compared as numbers, not text"
    );
}

#[test]
fn only_a_newer_well_formed_release_is_offered() {
    assert_eq!(
        release("0.23.0").newer_than("0.22.0"),
        Ok(Some(release("0.23.0")))
    );
    assert_eq!(release("0.22.0").newer_than("0.22.0"), Ok(None));
    assert_eq!(release("0.21.9").newer_than("0.22.0"), Ok(None));
    let refused = [
        Release {
            zip: "../Renoa.zip".to_owned(),
            ..release("0.23.0")
        },
        Release {
            zip: "renoa-mac-0.24.0.zip".to_owned(),
            ..release("0.23.0")
        },
        Release {
            sha256: "A".repeat(64),
            ..release("0.23.0")
        },
        Release {
            sha256: "a".repeat(63),
            ..release("0.23.0")
        },
        Release {
            size: 0,
            ..release("0.23.0")
        },
        Release {
            size: super::MAX_APP_BYTES + 1,
            ..release("0.23.0")
        },
        release("0.23"),
    ];
    for offered in refused {
        assert!(offered.clone().newer_than("0.22.0").is_err(), "{offered:?}");
    }
}

#[test]
fn the_app_downloads_from_beside_its_description() {
    assert_eq!(
        beside("https://renoa.live/app/mac.json", "renoa-mac-0.23.0.zip"),
        "https://renoa.live/app/renoa-mac-0.23.0.zip"
    );
}

/// Serves one response per connection, in order, and returns its origin.
async fn serve(responses: Vec<(u16, Vec<u8>)>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        for (status, body) in responses {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).await.unwrap();
            let head = format!(
                "HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(head.as_bytes()).await.unwrap();
            stream.write_all(&body).await.unwrap();
        }
    });
    origin
}

#[tokio::test]
async fn the_check_reads_the_served_description() {
    let described = serde_json::to_vec(&serde_json::json!({
        "version_name": "0.23.0", "zip": "renoa-mac-0.23.0.zip", "sha256": "a".repeat(64), "size": 10,
    }))
    .unwrap();
    let origin = serve(vec![
        (200, described),
        (404, Vec::new()),
        (200, vec![b' '; 5000]),
    ])
    .await;
    let client = reqwest::Client::new();
    let url = super::description_url(&origin);
    assert_eq!(
        newer(&client, &url, "0.22.0").await,
        Ok(Some(release("0.23.0")))
    );
    assert_eq!(
        newer(&client, &url, "0.22.0").await,
        Ok(None),
        "no Mac app served yet"
    );
    assert!(
        newer(&client, &url, "0.22.0").await.is_err(),
        "a description too long to be one"
    );
}

/// A Renoa.app with only what `check_bundle` reads, signed ad hoc.
fn unsigned_bundle(at: &Path, version: &str) {
    std::fs::create_dir_all(at.join("Contents/MacOS")).unwrap();
    std::fs::copy("/usr/bin/true", at.join("Contents/MacOS/renoa-mac")).unwrap();
    std::fs::write(
        at.join("Contents/Info.plist"),
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>live.renoa.mac</string>
<key>CFBundleExecutable</key><string>renoa-mac</string>
<key>CFBundleShortVersionString</key><string>{version}</string>
</dict></plist>"#
        ),
    )
    .unwrap();
}

#[cfg(target_os = "macos")]
fn sign_ad_hoc(bundle: &Path) {
    let signed = std::process::Command::new("/usr/bin/codesign")
        .args(["--force", "--sign", "-", "--identifier", "live.renoa.mac"])
        .arg(bundle)
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(signed.success());
}

#[cfg(target_os = "macos")]
#[test]
fn an_update_must_be_renoa_at_its_version_signed_by_the_running_apps_key() {
    let scratch = std::env::temp_dir().join(format!("renoa-update-test-{}", uuid::Uuid::new_v4()));
    let (running, staged) = (
        scratch.join("running/Renoa.app"),
        scratch.join("staged/Renoa.app"),
    );
    unsigned_bundle(&running, "0.22.0");
    unsigned_bundle(&staged, "0.23.0");
    assert!(
        super::check_bundle(&staged, "0.23.0", &running).is_err(),
        "an unsigned running app can't vouch for an update"
    );
    sign_ad_hoc(&running);
    sign_ad_hoc(&staged);
    // Ad hoc signatures pin the code's own hash, so another build never
    // satisfies the running app's requirement.
    assert!(super::check_bundle(&staged, "0.23.0", &running).is_err());
    assert_eq!(super::check_bundle(&running, "0.22.0", &running), Ok(()));
    assert!(
        super::check_bundle(&running, "0.23.0", &running).is_err(),
        "named another version"
    );
    std::fs::remove_dir_all(&scratch).unwrap();
}

/// A signed Renoa.app at `<scratch>/Applications`, its release zipped and
/// served, and a marker only the running copy has.
#[cfg(target_os = "macos")]
async fn installed_and_served(
    scratch: &Path,
    linked: bool,
    served: impl FnOnce(Vec<u8>) -> Vec<u8>,
) -> (std::path::PathBuf, String, Release) {
    let bundle = scratch.join("Applications/Renoa.app");
    unsigned_bundle(&bundle, "0.23.0");
    sign_ad_hoc(&bundle);
    let zip = scratch.join("renoa-mac-0.23.0.zip");
    let zipped = std::process::Command::new("/usr/bin/ditto")
        .args(["-c", "-k", "--keepParent"])
        .arg(&bundle)
        .arg(&zip)
        .status()
        .unwrap();
    assert!(zipped.success());
    if linked {
        // The same signed app elsewhere on disk, and a zip whose Renoa.app
        // only links to it.
        let elsewhere = scratch.join("elsewhere/Renoa.app");
        let copied = std::process::Command::new("/usr/bin/ditto")
            .arg(&bundle)
            .arg(&elsewhere)
            .status()
            .unwrap();
        assert!(copied.success());
        let pack = scratch.join("pack");
        std::fs::create_dir(&pack).unwrap();
        std::os::unix::fs::symlink(&elsewhere, pack.join("Renoa.app")).unwrap();
        std::fs::remove_file(&zip).unwrap();
        let zipped = std::process::Command::new("/usr/bin/zip")
            .args(["-q", "-y"])
            .arg(&zip)
            .arg("Renoa.app")
            .current_dir(&pack)
            .status()
            .unwrap();
        assert!(zipped.success());
    }
    std::fs::write(bundle.join("running-copy"), b"").unwrap();
    let bytes = std::fs::read(&zip).unwrap();
    let release = Release {
        version_name: "0.23.0".to_owned(),
        zip: "renoa-mac-0.23.0.zip".to_owned(),
        sha256: super::hex(ring::digest::digest(&ring::digest::SHA256, &bytes).as_ref()),
        size: bytes.len() as u64,
    };
    let origin = serve(vec![(200, served(bytes))]).await;
    (bundle, super::description_url(&origin), release)
}

fn leftovers(scratch: &Path) -> Vec<String> {
    std::fs::read_dir(scratch.join("Applications"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn an_update_replaces_the_running_app_and_leaves_nothing_beside_it() {
    let scratch = std::env::temp_dir().join(format!("renoa-update-test-{}", uuid::Uuid::new_v4()));
    let (bundle, url, release) = installed_and_served(&scratch, false, |bytes| bytes).await;

    super::install(&reqwest::Client::new(), &url, &release, &bundle)
        .await
        .unwrap();

    assert!(
        !bundle.join("running-copy").exists(),
        "the served app took the running one's place"
    );
    assert!(bundle.join("Contents/MacOS/renoa-mac").is_file());
    assert_eq!(leftovers(&scratch), ["Renoa.app"]);
    std::fs::remove_dir_all(&scratch).unwrap();
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn an_update_that_does_not_match_its_description_changes_nothing() {
    let scratch = std::env::temp_dir().join(format!("renoa-update-test-{}", uuid::Uuid::new_v4()));
    let (bundle, url, release) = installed_and_served(&scratch, false, |mut bytes| {
        bytes[0] ^= 1;
        bytes
    })
    .await;

    let refused = super::install(&reqwest::Client::new(), &url, &release, &bundle).await;

    assert_eq!(
        refused,
        Err("the update does not match its description".to_owned())
    );
    assert!(
        bundle.join("running-copy").exists(),
        "the running app is untouched"
    );
    assert_eq!(leftovers(&scratch), ["Renoa.app"]);
    std::fs::remove_dir_all(&scratch).unwrap();
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn an_update_whose_app_is_a_link_is_refused() {
    let scratch = std::env::temp_dir().join(format!("renoa-update-test-{}", uuid::Uuid::new_v4()));
    let (bundle, url, release) = installed_and_served(&scratch, true, |bytes| bytes).await;

    let refused = super::install(&reqwest::Client::new(), &url, &release, &bundle).await;

    assert_eq!(refused, Err("the update has no Renoa.app".to_owned()));
    assert!(
        bundle.join("running-copy").exists(),
        "the running app is untouched"
    );
    assert_eq!(leftovers(&scratch), ["Renoa.app"]);
    std::fs::remove_dir_all(&scratch).unwrap();
}

#[tokio::test]
async fn a_renoa_live_that_stops_answering_frees_the_updater() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    // Accepts the connection and never answers.
    let held = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        std::future::pending::<()>().await;
        drop(stream);
    });
    let client = super::client_waiting(std::time::Duration::from_millis(200));

    let checked = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        newer(&client, &super::description_url(&origin), "0.22.0"),
    )
    .await;

    assert!(matches!(checked, Ok(Err(_))), "{checked:?}");
    held.abort();
}

#[test]
fn an_update_cut_short_is_cleared_by_the_next_launch() {
    let scratch = std::env::temp_dir().join(format!("renoa-update-test-{}", uuid::Uuid::new_v4()));
    let bundle = scratch.join("Applications/Renoa.app");
    unsigned_bundle(&bundle, "0.23.0");
    let left = super::staging(&bundle).unwrap();
    std::fs::create_dir_all(left.join("unpacked")).unwrap();

    super::clear_leftovers(&bundle);

    assert_eq!(leftovers(&scratch), ["Renoa.app"]);
    std::fs::remove_dir_all(&scratch).unwrap();
}
