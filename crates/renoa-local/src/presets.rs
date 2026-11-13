//! Code-owned creation presets.
//!
//! A preset is a versioned creation seed. Creation snapshots it into the
//! agent's own operational definition, and runtime resolution never consults a
//! preset again. Changing preset content requires a new preset id. A test pins
//! each preset's seed to a digest as a tripwire against editing a released
//! preset in place; nothing enforces it at runtime.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::LazyLock;

use crate::{
    AgentBehavior, AgentDefinitionError, AgentDocuments, AgentPresetId, AutomaticCompaction,
    ModelProvider, WorkspaceInstructions, atomic_file::content_hash,
    capabilities::BuiltInCapability, plugins::host::HostPluginId,
};

/// Renoa's built-in coding agent seed.
pub(crate) const ALPHA_PRESET_ID: &str = "renoa.coding.alpha.v3";
/// Renoa's personal operator agent seed.
pub(crate) const ARCEE_PRESET_ID: &str = "renoa.personal.arcee.v5";
/// The seed general purpose agents can use.
pub(crate) const GENERAL_PRESET_ID: &str = "renoa.general.v1";
/// The seed for the everyday helpers people make in the app.
pub(crate) const APP_PRESET_ID: &str = "renoa.app.v7";

const ALPHA_INSTRUCTIONS: &str = include_str!("../prompts/alpha-v1.md");
const ARCEE_INSTRUCTIONS: &str = include_str!("../prompts/arcee-v1/system.md");
const ARCEE_SOUL: &str = include_str!("../prompts/arcee-v1/SOUL.md");
const APP_INSTRUCTIONS: &str = include_str!("../prompts/app-v1/system.md");
const APP_SOUL: &str = include_str!("../prompts/app-v1/SOUL.md");

const ARCEE_COMPACTION_TRIGGER: u64 = 400_000;
const ARCEE_COMPACTION_TARGET: u64 = 40_000;

// An everyday helper's one long chat compacts at 400,000 tokens, down to its
// summary and the newest messages within 40,000. A model whose dispatch limit
// is lower compacts at that limit (see `runtime::compaction_settings`).
const APP_COMPACTION_TRIGGER: u64 = 400_000;
const APP_COMPACTION_TARGET: u64 = 40_000;

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
            description: "A general purpose agent with helpful assistant instructions and no native file or shell tools.",
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
            description: "An everyday helper a person makes in the app: plain-spoken instructions, its own SOUL.md and the person's USER.md, no native file or shell tools, and its chat compacted at 400,000 tokens.",
            instructions: APP_INSTRUCTIONS,
            behavior: AgentBehavior {
                workspace_instructions: WorkspaceInstructions::Off,
                automatic_compaction: Some(AutomaticCompaction {
                    trigger_input_tokens: non_zero(APP_COMPACTION_TRIGGER),
                    target_input_tokens: non_zero(APP_COMPACTION_TARGET),
                }),
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

/// SHA-256 digests of `instructions` shipped with retired preset ids so
/// `move_agent_to_preset` does not mistake an unmodified retired preset base for
/// custom job rules.
const RETIRED_SEED_INSTRUCTION_DIGESTS: &[(&str, &str)] = &[
    (
        "renoa.app.v1",
        "49477e4b21977d03354ef003e65920649bb4237919d2f4ead4205990adeea7b3",
    ),
    (
        "renoa.app.v1",
        "4493f36d0aa84775a12ad52dd86d765903485f816dccdb16f0a64c5babd63e80",
    ),
    (
        "renoa.app.v1",
        "ebf93c935ba357aff3626ce2078e83601b3e97f96bbd94153d6c8dcc6039c554",
    ),
    (
        "renoa.app.v2",
        "f2cf6e459d31702c367748735c42178f0d856ffcbf507a49347a2db4bf56e950",
    ),
    (
        "renoa.app.v3",
        "8af0eb9324f528a8976acf82216d120282f13783d40b2b075f3893ff03bd53cb",
    ),
    (
        "renoa.app.v4",
        "8af0eb9324f528a8976acf82216d120282f13783d40b2b075f3893ff03bd53cb",
    ),
    (
        "renoa.app.v5",
        "a181a4a31001f7a93a486ac91721970eed18df245cc767cebe37bd9bc3db3be6",
    ),
    (
        "renoa.app.v5",
        "c66e52eb8a09c7b67ce904edb591ef3aed2300eaed8cb48635e9976e004a910c",
    ),
    (
        "renoa.app.v6",
        "c66e52eb8a09c7b67ce904edb591ef3aed2300eaed8cb48635e9976e004a910c",
    ),
    (
        "renoa.coding.alpha.v1",
        "6dffe8edd7c94fcf6fb7bcda351714d671badda5533495f80251b4c68edb3194",
    ),
    (
        "renoa.coding.alpha.v2",
        "6dffe8edd7c94fcf6fb7bcda351714d671badda5533495f80251b4c68edb3194",
    ),
    (
        "renoa.personal.arcee.v1",
        "fdd6add5d1f2f3a291eb4fd63a35254550fa2c2b8c5b1a5b3a6f691f24e70b72",
    ),
    (
        "renoa.personal.arcee.v1",
        "b7205eac0c49c059e5344b8c926d243f6485b74f4d12d3a25bcb5c33bff8e3fe",
    ),
    (
        "renoa.personal.arcee.v2",
        "b7205eac0c49c059e5344b8c926d243f6485b74f4d12d3a25bcb5c33bff8e3fe",
    ),
    (
        "renoa.personal.arcee.v3",
        "b7205eac0c49c059e5344b8c926d243f6485b74f4d12d3a25bcb5c33bff8e3fe",
    ),
    (
        "renoa.personal.arcee.v3",
        "c5c67ca799cb3d199459f95f87563e4a1fb06ee22dee664854203bdd854f608f",
    ),
    (
        "renoa.personal.arcee.v3",
        "48462133177f6d68525cb7ba700cc3cfb6b0ffdcf87f9773c2040c2da98a0a58",
    ),
    (
        "renoa.personal.arcee.v4",
        "fdff6b6416043ffca093355e5a9c8a20fb73d19954891f2034bf281b70f2c983",
    ),
];

/// Returns `true` when `instructions` matches a known stock seed for `id`
/// (either an active preset or a retired historical preset).
pub(crate) fn is_seed_instructions(id: &AgentPresetId, instructions: &str) -> bool {
    if let Ok(preset) = preset(id) {
        return preset.instructions == instructions;
    }
    let digest = sha256_hex(instructions.as_bytes());
    RETIRED_SEED_INSTRUCTION_DIGESTS
        .iter()
        .any(|(retired_id, known)| *retired_id == id.as_str() && *known == digest)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(64);
    for byte in content_hash(bytes) {
        write!(hex, "{byte:02x}").expect("writing to a String cannot fail");
    }
    hex
}

fn non_zero(value: u64) -> std::num::NonZeroU64 {
    std::num::NonZeroU64::new(value).expect("preset compaction bounds are non-zero")
}

fn preset_id(value: &str) -> AgentPresetId {
    AgentPresetId::new(value).expect("static preset ids are portable")
}

#[cfg(test)]
mod tests;
