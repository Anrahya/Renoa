//! Code-owned creation presets.
//!
//! A preset is a versioned creation seed. Creation snapshots it into the
//! agent's own operational definition, and runtime resolution never consults a
//! preset again. Changing preset content requires a new preset id. A test pins
//! each preset's seed to a digest as a tripwire against editing a released
//! preset in place; nothing enforces it at runtime.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use crate::{
    AgentBehavior, AgentDefinitionError, AgentDocuments, AgentPresetId, AutomaticCompaction,
    ModelProvider, WorkspaceInstructions, capabilities::BuiltInCapability,
    plugins::host::HostPluginId,
};

/// Renoa's built-in coding agent seed.
pub(crate) const ALPHA_PRESET_ID: &str = "renoa.coding.alpha.v3";
/// Renoa's personal operator agent seed.
pub(crate) const ARCEE_PRESET_ID: &str = "renoa.personal.arcee.v4";
/// The seed general purpose agents can use.
pub(crate) const GENERAL_PRESET_ID: &str = "renoa.general.v1";
/// The seed for the everyday helpers people make in the app.
pub(crate) const APP_PRESET_ID: &str = "renoa.app.v2";

const ALPHA_INSTRUCTIONS: &str = include_str!("../prompts/alpha-v1.md");
const ARCEE_INSTRUCTIONS: &str = include_str!("../prompts/arcee-v1/system.md");
const ARCEE_SOUL: &str = include_str!("../prompts/arcee-v1/SOUL.md");
const APP_INSTRUCTIONS: &str = include_str!("../prompts/app-v1/system.md");
const APP_SOUL: &str = include_str!("../prompts/app-v1/SOUL.md");

const ARCEE_COMPACTION_TRIGGER: u64 = 400_000;
const ARCEE_COMPACTION_TARGET: u64 = 40_000;

const ALPHA_CAPABILITY_BASELINE: &[BuiltInCapability] = &[
    BuiltInCapability::ReadFile,
    BuiltInCapability::EditFile,
    BuiltInCapability::WriteFile,
    BuiltInCapability::Bash,
    BuiltInCapability::Grep,
    BuiltInCapability::Find,
];

const ARCEE_CAPABILITY_BASELINE: &[BuiltInCapability] = &[
    BuiltInCapability::ReadFile,
    BuiltInCapability::EditFile,
    BuiltInCapability::WriteFile,
    BuiltInCapability::Bash,
    BuiltInCapability::Grep,
    BuiltInCapability::Find,
];

// The general and app presets grant no native file or shell tool. An everyday
// helper works through plugins, which every agent receives; the native tools act
// on a Host workspace that a person in the app never sees.
const NO_NATIVE_TOOLS: &[BuiltInCapability] = &[];

/// The documents a preset keeps. A soul is named by its default text, so a
/// preset cannot keep a `SOUL.md` without a default to publish.
#[derive(Clone, Copy)]
struct PresetDocuments {
    user: bool,
    soul: Option<&'static str>,
}

impl PresetDocuments {
    const NONE: Self = Self {
        user: false,
        soul: None,
    };
}

/// What a preset gives an agent: its operational seed and default `SOUL.md`.
/// Creation applies its request's choices on top; a move applies it as is.
#[derive(Clone, Copy)]
pub(crate) struct Seed {
    pub(crate) instructions: &'static str,
    pub(crate) behavior: AgentBehavior,
    pub(crate) documents: Option<AgentDocuments>,
    pub(crate) provider_restriction: Option<ModelProvider>,
    /// Present exactly when `documents` keeps a soul.
    pub(crate) soul: Option<&'static str>,
}

/// One immutable creation seed.
pub(crate) struct AgentPreset {
    id: AgentPresetId,
    description: &'static str,
    instructions: &'static str,
    behavior: AgentBehavior,
    documents: PresetDocuments,
    provider_restriction: Option<ModelProvider>,
    capability_baseline: &'static [BuiltInCapability],
    /// Compiled Host plugins a new agent starts with turned off.
    host_plugins_off: &'static [HostPluginId],
}

impl AgentPreset {
    #[must_use]
    pub(crate) fn id(&self) -> &AgentPresetId {
        &self.id
    }

    /// The model-facing description callers choose this preset by.
    #[must_use]
    pub(crate) const fn description(&self) -> &'static str {
        self.description
    }

    /// The one owner of what this preset seeds into an agent's definition.
    #[must_use]
    pub(crate) fn seed(&self) -> Seed {
        let documents = AgentDocuments {
            soul: self.documents.soul.is_some(),
            user: self.documents.user,
        };
        Seed {
            instructions: self.instructions,
            behavior: self.behavior,
            documents: documents.any().then_some(documents),
            provider_restriction: self.provider_restriction,
            soul: self.documents.soul,
        }
    }

    #[must_use]
    pub(crate) const fn host_plugins_off(&self) -> &'static [HostPluginId] {
        self.host_plugins_off
    }

    #[must_use]
    pub(crate) const fn capability_baseline(&self) -> &'static [BuiltInCapability] {
        self.capability_baseline
    }
}

static PRESETS: LazyLock<BTreeMap<AgentPresetId, AgentPreset>> = LazyLock::new(|| {
    seeds()
        .into_iter()
        .map(|preset| (preset.id.clone(), preset))
        .collect()
});

/// Every preset as written. Collecting into [`PRESETS`] would silently keep only
/// the last of two equal ids, so a test checks that nothing is dropped.
fn seeds() -> [AgentPreset; 4] {
    [
        AgentPreset {
            // Static preset ids are constants in this file, so construction
            // cannot fail at runtime.
            id: preset_id(ALPHA_PRESET_ID),
            description: "Renoa's coding agent for this workspace: curated coding instructions, project instructions, and the Host workspace tools.",
            instructions: ALPHA_INSTRUCTIONS,
            behavior: AgentBehavior {
                workspace_instructions: WorkspaceInstructions::ProjectAgentsFile,
                automatic_compaction: None,
            },
            documents: PresetDocuments::NONE,
            provider_restriction: None,
            capability_baseline: ALPHA_CAPABILITY_BASELINE,
            host_plugins_off: &[HostPluginId::Time],
        },
        AgentPreset {
            id: preset_id(ARCEE_PRESET_ID),
            description: "Renoa's personal operator: curation-owned instructions with SOUL and USER documents, automatic compaction, and the OpenCode Go provider.",
            instructions: ARCEE_INSTRUCTIONS,
            behavior: AgentBehavior {
                workspace_instructions: WorkspaceInstructions::ProjectAgentsFile,
                automatic_compaction: Some(AutomaticCompaction {
                    trigger_input_tokens: non_zero(ARCEE_COMPACTION_TRIGGER),
                    target_input_tokens: non_zero(ARCEE_COMPACTION_TARGET),
                }),
            },
            documents: PresetDocuments {
                user: true,
                soul: Some(ARCEE_SOUL),
            },
            provider_restriction: Some(ModelProvider::OpenCodeGo),
            capability_baseline: ARCEE_CAPABILITY_BASELINE,
            host_plugins_off: &[],
        },
        AgentPreset {
            id: preset_id(GENERAL_PRESET_ID),
            description: "A general purpose agent with helpful assistant instructions and no native file or shell tools. Explicit settings replace template defaults.",
            instructions: "You are a helpful assistant. Complete the assigned task and report the result clearly.",
            behavior: AgentBehavior {
                workspace_instructions: WorkspaceInstructions::Off,
                automatic_compaction: None,
            },
            documents: PresetDocuments::NONE,
            provider_restriction: None,
            capability_baseline: NO_NATIVE_TOOLS,
            host_plugins_off: &[],
        },
        AgentPreset {
            id: preset_id(APP_PRESET_ID),
            description: "An everyday helper a person makes in the app: plain-spoken instructions, its own SOUL.md and the person's USER.md, and no native file or shell tools.",
            instructions: APP_INSTRUCTIONS,
            behavior: AgentBehavior {
                workspace_instructions: WorkspaceInstructions::Off,
                automatic_compaction: None,
            },
            documents: PresetDocuments {
                user: true,
                soul: Some(APP_SOUL),
            },
            provider_restriction: None,
            capability_baseline: NO_NATIVE_TOOLS,
            host_plugins_off: &[],
        },
    ]
}

/// Every registered creation preset, in identity order.
pub(crate) fn catalog() -> impl Iterator<Item = &'static AgentPreset> {
    PRESETS.values()
}

/// Returns one registered creation preset.
///
/// # Errors
///
/// Returns an error for an unregistered preset identity.
pub(crate) fn preset(id: &AgentPresetId) -> Result<&'static AgentPreset, AgentDefinitionError> {
    PRESETS
        .get(id)
        .ok_or_else(|| AgentDefinitionError::UnknownPreset {
            preset: id.as_str().to_owned(),
        })
}

fn non_zero(value: u64) -> std::num::NonZeroU64 {
    std::num::NonZeroU64::new(value).expect("preset compaction bounds are non-zero")
}

fn preset_id(value: &str) -> AgentPresetId {
    AgentPresetId::new(value).expect("static preset ids are portable")
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::{
        ALPHA_PRESET_ID, APP_PRESET_ID, ARCEE_PRESET_ID, GENERAL_PRESET_ID, PRESETS, preset, seeds,
    };
    use crate::{AgentDefinitionError, AgentPresetId, atomic_file::content_hash};

    /// A tripwire, not enforcement: every preset id with the SHA-256 of its
    /// whole seed. Agents snapshot a seed at creation and a move rewrites one
    /// from it, so a released preset's seed changes only under a new id; this
    /// fails when someone edits one in place.
    const FROZEN: [(&str, &str); 4] = [
        (
            ALPHA_PRESET_ID,
            "257ee0931c8a7911a24f7440cfbf0ad9cff3705b6d07381726b5abacc4c9461e",
        ),
        (
            APP_PRESET_ID,
            "582f5205341d48cda19c066d113cbf678017e7ab187ec9420349840bd16329c4",
        ),
        (
            ARCEE_PRESET_ID,
            "9723236d71767a2bc28799d2009433403add63c6bb09550b67f8f3234eae76ca",
        ),
        (
            GENERAL_PRESET_ID,
            "b2bf09664f28cbdd40065ea3602cfa2637d214d713cf7115b2432027fc1fa001",
        ),
    ];

    fn seed_digest(id: &str) -> String {
        let seed = preset(&AgentPresetId::new(id).expect("id"))
            .expect("registered preset")
            .seed();
        // Serialized directly, never through `serde_json::Value`, whose key
        // order depends on which serde_json features the build unifies.
        let encoded = serde_json::to_string(&(
            seed.instructions,
            seed.behavior,
            seed.documents,
            seed.provider_restriction,
            seed.soul,
        ))
        .expect("a seed encodes");
        let mut hex = String::with_capacity(64);
        for byte in content_hash(encoded.as_bytes()) {
            write!(hex, "{byte:02x}").expect("writing to a String cannot fail");
        }
        hex
    }

    #[test]
    fn every_preset_seed_is_pinned_to_its_id() {
        let changed: Vec<String> = FROZEN
            .iter()
            .filter_map(|(id, pinned)| {
                let digest = seed_digest(id);
                (digest != *pinned).then(|| format!("{id} is now {digest}"))
            })
            .collect();
        assert!(
            changed.is_empty(),
            "a preset seed changed; give it a new preset id: {changed:?}"
        );
    }

    #[test]
    fn a_seed_keeps_a_soul_exactly_when_it_has_a_default() {
        for preset in super::catalog() {
            let seed = preset.seed();
            assert_eq!(
                seed.documents.is_some_and(|documents| documents.soul),
                seed.soul.is_some(),
                "{}",
                preset.id().as_str()
            );
        }
    }

    #[test]
    fn no_preset_is_dropped_by_a_duplicate_id() {
        assert_eq!(PRESETS.len(), seeds().len(), "two presets share an id");
    }

    #[test]
    fn every_static_preset_id_resolves() {
        for (id, ..) in FROZEN {
            let id = AgentPresetId::new(id).expect("portable preset id");
            let preset = preset(&id).expect("registered preset");
            assert_eq!(preset.id(), &id);
        }
        assert_eq!(
            super::catalog().count(),
            FROZEN.len(),
            "every registered preset has a static id"
        );
        let unregistered = AgentPresetId::new("renoa.unregistered.v1").expect("portable preset id");
        assert!(matches!(
            preset(&unregistered),
            Err(AgentDefinitionError::UnknownPreset { .. })
        ));
    }

    /// A preset that keeps USER.md says what it holds the way `agent_documents`
    /// does: lasting facts there, time-bound things in the notebook.
    #[test]
    fn a_preset_with_user_md_sends_time_bound_things_to_memory_note() {
        for preset in super::catalog() {
            let seed = preset.seed();
            if !seed.documents.is_some_and(|documents| documents.user) {
                continue;
            }
            let id = preset.id().as_str();
            assert!(seed.instructions.contains("memory_note"), "{id}");
            for retired in ["goals, commitments", "schedule information"] {
                assert!(!seed.instructions.contains(retired), "{id}: {retired}");
            }
        }
    }

    #[test]
    fn every_template_supplies_instructions() {
        for preset in super::catalog() {
            assert!(!preset.seed().instructions.trim().is_empty());
        }
    }

    #[test]
    fn the_arcee_preset_keeps_today_operational_behavior() {
        let id = AgentPresetId::new(ARCEE_PRESET_ID).expect("portable preset id");
        let preset = preset(&id).expect("registered preset");
        let seed = preset.seed();
        assert!(preset.host_plugins_off().is_empty());
        assert!(seed.behavior.loads_project_instructions());
        assert!(seed.behavior.automatic_compaction.is_some());
        assert_eq!(
            seed.provider_restriction,
            Some(crate::ModelProvider::OpenCodeGo)
        );
        let documents = seed.documents.expect("documents");
        assert!(documents.soul && documents.user);
        assert!(seed.soul.is_some());
    }

    #[test]
    fn the_general_template_has_no_native_tools_or_documents() {
        let id = AgentPresetId::new(GENERAL_PRESET_ID).expect("portable preset id");
        let preset = preset(&id).expect("registered preset");
        let seed = preset.seed();
        assert!(preset.host_plugins_off().is_empty());
        assert!(preset.capability_baseline().is_empty());
        assert!(!seed.behavior.loads_project_instructions());
        assert!(seed.documents.is_none());
        assert_eq!(seed.provider_restriction, None);
    }
}
