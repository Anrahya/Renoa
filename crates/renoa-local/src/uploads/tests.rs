use std::{fs, path::PathBuf};

use renoa_agent::ContentBlock;
use renoa_home::RenoaHome;
use uuid::Uuid;

use super::{MAX_UPLOAD_BYTES, UPLOAD_PART_BYTES, UploadError, UploadStatus, Uploads};

const PART: usize = 512 * 1024;

struct Fixture {
    _directory: tempfile::TempDir,
    uploads: Uploads,
    root: PathBuf,
    workspace: PathBuf,
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().canonicalize().expect("canonical path");
    let home = RenoaHome::at(path.join("home")).expect("home");
    let workspace = path.join("workspace");
    fs::create_dir(&workspace).expect("workspace");
    Fixture {
        uploads: Uploads::open(&home),
        root: home.uploads(),
        workspace,
        _directory: directory,
    }
}

fn sha256(bytes: &[u8]) -> String {
    crate::workspace::hex_sha256(bytes)
}

/// Two whole parts and a short last one.
fn file() -> Vec<u8> {
    (0..2 * PART + 10)
        .map(|index| u8::try_from(index % 251).expect("a byte"))
        .collect()
}

fn part(bytes: &[u8], index: usize) -> Vec<u8> {
    bytes[index * PART..((index + 1) * PART).min(bytes.len())].to_vec()
}

fn status(size: usize, received: usize, complete: bool) -> UploadStatus {
    UploadStatus {
        size: size as u64,
        received: received as u64,
        complete,
    }
}

#[test]
fn the_part_size_fits_one_message_as_base64_and_doubling_it_does_not() {
    let message = 1024 * 1024;
    assert_eq!(UPLOAD_PART_BYTES, PART);
    assert!(PART.div_ceil(3) * 4 < message);
    assert!((2 * PART).div_ceil(3) * 4 > message);
}

#[tokio::test]
async fn a_start_is_idempotent_and_a_refused_one_stores_nothing() {
    let fixture = fixture();
    let (owner, other, id) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let hash = sha256(b"abc");
    for (size, sha256) in [
        (0, hash.clone()),
        (MAX_UPLOAD_BYTES + 1, hash.clone()),
        (3, hash.to_uppercase()),
        (3, hash[..63].to_owned()),
    ] {
        let refused = fixture.uploads.start(owner, id, size, sha256).await;
        assert!(
            matches!(refused, Err(UploadError::Invalid(_))),
            "{refused:?}"
        );
    }
    assert!(!fixture.root.join(id.to_string()).exists());

    let started = fixture.uploads.start(owner, id, 3, hash.clone()).await;
    assert_eq!(started.expect("started"), status(3, 0, false));
    let again = fixture.uploads.start(owner, id, 3, hash.clone()).await;
    assert_eq!(again.expect("started again"), status(3, 0, false));
    for (principal, size, sha256) in [
        (owner, 4, hash.clone()),
        (owner, 3, sha256(b"abd")),
        (other, 3, hash.clone()),
    ] {
        let taken = fixture.uploads.start(principal, id, size, sha256).await;
        assert!(matches!(taken, Err(UploadError::Conflict(_))), "{taken:?}");
    }
    let entries = fs::read_dir(&fixture.root)
        .expect("uploads")
        .map(|entry| entry.expect("entry").file_name())
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), 2, "the lock and one upload: {entries:?}");
}

#[tokio::test]
async fn parts_arrive_in_order_and_complete_the_upload_once_the_hash_matches() {
    let fixture = fixture();
    let (owner, id) = (Uuid::new_v4(), Uuid::new_v4());
    let bytes = file();
    let size = bytes.len();
    fixture
        .uploads
        .start(owner, id, size as u64, sha256(&bytes))
        .await
        .expect("start");
    let put = |index: u64, data: Vec<u8>| fixture.uploads.put_part(owner, id, index, data);

    let gap = put(1, part(&bytes, 1)).await;
    assert!(
        matches!(&gap, Err(UploadError::Conflict(message)) if message == "Send part 0 of this upload next."),
        "{gap:?}"
    );
    assert_eq!(
        put(0, part(&bytes, 0)).await.expect("first"),
        status(size, PART, false)
    );
    assert_eq!(
        put(0, part(&bytes, 0)).await.expect("repeated"),
        status(size, PART, false)
    );
    // A part sent again is written again, so one a crash tore is mended.
    let mut torn = part(&bytes, 0);
    torn[7] = 0;
    assert_eq!(put(0, torn).await.expect("torn"), status(size, PART, false));
    assert_eq!(
        put(0, part(&bytes, 0)).await.expect("mended"),
        status(size, PART, false)
    );
    let short = put(1, part(&bytes, 1)[..10].to_vec()).await;
    assert!(
        matches!(&short, Err(UploadError::Invalid(message)) if message == "Part 1 of this upload is 524288 bytes."),
        "{short:?}"
    );
    assert!(matches!(
        put(3, vec![0; 10]).await,
        Err(UploadError::Invalid(_))
    ));
    assert!(matches!(
        fixture
            .uploads
            .put_part(Uuid::new_v4(), id, 1, part(&bytes, 1))
            .await,
        Err(UploadError::NotFound)
    ));
    assert!(matches!(
        fixture
            .uploads
            .put_part(owner, Uuid::new_v4(), 0, part(&bytes, 0))
            .await,
        Err(UploadError::NotFound)
    ));

    assert_eq!(
        put(1, part(&bytes, 1)).await.expect("second"),
        status(size, 2 * PART, false)
    );
    assert_eq!(
        put(2, part(&bytes, 2)).await.expect("last"),
        status(size, size, true)
    );
    let directory = fixture.root.join(id.to_string());
    assert_eq!(fs::read(directory.join("file")).expect("file"), bytes);
    assert!(!directory.join("partial").exists());
    assert_eq!(
        put(2, part(&bytes, 2)).await.expect("last again"),
        status(size, size, true)
    );
    assert!(matches!(
        put(2, vec![0; 10]).await,
        Err(UploadError::Conflict(_))
    ));
    let restarted = fixture
        .uploads
        .start(owner, id, size as u64, sha256(&bytes))
        .await;
    assert_eq!(restarted.expect("restart"), status(size, size, true));
}

#[tokio::test]
async fn a_file_whose_hash_does_not_match_is_removed() {
    let fixture = fixture();
    let (owner, id) = (Uuid::new_v4(), Uuid::new_v4());
    fixture
        .uploads
        .start(owner, id, 3, sha256(b"abc"))
        .await
        .expect("start");
    let mismatch = fixture
        .uploads
        .put_part(owner, id, 0, b"abd".to_vec())
        .await;
    assert!(
        matches!(mismatch, Err(UploadError::Invalid(_))),
        "{mismatch:?}"
    );
    assert!(!fixture.root.join(id.to_string()).exists());
    let again = fixture.uploads.start(owner, id, 3, sha256(b"abc")).await;
    assert_eq!(again.expect("a new start"), status(3, 0, false));
}

#[tokio::test]
async fn a_part_a_crash_cut_short_is_sent_again() {
    let fixture = fixture();
    let (owner, id) = (Uuid::new_v4(), Uuid::new_v4());
    let bytes = file();
    let size = bytes.len();
    fixture
        .uploads
        .start(owner, id, size as u64, sha256(&bytes))
        .await
        .expect("start");
    fixture
        .uploads
        .put_part(owner, id, 0, part(&bytes, 0))
        .await
        .expect("first");
    // A crash cut the second part's write short.
    let partial = fixture.root.join(id.to_string()).join("partial");
    let mut torn = fs::read(&partial).expect("partial");
    torn.extend_from_slice(&[9; 100]);
    fs::write(&partial, torn).expect("torn tail");
    let restarted = fixture
        .uploads
        .start(owner, id, size as u64, sha256(&bytes))
        .await;
    assert_eq!(restarted.expect("status"), status(size, PART, false));
    for index in 1..3 {
        fixture
            .uploads
            .put_part(
                owner,
                id,
                index,
                part(&bytes, usize::try_from(index).expect("index")),
            )
            .await
            .expect("part");
    }
    let file = fixture.root.join(id.to_string()).join("file");
    assert_eq!(fs::read(file).expect("file"), bytes);
}

#[tokio::test]
async fn an_upload_a_crash_left_unchecked_or_half_removed_converges() {
    let fixture = fixture();
    let (owner, id) = (Uuid::new_v4(), Uuid::new_v4());
    let bytes = file();
    let size = bytes.len();
    let start = || {
        fixture
            .uploads
            .start(owner, id, size as u64, sha256(&bytes))
    };
    start().await.expect("start");
    // Every byte arrived, but a crash came before the hash was checked.
    let directory = fixture.root.join(id.to_string());
    fs::write(directory.join("partial"), &bytes).expect("whole partial");
    assert_eq!(
        start().await.expect("status"),
        status(size, 2 * PART, false)
    );
    assert_eq!(
        fixture
            .uploads
            .put_part(owner, id, 2, part(&bytes, 2))
            .await
            .expect("last again"),
        status(size, size, true)
    );

    // A crash while removing a mismatched upload left its directory without
    // its record.
    let other = Uuid::new_v4();
    let left = fixture.root.join(other.to_string());
    fs::create_dir_all(&left).expect("left directory");
    fs::write(left.join("partial"), b"ab").expect("left bytes");
    let again = fixture.uploads.start(owner, other, 3, sha256(b"abc")).await;
    assert_eq!(again.expect("a new start"), status(3, 0, false));
}

async fn uploaded(fixture: &Fixture, owner: Uuid, bytes: &[u8]) -> Uuid {
    let id = Uuid::new_v4();
    fixture
        .uploads
        .start(owner, id, bytes.len() as u64, sha256(bytes))
        .await
        .expect("start");
    fixture
        .uploads
        .put_part(owner, id, 0, bytes.to_vec())
        .await
        .expect("part");
    id
}

fn texts(blocks: &[ContentBlock]) -> Vec<String> {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.clone()),
            ContentBlock::Image { .. } => None,
        })
        .collect()
}

#[tokio::test]
async fn an_attached_image_is_copied_once_and_shown() {
    let fixture = fixture();
    let owner = Uuid::new_v4();
    let jpeg = [&[0xFF, 0xD8, 0xFF, 0xE0][..], &[7; 600]].concat();
    let id = uploaded(&fixture, owner, &jpeg).await;
    let attach = || {
        fixture
            .uploads
            .attach(owner, id, "fridge.jpg".into(), fixture.workspace.clone())
    };
    let blocks = attach().await.expect("attached");
    assert_eq!(
        texts(&blocks),
        ["Attached image, included here so you can see it: attachments/fridge.jpg (604 bytes)."]
    );
    assert!(matches!(
        &blocks[1],
        ContentBlock::Image { mime_type, .. } if mime_type == "image/jpeg"
    ));
    assert_eq!(
        fs::read(fixture.workspace.join("attachments/fridge.jpg")).expect("copy"),
        jpeg
    );
    assert_eq!(attach().await.expect("attached again"), blocks);
    assert_eq!(
        fs::read_dir(fixture.workspace.join("attachments"))
            .expect("folder")
            .count(),
        1,
        "one copy and nothing left aside"
    );
}

#[tokio::test]
async fn a_name_another_file_holds_gets_a_marked_one() {
    let fixture = fixture();
    let owner = Uuid::new_v4();
    let id = uploaded(&fixture, owner, b"%PDF-1.7 menu").await;
    fs::create_dir(fixture.workspace.join("attachments")).expect("folder");
    fs::write(fixture.workspace.join("attachments/menu.pdf"), b"older").expect("older file");
    let blocks = fixture
        .uploads
        .attach(owner, id, "menu.pdf".into(), fixture.workspace.clone())
        .await
        .expect("attached");
    let mark = &sha256(b"%PDF-1.7 menu")[..8];
    assert_eq!(
        texts(&blocks),
        [format!(
            "Attached file: attachments/menu ({mark}).pdf (13 bytes)."
        )]
    );
    assert_eq!(blocks.len(), 1, "a PDF is not shown as an image");
    assert_eq!(
        fs::read(fixture.workspace.join("attachments/menu.pdf")).expect("older file"),
        b"older"
    );
}

#[tokio::test]
async fn an_upload_this_principal_did_not_complete_is_a_note() {
    let fixture = fixture();
    let owner = Uuid::new_v4();
    let finished = uploaded(&fixture, owner, b"notes").await;
    let unfinished = Uuid::new_v4();
    fixture
        .uploads
        .start(owner, unfinished, 5, sha256(b"notes"))
        .await
        .expect("start");
    for (principal, id) in [
        (Uuid::new_v4(), finished),
        (owner, unfinished),
        (owner, Uuid::new_v4()),
    ] {
        let blocks = fixture
            .uploads
            .attach(principal, id, "notes.txt".into(), fixture.workspace.clone())
            .await
            .expect("a note");
        assert_eq!(
            texts(&blocks),
            ["The attached file \"notes.txt\" did not arrive, so it is not in your workspace."]
        );
    }
    assert!(!fixture.workspace.join("attachments").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn an_attachments_link_out_of_the_workspace_is_refused() {
    let fixture = fixture();
    let owner = Uuid::new_v4();
    let id = uploaded(&fixture, owner, b"notes").await;
    let outside = fixture.root.join("outside");
    fs::create_dir_all(&outside).expect("outside");
    std::os::unix::fs::symlink(&outside, fixture.workspace.join("attachments")).expect("link");
    let refused = fixture
        .uploads
        .attach(owner, id, "notes.txt".into(), fixture.workspace.clone())
        .await;
    assert!(matches!(refused, Err(UploadError::Io(_))), "{refused:?}");
    assert_eq!(fs::read_dir(outside).expect("outside").count(), 0);
}

#[tokio::test]
async fn an_image_too_large_to_show_is_still_copied() {
    let fixture = fixture();
    let owner = Uuid::new_v4();
    // Its base64 is just over the 5 MiB a model is shown.
    let size = 5 * 1024 * 1024 / 4 * 3 + 1;
    let jpeg = [&[0xFF, 0xD8, 0xFF, 0xE0][..], &vec![7; size - 4]].concat();
    let id = Uuid::new_v4();
    fixture
        .uploads
        .start(owner, id, size as u64, sha256(&jpeg))
        .await
        .expect("start");
    for (index, chunk) in (0..).zip(jpeg.chunks(PART)) {
        fixture
            .uploads
            .put_part(owner, id, index, chunk.to_vec())
            .await
            .expect("part");
    }
    let blocks = fixture
        .uploads
        .attach(owner, id, "big.jpg".into(), fixture.workspace.clone())
        .await
        .expect("attached");
    assert_eq!(
        texts(&blocks),
        ["Attached file: attachments/big.jpg (3.7 MB)."]
    );
    assert_eq!(blocks.len(), 1, "not shown");
    assert_eq!(
        fs::read(fixture.workspace.join("attachments/big.jpg")).expect("copy"),
        jpeg
    );
}
