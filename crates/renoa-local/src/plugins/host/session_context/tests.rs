use std::fs;

use mnem::mind::FILE_MAX;
use renoa_kernel::{AgentId, CommandId, CommandStanding};
use uuid::Uuid;

use super::{
    ContextRequest, PIN_FILE, STAGED_PIN_FILE, SessionContext,
    pin_fault::{self, Stage},
    resolve,
};
use crate::{
    host::catalog,
    plugins::host::memory::{self, fixtures},
};

struct Fixture {
    _directory: tempfile::TempDir,
    database: std::path::PathBuf,
    data: std::path::PathBuf,
    session: std::path::PathBuf,
    agent: AgentId,
    person: Uuid,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("fixture");
        let data = directory.path().join("data");
        let session = directory.path().join("session");
        fs::create_dir_all(&data).expect("data");
        fs::create_dir_all(&session).expect("session");
        let database = directory.path().join("host.sqlite3");
        catalog::initialize(&database).expect("catalog");
        Self {
            _directory: directory,
            database,
            data,
            session,
            agent: AgentId::new(),
            person: Uuid::new_v4(),
        }
    }

    fn notebook(&self) -> std::path::PathBuf {
        fixtures::notebook(&self.data, self.person)
    }

    /// The revision of the person's MIND.md as an agent that is not the
    /// Host's main agent sees it, as this fixture's agent is not.
    fn shared_revision(&self) -> String {
        memory::mind(&self.data, self.person)
            .expect("mind")
            .expect("written")
            .without_nudges()
            .revision
    }

    fn pin(&self) -> std::path::PathBuf {
        self.session.join(PIN_FILE)
    }

    fn resolve(&self, command: CommandId, standing: CommandStanding) -> (SessionContext, usize) {
        let pin = self.pin();
        let (context, skipped) = resolve(&ContextRequest {
            database: &self.database,
            data_directory: &self.data,
            agent: self.agent,
            principal: self.person,
            command,
            standing,
            pin: &pin,
        });
        (context, skipped.len())
    }
}

const NEW: CommandStanding = CommandStanding {
    admitted: false,
    other_unfinished: false,
};
const ADMITTED: CommandStanding = CommandStanding {
    admitted: true,
    other_unfinished: false,
};

fn revision(context: &SessionContext) -> Option<&str> {
    context.blocks.first().map(|block| block.revision.as_str())
}

#[test]
fn a_block_needs_a_mind_and_renders_escaped_after_its_source() {
    let fixture = Fixture::new();
    let (empty, skipped) = fixture.resolve(CommandId::new(), NEW);
    assert_eq!((empty.render(), skipped), (None, 0));
    assert!(!fixture.data.join("users").exists(), "nothing is created");
    assert!(!fixture.pin().exists(), "an empty context needs no pin");

    fixtures::dream(&fixture.notebook(), "Moving <house> & job");
    let revision = fixture.shared_revision();
    let (context, skipped) = fixture.resolve(CommandId::new(), NEW);
    assert_eq!(skipped, 0);
    let rendered = context.render().expect("a block");
    assert!(
        rendered.starts_with(&format!(
            "<session_context>\n<context source=\"plugin:renoa.memory\" revision=\"{revision}\">\nMIND.md: "
        )),
        "{rendered}"
    );
    assert!(
        rendered.contains("- Moving &lt;house&gt; &amp; job (#0)"),
        "{rendered}"
    );
    assert!(
        rendered.ends_with("\n</context>\n</session_context>"),
        "{rendered}"
    );
}

#[test]
fn a_pinned_command_keeps_its_block_and_a_new_one_reloads() {
    let fixture = Fixture::new();
    fixtures::dream(&fixture.notebook(), "Training for a 10k");
    let first = fixture.shared_revision();
    let command = CommandId::new();
    let (admitted, _) = fixture.resolve(command, NEW);
    assert_eq!(revision(&admitted), Some(first.as_str()));

    fixtures::dream(&fixture.notebook(), "Recovering from a sprain");
    let second = fixture.shared_revision();
    assert_ne!(first, second);
    for standing in [NEW, ADMITTED] {
        let (retried, skipped) = fixture.resolve(command, standing);
        assert_eq!(
            (retried, skipped),
            (admitted.clone(), 0),
            "a retry reuses its pin"
        );
    }
    let (next, _) = fixture.resolve(CommandId::new(), NEW);
    assert_eq!(
        revision(&next),
        Some(second.as_str()),
        "a new command reloads"
    );
}

#[test]
fn an_admitted_command_without_a_pin_keeps_no_block_and_a_refused_one_writes_none() {
    let fixture = Fixture::new();
    fixtures::dream(&fixture.notebook(), "Training for a 10k");
    let (none, _) = fixture.resolve(CommandId::new(), ADMITTED);
    assert_eq!(none.render(), None, "it was admitted with no block");

    let unfinished = CommandId::new();
    let (pinned, _) = fixture.resolve(unfinished, NEW);
    let refused = CommandStanding {
        admitted: false,
        other_unfinished: true,
    };
    let (shown, _) = fixture.resolve(CommandId::new(), refused);
    assert_eq!(shown, pinned, "the refused command still reads the block");
    let (recovered, _) = fixture.resolve(unfinished, ADMITTED);
    assert_eq!(recovered, pinned, "the unfinished command keeps its pin");
}

fn write_mind(fixture: &Fixture, text: &str) {
    fixtures::dream(&fixture.notebook(), "Training for a 10k");
    fs::write(fixture.notebook().join("MIND.md"), text).expect("MIND.md");
}

#[test]
fn a_corrupt_or_oversized_mind_is_skipped_and_reported() {
    let corrupt = Fixture::new();
    write_mind(&corrupt, "not a mind");
    assert_eq!(
        corrupt.resolve(CommandId::new(), NEW),
        (SessionContext::default(), 1)
    );

    let oversized = Fixture::new();
    let long = format!(
        "## Durable\n- {} (#0)\n## Active\n## Hunches\n## Nudges\n",
        "x".repeat(FILE_MAX)
    );
    write_mind(&oversized, &long);
    assert_eq!(
        oversized.resolve(CommandId::new(), NEW),
        (SessionContext::default(), 1)
    );
    assert!(!oversized.pin().exists(), "a skipped block is not pinned");
}

#[test]
fn an_unreadable_pin_is_reported_and_replaced() {
    let fixture = Fixture::new();
    fixtures::dream(&fixture.notebook(), "Training for a 10k");
    fs::write(fixture.pin(), "{").expect("a torn pin");
    let command = CommandId::new();
    let (context, skipped) = fixture.resolve(command, NEW);
    assert!(context.render().is_some());
    assert_eq!(skipped, 1);
    assert_eq!(
        fixture.resolve(command, ADMITTED),
        (context, 0),
        "pinned again"
    );
}

#[test]
fn every_mind_mnem_accepts_renders_however_it_escapes() {
    for largest in fixtures::largest_minds() {
        assert!(largest.len() <= FILE_MAX);
        let fixture = Fixture::new();
        write_mind(&fixture, &largest);
        assert!(
            memory::mind(&fixture.data, fixture.person)
                .expect("Mnem accepts it")
                .is_some()
        );
        let (context, skipped) = fixture.resolve(CommandId::new(), NEW);
        assert_eq!(skipped, 0, "none is left out for its size");
        assert!(context.render().is_some());
    }
}

#[test]
fn a_pin_replaced_but_not_made_durable_still_carries_its_block() {
    let fixture = Fixture::new();
    fixtures::dream(&fixture.notebook(), "Training for a 10k");
    let command = CommandId::new();
    pin_fault::arm(&fixture.pin(), Stage::AfterRename);
    let (context, skipped) = fixture.resolve(command, NEW);
    assert!(
        context.render().is_some(),
        "the prompt carries what the pin holds"
    );
    assert_eq!(skipped, 1, "the failure is reported");
    assert_eq!(
        fixture.resolve(command, ADMITTED),
        (context, 0),
        "a retry reads the same pin"
    );
}

#[test]
fn a_pin_not_written_leaves_no_staged_file_and_no_block() {
    let fixture = Fixture::new();
    fixtures::dream(&fixture.notebook(), "Training for a 10k");
    let command = CommandId::new();
    pin_fault::arm(&fixture.pin(), Stage::BeforeRename);
    let (context, skipped) = fixture.resolve(command, NEW);
    assert_eq!((context.render(), skipped), (None, 1));
    assert!(
        !fixture.session.join(STAGED_PIN_FILE).exists(),
        "no staged file"
    );
    assert!(!fixture.pin().exists());
    assert_eq!(
        fixture.resolve(command, ADMITTED),
        (SessionContext::default(), 0),
        "a retry finds no block either"
    );
}

#[test]
fn the_pin_is_one_file_in_the_session_directory() {
    let fixture = Fixture::new();
    fixtures::dream(&fixture.notebook(), "Training for a 10k");
    fixture.resolve(CommandId::new(), NEW);
    let entries: Vec<String> = fs::read_dir(&fixture.session)
        .expect("session directory")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(entries, [PIN_FILE], "no staged file is left behind");
}
