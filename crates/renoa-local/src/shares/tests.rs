//! Shares against a real catalog and real files: who may read one, and that a
//! read serves the file as it is now, inside its workspace.

use std::{fs, path::PathBuf};

use renoa_kernel::AgentId;
use rusqlite::Connection;
use uuid::Uuid;

use super::{
    MAX_SHARE_BYTES, PART, ShareError, SharedPath, Shares, restore_schema_52, share, unshare,
};
use crate::host::catalog;

struct Fixture {
    _directory: tempfile::TempDir,
    database: PathBuf,
    workspace: PathBuf,
    agent: AgentId,
    shares: Shares,
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().expect("directory");
    let root = directory.path().canonicalize().expect("canonical root");
    let database = root.join(catalog::HOST_DATABASE);
    fs::create_dir_all(database.parent().expect("state directory")).expect("state directory");
    catalog::initialize(&database).expect("catalog");
    let agent = Uuid::new_v4();
    crate::test_agents::insert_agent(&database, &agent.to_string());
    let workspace = root.join("workspace");
    fs::create_dir(&workspace).expect("workspace");
    Fixture {
        shares: Shares {
            database: database.clone(),
        },
        _directory: directory,
        database,
        workspace,
        agent: AgentId::from_uuid(agent),
    }
}

impl Fixture {
    fn file(&self, path: &str) -> SharedPath {
        SharedPath {
            workspace: self.workspace.clone(),
            path: path.to_owned(),
        }
    }

    fn share(&self, principal: Option<Uuid>, path: &str) -> Uuid {
        share(&self.database, self.agent, principal, &self.file(path)).expect("share")
    }

    fn rows(&self) -> i64 {
        Connection::open(&self.database)
            .expect("catalog")
            .query_row("SELECT count(*) FROM host_shares", [], |row| row.get(0))
            .expect("count")
    }
}

/// Two whole parts and a short last one.
fn bytes() -> Vec<u8> {
    let part = usize::try_from(PART).expect("a part fits memory");
    (0..2 * part + 10)
        .map(|index| u8::try_from(index % 251).expect("a byte"))
        .collect()
}

#[test]
fn sharing_again_returns_the_same_share_and_each_person_gets_their_own() {
    let fixture = fixture();
    let (person, other) = (Uuid::new_v4(), Uuid::new_v4());
    let first = fixture.share(Some(person), "page.html");
    assert_eq!(fixture.share(Some(person), "page.html"), first);
    let theirs = fixture.share(Some(other), "page.html");
    let owners = fixture.share(None, "page.html");
    assert_eq!(fixture.share(None, "page.html"), owners);
    let another = fixture.share(Some(person), "other.html");
    let all = [first, theirs, owners, another];
    for (index, id) in all.iter().enumerate() {
        assert!(!all[index + 1..].contains(id), "{all:?}");
    }
    assert_eq!(fixture.rows(), 4);

    assert!(
        unshare(
            &fixture.database,
            fixture.agent,
            Some(person),
            &fixture.file("page.html")
        )
        .expect("unshare")
    );
    assert_eq!(fixture.rows(), 2, "only the two stopped shares are gone");
    assert_ne!(
        fixture.share(Some(person), "page.html"),
        first,
        "sharing after stopping makes a new link"
    );
}

/// A person stops what they can read, a share made with no person too; a
/// turn with no person stops only its own.
#[test]
fn a_person_stops_every_share_they_can_read() {
    let fixture = fixture();
    let (person, other) = (Uuid::new_v4(), Uuid::new_v4());
    let stop = |principal: Option<Uuid>| {
        unshare(
            &fixture.database,
            fixture.agent,
            principal,
            &fixture.file("page.html"),
        )
        .expect("unshare")
    };
    fixture.share(Some(person), "page.html");
    assert!(
        !stop(None),
        "a turn with no person cannot stop a person's share"
    );
    fixture.share(None, "page.html");
    fixture.share(Some(other), "page.html");
    assert!(stop(Some(person)));
    assert_eq!(
        fixture.rows(),
        1,
        "their share and the one with no person are gone"
    );
    assert!(!stop(Some(person)));
}

#[tokio::test]
async fn only_its_person_reads_a_share_and_anyone_signed_in_reads_one_with_no_person() {
    let fixture = fixture();
    fs::write(fixture.workspace.join("page.html"), "<p>hi</p>").expect("file");
    let (person, other) = (Uuid::new_v4(), Uuid::new_v4());
    let theirs = fixture.share(Some(person), "page.html");
    let owners = fixture.share(None, "page.html");
    assert_eq!(
        fixture
            .shares
            .describe(theirs, person)
            .await
            .expect("theirs")
            .size,
        9
    );
    assert!(matches!(
        fixture.shares.describe(theirs, other).await,
        Err(ShareError::NotFound)
    ));
    assert!(matches!(
        fixture.shares.part(theirs, other, 0).await,
        Err(ShareError::NotFound)
    ));
    for reader in [person, other] {
        fixture
            .shares
            .describe(owners, reader)
            .await
            .expect("a share with no person");
    }
    assert!(matches!(
        fixture.shares.describe(Uuid::new_v4(), person).await,
        Err(ShareError::NotFound)
    ));
}

#[tokio::test]
async fn a_read_serves_the_file_as_it_is_now_in_parts() {
    let fixture = fixture();
    let person = Uuid::new_v4();
    let contents = bytes();
    fs::create_dir(fixture.workspace.join("out")).expect("folder");
    fs::write(fixture.workspace.join("out/Photo.JPG"), &contents).expect("file");
    let id = fixture.share(Some(person), "out/Photo.JPG");

    let described = fixture.shares.describe(id, person).await.expect("describe");
    assert_eq!(described.name, "Photo.JPG");
    assert_eq!(described.media_type, "image/jpeg");
    assert_eq!(described.size, contents.len() as u64);
    assert_eq!(described.sha256, crate::workspace::hex_sha256(&contents));
    assert_eq!((described.part_bytes, described.parts), (PART, 3));
    let mut read = Vec::new();
    for index in 0..described.parts {
        read.extend(fixture.shares.part(id, person, index).await.expect("part"));
    }
    assert_eq!(read, contents);
    assert!(matches!(
        fixture.shares.part(id, person, 3).await,
        Err(ShareError::NoSuchPart { index: 3, parts: 3 })
    ));

    fs::write(fixture.workspace.join("out/Photo.JPG"), b"new").expect("rewrite");
    let described = fixture
        .shares
        .describe(id, person)
        .await
        .expect("describe again");
    assert_eq!((described.size, described.parts), (3, 1));
    assert_eq!(described.sha256, crate::workspace::hex_sha256(b"new"));
    assert_eq!(
        fixture.shares.part(id, person, 0).await.expect("part"),
        b"new"
    );

    fs::remove_file(fixture.workspace.join("out/Photo.JPG")).expect("remove");
    assert!(matches!(
        fixture.shares.describe(id, person).await,
        Err(ShareError::NotFound)
    ));
    assert!(matches!(
        fixture.shares.part(id, person, 0).await,
        Err(ShareError::NotFound)
    ));
}

#[tokio::test]
async fn a_share_never_serves_outside_its_workspace_or_a_folder() {
    let fixture = fixture();
    let person = Uuid::new_v4();
    let outside = fixture
        .workspace
        .parent()
        .expect("parent")
        .join("secret.txt");
    fs::write(&outside, "secret").expect("outside file");
    std::os::unix::fs::symlink(&outside, fixture.workspace.join("link.txt")).expect("link");
    fs::create_dir(fixture.workspace.join("folder")).expect("folder");
    for path in ["link.txt", "../secret.txt", "folder"] {
        let id = fixture.share(Some(person), path);
        assert!(
            matches!(
                fixture.shares.describe(id, person).await,
                Err(ShareError::NotFound)
            ),
            "{path}"
        );
        assert!(
            matches!(
                fixture.shares.part(id, person, 0).await,
                Err(ShareError::NotFound)
            ),
            "{path}"
        );
    }
}

/// A folder on the way that became a file leaves the share not found, not a
/// failure to retry.
#[tokio::test]
async fn a_share_whose_folder_became_a_file_is_not_found() {
    let fixture = fixture();
    let person = Uuid::new_v4();
    fs::create_dir(fixture.workspace.join("out")).expect("folder");
    fs::write(fixture.workspace.join("out/page.html"), "hi").expect("file");
    let id = fixture.share(Some(person), "out/page.html");
    fs::remove_dir_all(fixture.workspace.join("out")).expect("remove the folder");
    fs::write(fixture.workspace.join("out"), "now a file").expect("a file where it was");
    assert!(matches!(
        fixture.shares.describe(id, person).await,
        Err(ShareError::NotFound)
    ));
    assert!(matches!(
        fixture.shares.part(id, person, 0).await,
        Err(ShareError::NotFound)
    ));
}

#[tokio::test]
async fn a_file_over_the_limit_is_refused() {
    let fixture = fixture();
    let person = Uuid::new_v4();
    fs::File::create(fixture.workspace.join("big.bin"))
        .expect("file")
        .set_len(MAX_SHARE_BYTES + 1)
        .expect("size");
    let id = fixture.share(Some(person), "big.bin");
    let size = MAX_SHARE_BYTES + 1;
    assert!(matches!(
        fixture.shares.describe(id, person).await,
        Err(ShareError::TooLarge { size: found }) if found == size
    ));
    assert!(matches!(
        fixture.shares.part(id, person, 0).await,
        Err(ShareError::TooLarge { .. })
    ));
}

#[test]
fn schema_53_gives_a_schema_52_catalog_an_empty_share_table() {
    let fixture = fixture();
    let db = Connection::open(&fixture.database).expect("catalog");
    restore_schema_52(&db);
    db.execute_batch(
        "UPDATE host_metadata SET schema_version = 52 WHERE singleton = 1;
         PRAGMA user_version = 52;",
    )
    .expect("a schema 52 catalog");
    drop(db);
    catalog::initialize(&fixture.database).expect("upgrade schema 52");
    catalog::initialize(&fixture.database).expect("reopen the upgraded catalog");
    assert_eq!(fixture.rows(), 0);
    fixture.share(None, "page.html");
}
