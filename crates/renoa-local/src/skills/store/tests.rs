use std::{fs, path::Path};

use renoa_kernel::{CommandId, SessionId};
use tempfile::{TempDir, tempdir};

use super::SkillStore;
use crate::{host::catalog, skills::SkillError};

const PROFILE: &str = "renoa.coding.alpha.v1";

struct Fixture {
    _directory: TempDir,
    workspace: std::path::PathBuf,
    global: std::path::PathBuf,
    packages: std::path::PathBuf,
    database: std::path::PathBuf,
    store: SkillStore,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempdir().expect("temporary skill Host");
        let workspace = directory.path().join("workspace");
        let global = directory.path().join("global");
        let packages = directory.path().join("data/skills");
        let database = directory.path().join("data/host.sqlite3");
        fs::create_dir_all(&workspace).expect("create workspace");
        fs::create_dir_all(&global).expect("create global source");
        fs::create_dir_all(database.parent().expect("database parent"))
            .expect("create data directory");
        catalog::initialize(&database).expect("initialize Host catalog");
        let store =
            SkillStore::initialize(database.clone(), packages.clone(), Some(global.clone()))
                .expect("initialize skill store");
        Self {
            _directory: directory,
            workspace,
            global,
            packages,
            database,
            store,
        }
    }

    fn reopen(&self) -> SkillStore {
        SkillStore::initialize(
            self.database.clone(),
            self.packages.clone(),
            Some(self.global.clone()),
        )
        .expect("reopen skill store")
    }
}

#[test]
fn hot_sync_keeps_collisions_exact_and_active_revisions_stable() {
    let fixture = Fixture::new();
    let (project, first_digest, session_id) = activate_initial_project_revision(&fixture);

    write_skill(
        &project,
        "review",
        "Project review.",
        "PROJECT_REVISION_TWO",
    );
    write_skill(&project, "hot-helper", "New helper.", "HOT_HELPER_BODY");
    fixture
        .store
        .sync(PROFILE, &fixture.workspace)
        .expect("hot sync changed source without restart");
    let summaries = fixture
        .store
        .summaries(PROFILE, &fixture.workspace)
        .expect("changed summaries");
    assert_eq!(summaries.len(), 2);
    assert_eq!(
        summaries
            .iter()
            .find(|skill| skill.name == "review")
            .expect("new project revision")
            .description,
        "Project review."
    );
    assert!(fixture.packages.join(&first_digest).is_dir());
    let pinned = fixture
        .store
        .activate(
            PROFILE,
            &fixture.workspace,
            session_id,
            CommandId::new(),
            "review",
        )
        .expect("existing session keeps its active revision after source changes");
    assert_eq!(pinned.digest, first_digest);
    assert_eq!(pinned.body.trim(), "PROJECT_REVISION_ONE");

    let current = fixture
        .store
        .activate(
            PROFILE,
            &fixture.workspace,
            SessionId::new(),
            CommandId::new(),
            "review",
        )
        .expect("new session resolves the current project revision");
    assert_ne!(current.digest, first_digest);
    assert_eq!(current.body.trim(), "PROJECT_REVISION_TWO");
    fixture
        .store
        .activate(
            PROFILE,
            &fixture.workspace,
            session_id,
            CommandId::new(),
            "hot-helper",
        )
        .expect("activate hot-added helper");

    let reopened = fixture.reopen();
    let active = reopened
        .active(session_id, None)
        .expect("load durable activations");
    assert_eq!(
        active
            .iter()
            .map(|skill| (skill.metadata.name.as_str(), skill.body.trim()))
            .collect::<Vec<_>>(),
        [
            ("review", "PROJECT_REVISION_ONE"),
            ("hot-helper", "HOT_HELPER_BODY"),
        ]
    );
    reopened
        .remove_session(session_id)
        .expect("remove session activations");
    assert!(
        reopened
            .active(session_id, None)
            .expect("reload removed session")
            .is_empty()
    );
}

fn activate_initial_project_revision(fixture: &Fixture) -> (std::path::PathBuf, String, SessionId) {
    write_skill(
        &fixture.global,
        "review",
        "Global review.",
        "GLOBAL_REVISION",
    );
    let project = fixture.workspace.join(".agents/skills");
    write_skill(
        &project,
        "review",
        "Project review.",
        "PROJECT_REVISION_ONE",
    );

    fixture
        .store
        .sync(PROFILE, &fixture.workspace)
        .expect("initial hot sync");
    let summaries = fixture
        .store
        .summaries(PROFILE, &fixture.workspace)
        .expect("initial summaries");
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].name, "review");
    assert_eq!(summaries[0].description, "Project review.");
    let session_id = SessionId::new();
    let command_id = CommandId::new();
    let active = fixture
        .store
        .activate(
            PROFILE,
            &fixture.workspace,
            session_id,
            command_id,
            "review",
        )
        .expect("activate selected project revision");
    let first_digest = active.digest.clone();
    assert_eq!(active.body.trim(), "PROJECT_REVISION_ONE");
    assert!(
        fixture
            .store
            .active(session_id, Some(command_id))
            .expect("resolve the activating command's frozen skill context")
            .is_empty(),
        "an unfinished command must resume with its original skill context"
    );
    assert_eq!(
        fixture
            .store
            .active(session_id, None)
            .expect("resolve the following command's skill context")
            .len(),
        1,
        "the activation must become effective for the following command"
    );
    (project, first_digest, session_id)
}

#[test]
fn a_failed_source_scan_keeps_the_last_complete_binding_snapshot() {
    let fixture = Fixture::new();
    let source = fixture.workspace.join(".agents/skills");
    write_skill(&source, "stable", "Stable skill.", "STABLE_BODY");
    fixture
        .store
        .sync(PROFILE, &fixture.workspace)
        .expect("publish initial source snapshot");
    let backup = fixture.workspace.join(".agents/skills-backup");
    fs::rename(&source, &backup).expect("move source out of the way");
    fs::write(&source, "not a directory").expect("replace source with invalid entry");

    assert!(matches!(
        fixture.store.sync(PROFILE, &fixture.workspace),
        Err(SkillError::Invalid(_))
    ));
    assert_eq!(
        fixture
            .store
            .summaries(PROFILE, &fixture.workspace)
            .expect("load prior complete snapshot")
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>(),
        ["stable"]
    );
}

#[test]
fn invalid_entries_are_reported_without_hiding_valid_skills() {
    let fixture = Fixture::new();
    let source = fixture.workspace.join(".agents/skills");
    write_skill(&source, "valid", "Valid skill.", "VALID_BODY");
    let invalid = source.join("permission-bearing");
    fs::create_dir_all(&invalid).expect("create unsupported skill");
    fs::write(
        invalid.join("SKILL.md"),
        "---\nname: permission-bearing\ndescription: Requests implicit permission.\nallowed-tools: Bash\n---\nNever imported.\n",
    )
    .expect("write unsupported skill");

    fixture
        .store
        .sync(PROFILE, &fixture.workspace)
        .expect("isolate unsupported entry");

    assert_eq!(
        fixture
            .store
            .rejection_count(PROFILE, &fixture.workspace)
            .expect("count rejected skill entries"),
        1
    );
    assert_eq!(
        fixture
            .store
            .summaries(PROFILE, &fixture.workspace)
            .expect("load valid skill")
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>(),
        ["valid"]
    );
}

#[test]
fn oversized_active_instructions_fail_instead_of_being_truncated() {
    let fixture = Fixture::new();
    let source = fixture.workspace.join(".agents/skills");
    let body = "x".repeat(crate::skills::MAX_ACTIVE_SKILL_INSTRUCTION_BYTES);
    write_skill(&source, "oversized", "Oversized instructions.", &body);
    fixture
        .store
        .sync(PROFILE, &fixture.workspace)
        .expect("publish oversized but valid package");
    let session_id = SessionId::new();

    assert!(matches!(
        fixture.store.activate(
            PROFILE,
            &fixture.workspace,
            session_id,
            CommandId::new(),
            "oversized",
        ),
        Err(SkillError::Conflict(_))
    ));
    assert!(
        fixture
            .store
            .active(session_id, None)
            .expect("load rejected activation state")
            .is_empty()
    );
}

#[test]
fn active_skill_count_fails_at_the_seventeenth_revision() {
    let fixture = Fixture::new();
    let source = fixture.workspace.join(".agents/skills");
    for index in 0..=crate::skills::MAX_ACTIVE_SKILLS {
        write_skill(
            &source,
            &format!("skill-{index}"),
            "Bounded skill.",
            "Follow the bounded workflow.",
        );
    }
    fixture
        .store
        .sync(PROFILE, &fixture.workspace)
        .expect("publish bounded skills");
    let names = fixture
        .store
        .summaries(PROFILE, &fixture.workspace)
        .expect("load bounded skill summaries")
        .into_iter()
        .map(|skill| skill.name)
        .collect::<Vec<_>>();
    let session_id = SessionId::new();
    for name in &names[..crate::skills::MAX_ACTIVE_SKILLS] {
        fixture
            .store
            .activate(
                PROFILE,
                &fixture.workspace,
                session_id,
                CommandId::new(),
                name,
            )
            .expect("activate skill within count limit");
    }

    assert!(matches!(
        fixture.store.activate(
            PROFILE,
            &fixture.workspace,
            session_id,
            CommandId::new(),
            &names[crate::skills::MAX_ACTIVE_SKILLS],
        ),
        Err(SkillError::Conflict(_))
    ));
    assert_eq!(
        fixture
            .store
            .active(session_id, None)
            .expect("load bounded active skills")
            .len(),
        crate::skills::MAX_ACTIVE_SKILLS
    );
}

fn write_skill(root: &Path, name: &str, description: &str, body: &str) {
    let directory = root.join(name);
    fs::create_dir_all(&directory).expect("create skill directory");
    fs::write(
        directory.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n{body}\n"),
    )
    .expect("write skill");
}
