//! Static site publishing, immutable snapshot isolation, atomic replacement,
//! listing, unpublishing, schema 55 migration, and Host reset tests.

use std::{fs, path::PathBuf};

use renoa_home::RenoaHome;
use renoa_kernel::AgentId;
use rusqlite::Connection;
use uuid::Uuid;

use super::{
    MAX_SITE_ENTRIES, MAX_SITE_TOTAL_BYTES, SiteEntry, SiteError, Sites, list_reachable, publish,
    restore_schema_54, sites_dir_for_database, unpublish, validate_site_name,
};
use crate::{host::catalog, host::reset_host_data_root};

struct Fixture {
    _directory: tempfile::TempDir,
    root: PathBuf,
    database: PathBuf,
    sites_dir: PathBuf,
    workspace: PathBuf,
    agent: AgentId,
    sites: Sites,
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().expect("directory");
    let root = directory.path().canonicalize().expect("canonical root");
    let home = RenoaHome::at(root.clone()).expect("home");
    home.initialize().expect("initialize home");
    let database = home.host_database();
    catalog::initialize(&database).expect("catalog");
    let agent = Uuid::new_v4();
    crate::test_agents::insert_agent(&database, &agent.to_string());
    let workspace = root.join("workspace");
    fs::create_dir(&workspace).expect("workspace");
    let sites = Sites::open(&home);
    let sites_dir = home.sites();
    Fixture {
        _directory: directory,
        root,
        database,
        sites_dir,
        workspace,
        agent: AgentId::from_uuid(agent),
        sites,
    }
}

impl Fixture {
    fn rows(&self) -> i64 {
        Connection::open(&self.database)
            .expect("catalog")
            .query_row("SELECT count(*) FROM host_sites", [], |row| row.get(0))
            .expect("count")
    }

    fn snapshot_dirs(&self) -> Vec<String> {
        if !self.sites_dir.exists() {
            return Vec::new();
        }
        let mut names = fs::read_dir(&self.sites_dir)
            .expect("read sites_dir")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();
        names.sort();
        names
    }
}

/// The bytes the gateway would serve at `rel` in site `name`.
async fn read_whole_file(sites: &Sites, name: &str, rel: &str) -> Vec<u8> {
    match sites.entry(name, rel).await.expect("site entry") {
        SiteEntry::File(path) => fs::read(path).expect("read snapshot file"),
        SiteEntry::Listing(html) => html.into_bytes(),
    }
}

fn not_found(result: &Result<SiteEntry, SiteError>) -> bool {
    matches!(result, Err(SiteError::NotFound))
}

#[tokio::test]
async fn publish_creates_immutable_snapshot_isolated_from_later_workspace_edits() {
    let fixture = fixture();
    let owner = Uuid::new_v4();
    let dist = fixture.workspace.join("dist");
    fs::create_dir_all(dist.join("assets")).expect("assets");
    fs::write(
        dist.join("index.html"),
        "<h1>v1</h1><script src=\"assets/app.js\"></script>",
    )
    .expect("index");
    let big_js: Vec<u8> = (0..(2 * 512 * 1024 + 17))
        .map(|i| b'a' + u8::try_from(i % 26).unwrap())
        .collect();
    fs::write(dist.join("assets/app.js"), &big_js).expect("js");

    let published = publish(
        &fixture.database,
        fixture.agent,
        Some(owner),
        &fixture.workspace,
        "demo-app",
        "dist",
    )
    .expect("publish");
    assert_eq!(published.path, "/sites/demo-app/");
    assert_eq!(published.file_count, 2);
    assert_eq!(published.source_folder, "dist");

    assert_eq!(
        read_whole_file(&fixture.sites, "demo-app", "assets/app.js").await,
        big_js
    );

    // Mutate workspace completely after publish; the live snapshot must stay identical.
    fs::write(dist.join("index.html"), "<h1>mutated</h1>").expect("mutate");
    fs::remove_file(dist.join("assets/app.js")).expect("delete");
    fs::write(dist.join("extra.html"), "not published").expect("add");

    assert_eq!(
        read_whole_file(&fixture.sites, "demo-app", "").await,
        b"<h1>v1</h1><script src=\"assets/app.js\"></script>"
    );
    assert_eq!(
        read_whole_file(&fixture.sites, "demo-app", "assets/app.js").await,
        big_js
    );
    assert!(not_found(
        &fixture.sites.entry("demo-app", "extra.html").await
    ));
}

#[tokio::test]
async fn a_folder_path_ending_in_a_slash_serves_its_index_and_a_bare_folder_is_not_found() {
    let fixture = fixture();
    let owner = Uuid::new_v4();
    let dist = fixture.workspace.join("dist");
    fs::create_dir_all(dist.join("docs")).expect("docs");
    fs::write(dist.join("index.html"), "root").expect("root");
    fs::write(dist.join("docs/index.html"), "nested").expect("nested");

    publish(
        &fixture.database,
        fixture.agent,
        Some(owner),
        &fixture.workspace,
        "docs-site",
        "dist",
    )
    .expect("publish");

    assert!(not_found(&fixture.sites.entry("docs-site", "docs").await));
    assert!(not_found(
        &fixture.sites.entry("docs-site", "docs/index.html/").await
    ));
    for nested in ["docs/", "docs/index.html"] {
        assert_eq!(
            read_whole_file(&fixture.sites, "docs-site", nested).await,
            b"nested"
        );
    }
    for hostile in [
        "docs//index.html",
        "./index.html",
        "docs/../index.html",
        "/index.html",
    ] {
        assert!(
            not_found(&fixture.sites.entry("docs-site", hostile).await),
            "{hostile}"
        );
    }
}

#[tokio::test]
async fn republishing_replaces_site_atomically_and_removes_old_snapshot() {
    let fixture = fixture();
    let owner = Uuid::new_v4();
    let v1 = fixture.workspace.join("v1");
    let v2 = fixture.workspace.join("v2");
    fs::create_dir(&v1).expect("v1");
    fs::create_dir(&v2).expect("v2");
    fs::write(v1.join("index.html"), "one").expect("index1");
    fs::write(v1.join("old.txt"), "stale").expect("old");
    fs::write(v2.join("index.html"), "two").expect("index2");
    fs::write(v2.join("new.css"), "body{}").expect("css");

    publish(
        &fixture.database,
        fixture.agent,
        Some(owner),
        &fixture.workspace,
        "my-site",
        "v1",
    )
    .expect("publish v1");
    let first_dirs = fixture.snapshot_dirs();
    assert_eq!(first_dirs.len(), 1);

    publish(
        &fixture.database,
        fixture.agent,
        Some(owner),
        &fixture.workspace,
        "my-site",
        "v2",
    )
    .expect("publish v2");
    let second_dirs = fixture.snapshot_dirs();
    assert_eq!(second_dirs.len(), 1);
    assert_ne!(second_dirs, first_dirs);
    assert_eq!(fixture.rows(), 1);

    assert_eq!(read_whole_file(&fixture.sites, "my-site", "").await, b"two");
    assert_eq!(
        read_whole_file(&fixture.sites, "my-site", "new.css").await,
        b"body{}"
    );
    assert!(not_found(&fixture.sites.entry("my-site", "old.txt").await));
}

#[tokio::test]
async fn folder_without_index_html_serves_escaped_directory_listing() {
    let fixture = fixture();
    let owner = Uuid::new_v4();
    let dir = fixture.workspace.join("gallery");
    fs::create_dir(&dir).expect("gallery");
    fs::write(dir.join("a<b&c>.txt"), "hi").expect("file");

    publish(
        &fixture.database,
        fixture.agent,
        Some(owner),
        &fixture.workspace,
        "gallery",
        "gallery",
    )
    .expect("publish");
    let Ok(SiteEntry::Listing(html)) = fixture.sites.entry("gallery", "").await else {
        panic!("a site without index.html lists its files");
    };
    assert!(html.contains("a&lt;b&amp;c&gt;.txt"), "{html}");
    assert!(html.contains("href=\"a%3Cb%26c%3E.txt\""), "{html}");
    assert!(not_found(
        &fixture.sites.entry("gallery", "index.html").await
    ));
}

#[test]
fn invalid_inputs_and_symlinks_are_rejected_without_residue() {
    let fixture = fixture();
    let owner = Uuid::new_v4();

    for bad_name in [
        "",
        "-lead",
        "trail-",
        "Upper",
        "a_b",
        "a/b",
        &"a".repeat(64),
    ] {
        assert!(matches!(
            validate_site_name(bad_name),
            Err(SiteError::InvalidName)
        ));
    }
    assert!(validate_site_name("a").is_ok());
    assert!(validate_site_name(&"a".repeat(63)).is_ok());

    let empty = fixture.workspace.join("empty");
    fs::create_dir(&empty).expect("empty");
    assert!(matches!(
        publish(
            &fixture.database,
            fixture.agent,
            Some(owner),
            &fixture.workspace,
            "site-a",
            "empty"
        ),
        Err(SiteError::InvalidFolder(_))
    ));

    let secret = fixture.root.join("secret.txt");
    fs::write(&secret, "secret").expect("secret");
    let leaky = fixture.workspace.join("leaky");
    fs::create_dir(&leaky).expect("leaky");
    fs::write(leaky.join("index.html"), "ok").expect("index");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&secret, leaky.join("link.txt")).expect("symlink");

    assert!(matches!(
        publish(
            &fixture.database,
            fixture.agent,
            Some(owner),
            &fixture.workspace,
            "leaky",
            "leaky"
        ),
        Err(SiteError::InvalidFolder(_))
    ));

    let outside_dir = fixture.root.join("outside");
    fs::create_dir(&outside_dir).expect("outside");
    fs::write(outside_dir.join("index.html"), "outside").expect("outside index");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside_dir, fixture.workspace.join("linked-dir"))
        .expect("linked dir");

    for bad_folder in ["../outside", "/etc", "linked-dir", ""] {
        assert!(matches!(
            publish(
                &fixture.database,
                fixture.agent,
                Some(owner),
                &fixture.workspace,
                "bad",
                bad_folder
            ),
            Err(SiteError::InvalidFolder(_))
        ));
    }

    let too_many = fixture.workspace.join("many");
    fs::create_dir(&too_many).expect("many");
    for idx in 0..=MAX_SITE_ENTRIES {
        fs::write(too_many.join(format!("f{idx}.txt")), "x").expect("file");
    }
    assert!(matches!(
        publish(
            &fixture.database,
            fixture.agent,
            Some(owner),
            &fixture.workspace,
            "many",
            "many"
        ),
        Err(SiteError::TooManyEntries)
    ));

    assert_eq!(fixture.rows(), 0);
    assert!(
        fixture.snapshot_dirs().is_empty(),
        "rejected publishes must leave zero files or staging directories behind: {:?}",
        fixture.snapshot_dirs()
    );
}

#[tokio::test]
async fn ownership_listing_unpublish_and_path_traversal_guards() {
    let fixture = fixture();
    let (alice, bob) = (Uuid::new_v4(), Uuid::new_v4());
    let dir = fixture.workspace.join("app");
    fs::create_dir(&dir).expect("dir");
    fs::write(dir.join("index.html"), "hi").expect("index");

    publish(
        &fixture.database,
        fixture.agent,
        Some(alice),
        &fixture.workspace,
        "alpha",
        "app",
    )
    .expect("alice publish");
    publish(
        &fixture.database,
        fixture.agent,
        None,
        &fixture.workspace,
        "auto",
        "app",
    )
    .expect("automation publish");

    assert!(matches!(
        publish(
            &fixture.database,
            fixture.agent,
            Some(bob),
            &fixture.workspace,
            "alpha",
            "app"
        ),
        Err(SiteError::PermissionDenied)
    ));
    assert_eq!(fixture.snapshot_dirs().len(), 2);

    // The gateway is the owner's alone, so it serves every published site.
    assert_eq!(read_whole_file(&fixture.sites, "alpha", "").await, b"hi");
    assert!(not_found(
        &fixture.sites.entry("alpha", "../auto/index.html").await
    ));

    let bob_list = fixture.sites.list(bob).await.expect("bob list");
    assert_eq!(bob_list.len(), 1);
    assert_eq!(bob_list[0].name, "auto");

    let alice_list = fixture.sites.list(alice).await.expect("alice list");
    assert_eq!(
        alice_list
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha", "auto"]
    );
    assert_eq!(
        list_reachable(&fixture.database, None)
            .expect("none list")
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        vec!["auto"]
    );

    assert!(
        !fixture
            .sites
            .unpublish(bob, "alpha")
            .await
            .expect("bob unpub")
    );
    assert!(
        fixture
            .sites
            .unpublish(alice, "alpha")
            .await
            .expect("alice unpub")
    );
    assert_eq!(fixture.snapshot_dirs().len(), 1);
    assert!(
        unpublish(
            &fixture.database,
            &sites_dir_for_database(&fixture.database),
            None,
            "auto"
        )
        .expect("unpublish auto")
    );
    assert!(fixture.snapshot_dirs().is_empty());
}

#[test]
fn schema_55_gives_schema_54_catalog_empty_host_sites_table() {
    let fixture = fixture();
    let db = Connection::open(&fixture.database).expect("catalog");
    restore_schema_54(&db);
    db.execute_batch(
        "UPDATE host_metadata SET schema_version = 54 WHERE singleton = 1;
         PRAGMA user_version = 54;",
    )
    .expect("store schema 54");
    assert!(db.prepare("SELECT count(*) FROM host_sites").is_err());
    drop(db);

    catalog::initialize(&fixture.database).expect("upgrade schema 54");
    catalog::initialize(&fixture.database).expect("reopen schema 55");
    assert_eq!(fixture.rows(), 0);
}

#[test]
fn reset_clears_host_sites_rows_and_snapshots_leaving_workspace_intact() {
    let fixture = fixture();
    let dir = fixture.workspace.join("dist");
    fs::create_dir(&dir).expect("dist");
    fs::write(dir.join("index.html"), "<h1>keep workspace</h1>").expect("index");

    publish(
        &fixture.database,
        fixture.agent,
        None,
        &fixture.workspace,
        "site-one",
        "dist",
    )
    .expect("publish");
    assert_eq!(fixture.rows(), 1);
    assert_eq!(fixture.snapshot_dirs().len(), 1);

    reset_host_data_root(&fixture.root).expect("reset host");

    assert_eq!(
        fs::read_to_string(dir.join("index.html")).expect("workspace preserved"),
        "<h1>keep workspace</h1>"
    );
    assert!(fixture.snapshot_dirs().is_empty(), "site snapshots cleared");
    assert!(
        !fixture.database.exists() || {
            catalog::initialize(&fixture.database).expect("reinit");
            fixture.rows() == 0
        }
    );
}

#[test]
fn a_site_past_the_size_cap_is_refused_without_residue() {
    let fixture = fixture();
    // A site past the cap is refused before its large file is copied.
    let huge = fixture.workspace.join("huge");
    fs::create_dir(&huge).expect("huge");
    fs::write(huge.join("index.html"), "ok").expect("huge index");
    fs::File::create(huge.join("clip.mp4"))
        .expect("clip")
        .set_len(MAX_SITE_TOTAL_BYTES + 1)
        .expect("sparse clip");
    assert!(matches!(
        publish(
            &fixture.database,
            fixture.agent,
            None,
            &fixture.workspace,
            "huge",
            "huge"
        ),
        Err(SiteError::SiteTooLarge)
    ));

    assert_eq!(fixture.rows(), 0);
    assert!(fixture.snapshot_dirs().is_empty());
}

#[test]
fn a_site_published_without_a_person_stays_unowned_when_a_person_replaces_it() {
    let fixture = fixture();
    let (alice, bob) = (Uuid::new_v4(), Uuid::new_v4());
    let dir = fixture.workspace.join("app");
    fs::create_dir(&dir).expect("dir");
    fs::write(dir.join("index.html"), "hi").expect("index");
    let publish_as = |principal| {
        publish(
            &fixture.database,
            fixture.agent,
            principal,
            &fixture.workspace,
            "shared",
            "app",
        )
    };
    publish_as(None).expect("unowned publish");
    publish_as(Some(bob)).expect("anyone replaces an unowned site");
    let alice_sees = list_reachable(&fixture.database, Some(alice)).expect("alice list");
    assert_eq!(alice_sees.len(), 1, "bob's replace left the site unowned");
    publish_as(Some(alice)).expect("alice replaces it too");

    publish(
        &fixture.database,
        fixture.agent,
        Some(alice),
        &fixture.workspace,
        "alices",
        "app",
    )
    .expect("alice's own site");
    let before = fixture.snapshot_dirs();
    assert!(matches!(
        publish(
            &fixture.database,
            fixture.agent,
            None,
            &fixture.workspace,
            "alices",
            "app"
        ),
        Err(SiteError::PermissionDenied)
    ));
    assert_eq!(
        fixture.snapshot_dirs(),
        before,
        "a refused publish leaves nothing"
    );
}

#[test]
fn the_sweep_removes_only_what_no_site_names() {
    let fixture = fixture();
    let dir = fixture.workspace.join("app");
    fs::create_dir(&dir).expect("dir");
    fs::write(dir.join("index.html"), "hi").expect("index");
    publish(
        &fixture.database,
        fixture.agent,
        None,
        &fixture.workspace,
        "kept",
        "app",
    )
    .expect("publish");
    let kept = fixture.snapshot_dirs();
    fs::create_dir(fixture.sites_dir.join(".staging-crashed")).expect("staging");
    fs::create_dir(fixture.sites_dir.join(Uuid::new_v4().to_string())).expect("orphan");
    fs::write(fixture.sites_dir.join("stray"), "x").expect("stray file");

    assert_eq!(
        super::sweep(&fixture.database, &fixture.sites_dir).expect("sweep"),
        3
    );
    assert_eq!(fixture.snapshot_dirs(), kept);
    assert_eq!(
        super::sweep(&fixture.database, &fixture.sites_dir).expect("again"),
        0
    );
}

#[test]
fn a_fifo_in_the_folder_is_refused_without_blocking_or_residue() {
    let fixture = fixture();
    let dir = fixture.workspace.join("piped");
    fs::create_dir(&dir).expect("dir");
    fs::write(dir.join("index.html"), "hi").expect("index");
    nix::unistd::mkfifo(&dir.join("pipe"), nix::sys::stat::Mode::S_IRWXU).expect("fifo");
    assert!(matches!(
        publish(
            &fixture.database,
            fixture.agent,
            None,
            &fixture.workspace,
            "piped",
            "piped"
        ),
        Err(SiteError::InvalidFolder(_))
    ));
    assert_eq!(fixture.rows(), 0);
    assert!(fixture.snapshot_dirs().is_empty());
}

#[test]
fn both_ways_of_opening_sites_name_the_same_folder() {
    let directory = tempfile::tempdir().expect("directory");
    let home = RenoaHome::at(directory.path()).expect("home");
    assert_eq!(
        Sites::open(&home).sites_dir,
        Sites::from_database(&home.host_database()).sites_dir
    );
}
