use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use renoa_agent::ToolErrorCode;
use renoa_kernel::AgentId;
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::files::{fault, revision};
use super::{AgentDocumentStore, Document};

const SOUL: &str = "Be careful.\n";

fn both() -> crate::AgentDocuments {
    crate::AgentDocuments {
        soul: true,
        user: true,
    }
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).expect("metadata").permissions().mode() & 0o777
}

fn opened(data: &Path, agent: AgentId) -> AgentDocumentStore {
    AgentDocumentStore::publish(data, agent, both(), SOUL).expect("publish");
    AgentDocumentStore::open(data, agent, both()).expect("open documents")
}

fn empty_revision() -> String {
    revision(b"")
}

#[test]
fn publication_creates_only_a_private_soul_and_is_adoptable() {
    let directory = tempdir().expect("temporary data directory");
    let agent = AgentId::new();
    AgentDocumentStore::publish(directory.path(), agent, both(), SOUL).expect("publish");

    let root = directory.path().join("agents").join(agent.to_string());
    let metadata = fs::symlink_metadata(root.join("SOUL.md")).expect("published soul");
    assert!(metadata.file_type().is_file());
    assert_eq!(mode(&root), 0o700);
    assert!(
        !root.join("USER.md").exists(),
        "USER.md belongs to a person, never to an agent"
    );
    assert!(!directory.path().join("users").exists());

    // A retry adopts the same publication instead of failing.
    AgentDocumentStore::publish(directory.path(), agent, both(), SOUL).expect("adopt");
}

#[test]
fn an_agent_that_reads_only_user_publishes_nothing() {
    let directory = tempdir().expect("temporary data directory");
    let agent = AgentId::new();
    let user_only = crate::AgentDocuments {
        soul: false,
        user: true,
    };
    AgentDocumentStore::publish(directory.path(), agent, user_only, SOUL).expect("publish");
    assert!(!directory.path().join("agents").exists());

    let store = AgentDocumentStore::open(directory.path(), agent, user_only).expect("open");
    assert!(store.render().expect("render").is_none());
    assert!(
        store.binding().is_none(),
        "without a person there is nothing to edit"
    );
}

#[test]
fn conflicting_pre_existing_content_fails_closed() {
    let directory = tempdir().expect("temporary data directory");
    let agent = AgentId::new();
    let root = directory.path().join("agents").join(agent.to_string());
    fs::create_dir_all(&root).expect("create document root");
    fs::write(root.join("SOUL.md"), "operator-written\n").expect("write conflicting document");

    let error = AgentDocumentStore::publish(directory.path(), agent, both(), SOUL)
        .expect_err("conflicting content must fail");
    assert!(
        error
            .to_string()
            .contains("already exists with different content"),
        "unexpected error: {error}"
    );
    assert_eq!(
        fs::read_to_string(root.join("SOUL.md")).expect("read document"),
        "operator-written\n"
    );
}

#[test]
fn conflicting_content_does_not_change_existing_root_permissions() {
    let directory = tempdir().expect("temporary data directory");
    let agent = AgentId::new();
    let root = directory.path().join("agents").join(agent.to_string());
    fs::create_dir_all(&root).expect("create document root");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755))
        .expect("set operator permissions");
    fs::write(root.join("SOUL.md"), "operator-written\n").expect("write conflict");

    AgentDocumentStore::publish(directory.path(), agent, both(), SOUL)
        .expect_err("conflicting content must fail");

    assert_eq!(
        mode(&root),
        0o755,
        "a rejected publication must not change existing directory metadata"
    );
}

#[test]
fn a_post_persist_failure_removes_the_document_it_installed() {
    let directory = tempdir().expect("temporary data directory");
    let agent = AgentId::new();
    let root = directory.path().join("agents").join(agent.to_string());
    fault::arm("SOUL.md", fault::Injection::PostPersistFailure);

    let error = AgentDocumentStore::publish(directory.path(), agent, both(), SOUL)
        .expect_err("an injected post-persist failure must fail the publication");
    fault::disarm();
    assert!(
        error.to_string().contains("sync agent document"),
        "unexpected error: {error}"
    );
    assert!(
        !root.exists(),
        "neither the document nor its empty root may survive a failed publication"
    );
}

#[test]
fn a_racing_writer_is_adopted_only_when_it_wrote_the_same_soul() {
    let directory = tempdir().expect("temporary data directory");
    let adopted = AgentId::new();
    fault::arm("SOUL.md", fault::Injection::IdenticalWinner);
    AgentDocumentStore::publish(directory.path(), adopted, both(), SOUL)
        .expect("an identical winner is adopted");
    fault::disarm();

    let refused = AgentId::new();
    let root = directory.path().join("agents").join(refused.to_string());
    fault::arm("SOUL.md", fault::Injection::ConflictingWinner);
    let error = AgentDocumentStore::publish(directory.path(), refused, both(), SOUL)
        .expect_err("a conflicting winner must fail closed");
    fault::disarm();
    assert!(
        error
            .to_string()
            .contains("already exists with different content"),
        "unexpected error: {error}"
    );
    assert_eq!(
        fs::read_to_string(root.join("SOUL.md")).expect("the winner survives"),
        "injected winner\n"
    );
}

#[test]
fn open_requires_the_soul_the_agent_keeps() {
    let directory = tempdir().expect("temporary data directory");
    let agent = AgentId::new();
    fs::create_dir_all(directory.path().join("agents").join(agent.to_string()))
        .expect("create document root");

    let missing = AgentDocumentStore::open(directory.path(), agent, both())
        .expect_err("an enabled missing soul must fail closed");
    assert!(
        missing.to_string().contains("regular file"),
        "unexpected: {missing}"
    );
}

#[test]
fn a_turn_shows_only_the_profile_of_the_person_it_talks_to() {
    let directory = tempdir().expect("temporary data directory");
    let store = opened(directory.path(), AgentId::new());
    let (owner, stranger) = (Uuid::new_v4(), Uuid::new_v4());
    let profile = directory.path().join("users").join(owner.to_string());
    fs::create_dir_all(&profile).expect("create profile directory");
    fs::write(profile.join("USER.md"), "Lives in Asia/Kolkata.\n").expect("write profile");

    let anonymous = store
        .clone()
        .with_principal(None)
        .render()
        .expect("render")
        .expect("documents");
    assert!(anonymous.contains("source=\"SOUL.md\""));
    assert!(
        !anonymous.contains("USER.md"),
        "a turn without a person has no USER.md"
    );

    let own = store
        .clone()
        .with_principal(Some(owner))
        .render()
        .expect("render")
        .expect("documents");
    assert!(own.contains("source=\"USER.md\""));
    assert!(own.contains("Lives in Asia/Kolkata."));

    let other = store
        .with_principal(Some(stranger))
        .render()
        .expect("render")
        .expect("documents");
    assert!(
        !other.contains("Asia/Kolkata"),
        "one person never sees another's"
    );
    assert!(
        other.contains(&format!("revision=\"{}\"", empty_revision())),
        "a person with no profile reads as empty"
    );
}

#[tokio::test]
async fn one_profile_is_shared_by_every_agent_that_talks_to_the_person() {
    let directory = tempdir().expect("temporary data directory");
    let person = Uuid::new_v4();
    let first = opened(directory.path(), AgentId::new()).with_principal(Some(person));
    let second = opened(directory.path(), AgentId::new()).with_principal(Some(person));

    let written = first
        .update(
            Document::User,
            &empty_revision(),
            "Prefers mornings.\n",
            &CancellationToken::new(),
        )
        .await
        .expect("the first edit creates the profile");
    assert!(
        second
            .render()
            .expect("render")
            .expect("documents")
            .contains("Prefers mornings."),
        "an edit by one agent reaches the next turn of every other agent"
    );

    let stale = second
        .update(
            Document::User,
            &empty_revision(),
            "Prefers evenings.\n",
            &CancellationToken::new(),
        )
        .await
        .expect_err("an edit based on an outdated profile must conflict");
    assert!(
        stale.to_string().contains("changed after this turn began"),
        "unexpected conflict: {stale}"
    );
    second
        .update(
            Document::User,
            &written,
            "Prefers mornings and tea.\n",
            &CancellationToken::new(),
        )
        .await
        .expect("an edit based on the current profile succeeds");
    assert!(
        first
            .render()
            .expect("render")
            .expect("documents")
            .contains("Prefers mornings and tea.")
    );
}

#[tokio::test]
async fn a_first_profile_edit_writes_a_private_file_and_a_stale_one_writes_nothing() {
    let directory = tempdir().expect("temporary data directory");
    let person = Uuid::new_v4();
    let store = opened(directory.path(), AgentId::new()).with_principal(Some(person));
    let users = directory.path().join("users");

    store
        .update(
            Document::User,
            &"0".repeat(64),
            "Guessed.\n",
            &CancellationToken::new(),
        )
        .await
        .expect_err("a stale first edit must conflict");
    assert!(
        !users.exists(),
        "a rejected first edit must leave no profile directory behind"
    );

    store
        .update(
            Document::User,
            &empty_revision(),
            "Stated.\n",
            &CancellationToken::new(),
        )
        .await
        .expect("first edit");
    let profile = users.join(person.to_string());
    assert_eq!(
        fs::read_to_string(profile.join("USER.md")).expect("profile"),
        "Stated.\n"
    );
    assert_eq!(mode(&users), 0o700);
    assert_eq!(mode(&profile), 0o700);
}

#[tokio::test]
async fn an_edit_makes_a_profile_directory_left_open_private() {
    let directory = tempdir().expect("temporary data directory");
    let person = Uuid::new_v4();
    let store = opened(directory.path(), AgentId::new()).with_principal(Some(person));
    let users = directory.path().join("users");
    let profile = users.join(person.to_string());
    // A first edit stopped between creating its directories and restricting them.
    fs::create_dir_all(&profile).expect("create profile directory");
    for path in [&users, &profile] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("open directory");
    }

    store
        .update(
            Document::User,
            &empty_revision(),
            "Stated.\n",
            &CancellationToken::new(),
        )
        .await
        .expect("edit");
    assert_eq!(mode(&users), 0o700);
    assert_eq!(mode(&profile), 0o700);
}

#[tokio::test]
async fn a_soul_edit_never_recreates_a_removed_soul() {
    let directory = tempdir().expect("temporary data directory");
    let agent = AgentId::new();
    let store = opened(directory.path(), agent);
    let soul = directory
        .path()
        .join("agents")
        .join(agent.to_string())
        .join("SOUL.md");
    fs::remove_file(&soul).expect("remove soul");

    store
        .update(
            Document::Soul,
            &empty_revision(),
            "Replacement.\n",
            &CancellationToken::new(),
        )
        .await
        .expect_err("an edit must not recreate a removed SOUL.md");
    assert!(!soul.exists());
}

#[tokio::test]
async fn a_turn_without_a_person_cannot_edit_a_profile() {
    let directory = tempdir().expect("temporary data directory");
    let store = opened(directory.path(), AgentId::new());

    let error = store
        .update(
            Document::User,
            &empty_revision(),
            "Anything.\n",
            &CancellationToken::new(),
        )
        .await
        .expect_err("no person means no USER.md");
    assert!(
        error.to_string().contains("no person is identified"),
        "unexpected: {error}"
    );
    assert!(!directory.path().join("users").exists());
}

#[tokio::test]
async fn a_linked_profile_directory_is_refused() {
    let directory = tempdir().expect("temporary data directory");
    let elsewhere = tempdir().expect("temporary escape target");
    let person = Uuid::new_v4();
    let store = opened(directory.path(), AgentId::new()).with_principal(Some(person));
    fs::create_dir(directory.path().join("users")).expect("create users directory");
    std::os::unix::fs::symlink(
        elsewhere.path(),
        directory.path().join("users").join(person.to_string()),
    )
    .expect("link escape");

    let error = store
        .render()
        .expect_err("a linked profile must fail closed");
    assert!(
        error.to_string().contains("never a link"),
        "unexpected: {error}"
    );
    let refused = store
        .update(
            Document::User,
            &empty_revision(),
            "Escaped.\n",
            &CancellationToken::new(),
        )
        .await
        .expect_err("editing through a linked profile must fail closed");
    assert_eq!(
        refused.code(),
        ToolErrorCode::InvalidInput,
        "a linked profile is refused as it stands, not reported as a storage failure to retry"
    );
    assert!(
        fs::read_dir(elsewhere.path())
            .expect("read escape target")
            .next()
            .is_none(),
        "the escape target must not receive a profile"
    );
}

#[test]
fn open_rejects_a_document_root_outside_the_data_directory() {
    let directory = tempdir().expect("temporary data directory");
    let elsewhere = tempdir().expect("temporary escape target");
    let agent = AgentId::new();
    fs::set_permissions(elsewhere.path(), fs::Permissions::from_mode(0o755))
        .expect("set escape target permissions");
    fs::create_dir_all(directory.path().join("agents")).expect("create agents directory");
    std::os::unix::fs::symlink(
        elsewhere.path(),
        directory.path().join("agents").join(agent.to_string()),
    )
    .expect("link escape");

    let error = AgentDocumentStore::open(directory.path(), agent, both())
        .expect_err("an escaping document root must fail closed");
    assert!(error.to_string().contains("outside"), "unexpected: {error}");

    let escape = AgentDocumentStore::publish(directory.path(), agent, both(), SOUL)
        .expect_err("publishing through an escaping document root must fail closed");
    assert!(
        escape.to_string().contains("outside"),
        "unexpected: {escape}"
    );
    assert!(
        fs::read_dir(elsewhere.path())
            .expect("read escape target")
            .next()
            .is_none(),
        "the escape target must not receive any publication"
    );
    assert_eq!(
        mode(elsewhere.path()),
        0o755,
        "the escape target's permissions must not be changed before the rejection"
    );
}

#[test]
fn an_in_tree_document_root_alias_is_refused() {
    let directory = tempdir().expect("temporary data directory");
    let owner = AgentId::new();
    let alias = AgentId::new();
    AgentDocumentStore::publish(directory.path(), owner, both(), SOUL).expect("publish owner");
    let agents = directory.path().join("agents");
    std::os::unix::fs::symlink(
        agents.join(owner.to_string()),
        agents.join(alias.to_string()),
    )
    .expect("link alias");

    let error = AgentDocumentStore::publish(directory.path(), alias, both(), SOUL)
        .expect_err("an aliased document root must fail closed");
    assert!(error.to_string().contains("outside"), "unexpected: {error}");
    assert_eq!(
        fs::read_to_string(agents.join(owner.to_string()).join("SOUL.md"))
            .expect("read owner document"),
        SOUL
    );
    assert!(
        fs::symlink_metadata(agents.join(alias.to_string()))
            .expect("alias metadata")
            .file_type()
            .is_symlink(),
        "the alias must not be replaced by a directory"
    );
}

#[tokio::test]
async fn content_hash_cas_rejects_a_stale_soul_revision_and_accepts_the_current_one() {
    let directory = tempdir().expect("temporary data directory");
    let store = opened(directory.path(), AgentId::new());

    let stale = store
        .update(
            Document::Soul,
            "0".repeat(64).as_str(),
            "new\n",
            &CancellationToken::new(),
        )
        .await
        .expect_err("a stale revision must conflict");
    assert!(
        stale.to_string().contains("changed after this turn began"),
        "unexpected conflict: {stale}"
    );

    let current = store
        .update(
            Document::Soul,
            &revision(SOUL.as_bytes()),
            "new soul\n",
            &CancellationToken::new(),
        )
        .await
        .expect("update with the current revision");
    assert_eq!(current.len(), 64);
    assert!(
        store
            .render()
            .expect("render")
            .expect("documents")
            .contains("new soul")
    );
}
