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
fn plugin_skills_hot_load_replace_the_same_plugin_and_reject_cross_plugin_collisions() {
    let fixture = Fixture::new();
    let first = fixture.workspace.join("plugin-one-v1");
    write_skill(
        &first.join("skills"),
        "review",
        "First plugin review.",
        "PLUGIN_ONE_V1",
    );

    let first_report = fixture
        .store
        .sync_plugin(PROFILE, "plugin-one", &first)
        .expect("load first plugin skills");
    assert_eq!(first_report.accepted, ["review"]);
    assert!(first_report.rejected.is_empty());
    assert_eq!(
        fixture
            .store
            .summaries(PROFILE, &fixture.workspace)
            .expect("read hot-loaded plugin skill")[0]
            .description,
        "First plugin review."
    );

    let second = fixture.workspace.join("plugin-one-v2");
    write_skill(
        &second.join("skills"),
        "review",
        "Second plugin review.",
        "PLUGIN_ONE_V2",
    );
    fixture
        .store
        .sync_plugin(PROFILE, "plugin-one", &second)
        .expect("replace one plugin's skill revision");
    let loaded = fixture
        .store
        .activate(
            PROFILE,
            &fixture.workspace,
            SessionId::new(),
            CommandId::new(),
            "review",
        )
        .expect("activate replacement plugin skill");
    assert_eq!(loaded.body.trim(), "PLUGIN_ONE_V2");

    let competing = fixture.workspace.join("plugin-two");
    write_skill(
        &competing.join("skills"),
        "review",
        "Competing review.",
        "PLUGIN_TWO",
    );
    let conflict = fixture
        .store
        .sync_plugin(PROFILE, "plugin-two", &competing)
        .expect("isolate a plugin skill collision");
    assert!(conflict.accepted.is_empty());
    assert_eq!(conflict.rejected.len(), 1);
    assert_eq!(conflict.rejected[0].entry(), "review");
    assert!(conflict.rejected[0].reason().contains("plugin-one"));

    let replay = fixture
        .store
        .sync_plugin(PROFILE, "plugin-one", &second)
        .expect("replay the same plugin revision");
    assert_eq!(replay.accepted, ["review"]);
    assert!(replay.rejected.is_empty());
}

#[test]
fn project_and_global_skills_override_plugin_skills_without_mutating_the_plugin() {
    let fixture = Fixture::new();
    let plugin = fixture.workspace.join("plugin");
    write_skill(&plugin.join("skills"), "review", "Plugin review.", "PLUGIN");
    fixture
        .store
        .sync_plugin(PROFILE, "plugin", &plugin)
        .expect("load plugin skill");
    write_skill(&fixture.global, "review", "Global review.", "GLOBAL");
    fixture
        .store
        .sync(PROFILE, &fixture.workspace)
        .expect("load global override");
    assert_eq!(
        fixture
            .store
            .summaries(PROFILE, &fixture.workspace)
            .expect("resolve global precedence")[0]
            .description,
        "Global review."
    );

    let project = fixture.workspace.join(".agents/skills");
    write_skill(&project, "review", "Project review.", "PROJECT");
    fixture
        .store
        .sync(PROFILE, &fixture.workspace)
        .expect("load project override");
    assert_eq!(
        fixture
            .store
            .summaries(PROFILE, &fixture.workspace)
            .expect("resolve project precedence")[0]
            .description,
        "Project review."
    );
}

#[test]
fn large_active_instructions_are_preserved_without_a_host_policy_limit() {
    let fixture = Fixture::new();
    let source = fixture.workspace.join(".agents/skills");
    let body = "x".repeat(300 * 1_024);
    write_skill(&source, "large", "Large instructions.", &body);
    fixture
        .store
        .sync(PROFILE, &fixture.workspace)
        .expect("publish large valid package");
    let session_id = SessionId::new();

    fixture
        .store
        .activate(
            PROFILE,
            &fixture.workspace,
            session_id,
            CommandId::new(),
            "large",
        )
        .expect("activate instructions larger than the former Host limit");
    let active = fixture
        .store
        .active(session_id, None)
        .expect("load large active skill");
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].body, format!("{body}\n"));
}

#[test]
fn more_than_sixteen_skills_can_be_active() {
    const SKILL_COUNT: usize = 17;

    let fixture = Fixture::new();
    let source = fixture.workspace.join(".agents/skills");
    for index in 0..SKILL_COUNT {
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
    for name in &names {
        fixture
            .store
            .activate(
                PROFILE,
                &fixture.workspace,
                session_id,
                CommandId::new(),
                name,
            )
            .expect("activate skill without an arbitrary count policy");
    }

    assert_eq!(
        fixture
            .store
            .active(session_id, None)
            .expect("load all active skills")
            .len(),
        SKILL_COUNT
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
