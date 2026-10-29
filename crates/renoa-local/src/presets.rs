//! Code-owned creation presets.
//!
//! A preset is a versioned creation seed. Creation snapshots it into the
//! agent's own operational definition, and runtime resolution never consults a
//! preset again. Changing preset content requires a new preset id; a test pins
//! each preset's instructions and default `SOUL.md` to its id.

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
pub(crate) const ARCEE_PRESET_ID: &str = "renoa.personal.arcee.v3";
/// The seed general purpose agents can use.
pub(crate) const GENERAL_PRESET_ID: &str = "renoa.general.v1";
/// The seed for the everyday helpers people make in the app.
pub(crate) const APP_PRESET_ID: &str = "renoa.app.v1";

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

/// One immutable creation seed.
pub(crate) struct AgentPreset {
    id: AgentPresetId,
    description: &'static str,
    instructions: &'static str,
    behavior: AgentBehavior,
    documents: Option<AgentDocuments>,
    soul_default: Option<&'static str>,
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

    #[must_use]
    pub(crate) const fn behavior(&self) -> AgentBehavior {
        self.behavior
    }

    #[must_use]
    pub(crate) const fn documents(&self) -> Option<AgentDocuments> {
        self.documents
    }

    /// The `SOUL.md` a new agent from this preset starts with.
    #[must_use]
    pub(crate) const fn soul_default(&self) -> Option<&'static str> {
        self.soul_default
    }

    #[must_use]
    pub(crate) const fn provider_restriction(&self) -> Option<ModelProvider> {
        self.provider_restriction
    }

    #[must_use]
    pub(crate) const fn host_plugins_off(&self) -> &'static [HostPluginId] {
        self.host_plugins_off
    }

    #[must_use]
    pub(crate) const fn capability_baseline(&self) -> &'static [BuiltInCapability] {
        self.capability_baseline
    }

    pub(crate) fn instructions(&self, supplied: Option<&str>) -> String {
        supplied.unwrap_or(self.instructions).to_owned()
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
            documents: None,
            soul_default: None,
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
            documents: Some(AgentDocuments {
                soul: true,
                user: true,
            }),
            soul_default: Some(ARCEE_SOUL),
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
            documents: None,
            soul_default: None,
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
            documents: Some(AgentDocuments {
                soul: true,
                user: true,
            }),
            soul_default: Some(APP_SOUL),
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

    /// Every preset id with the SHA-256 of its instructions and default
    /// `SOUL.md`. Agents snapshot a preset at creation, so changed text needs a
    /// new preset id, never a new digest under an old one.
    const FROZEN: [(&str, &str, Option<&str>); 4] = [
        (
            ALPHA_PRESET_ID,
            "6dffe8edd7c94fcf6fb7bcda351714d671badda5533495f80251b4c68edb3194",
            None,
        ),
        (
            APP_PRESET_ID,
            "4493f36d0aa84775a12ad52dd86d765903485f816dccdb16f0a64c5babd63e80",
            Some("7b14e1d869cdf3578de62d0a5f8eddc1da71571878b88d58fc85600ae286ad9f"),
        ),
        (
            ARCEE_PRESET_ID,
            "48462133177f6d68525cb7ba700cc3cfb6b0ffdcf87f9773c2040c2da98a0a58",
            Some("ec44979b6960ec7d2192f8f3d284b3ed9ec122c846ce7417a0d316cc430a25bf"),
        ),
        (
            GENERAL_PRESET_ID,
            "55afe297f8f3a3c970d10ef4e66c0f7055978f530a6e89cc21082b560e443cb3",
            None,
        ),
    ];

    fn sha256(text: &str) -> String {
        let mut hex = String::with_capacity(64);
        for byte in content_hash(text.as_bytes()) {
            write!(hex, "{byte:02x}").expect("writing to a String cannot fail");
        }
        hex
    }

    #[test]
    fn preset_text_is_frozen_under_its_id() {
        for (id, instructions, soul) in FROZEN {
            let preset = preset(&AgentPresetId::new(id).expect("id")).expect("registered preset");
            let changed = format!("`{id}` changed; give the new text a new preset id");
            assert_eq!(sha256(preset.instructions), instructions, "{changed}");
            assert_eq!(
                preset.soul_default().map(sha256).as_deref(),
                soul,
                "{changed}"
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

    #[test]
    fn templates_supply_defaults_and_accept_explicit_instructions() {
        for (id, ..) in FROZEN {
            let template = preset(&AgentPresetId::new(id).expect("id")).expect("template");
            assert!(!template.instructions(None).is_empty());
            assert_eq!(template.instructions(Some("Do the job.")), "Do the job.");
        }
    }

    #[test]
    fn the_arcee_preset_keeps_today_operational_behavior() {
        let id = AgentPresetId::new(ARCEE_PRESET_ID).expect("portable preset id");
        let preset = preset(&id).expect("registered preset");
        let behavior = preset.behavior();
        assert!(preset.host_plugins_off().is_empty());
        assert!(behavior.loads_project_instructions());
        assert!(behavior.automatic_compaction.is_some());
        assert_eq!(
            preset.provider_restriction(),
            Some(crate::ModelProvider::OpenCodeGo)
        );
        let documents = preset.documents().expect("documents");
        assert!(documents.soul && documents.user);
        assert!(preset.soul_default().is_some());
    }

    #[test]
    fn the_general_template_has_no_machine_access() {
        let id = AgentPresetId::new(GENERAL_PRESET_ID).expect("portable preset id");
        let preset = preset(&id).expect("registered preset");
        assert!(preset.host_plugins_off().is_empty());
        assert!(!preset.behavior().loads_project_instructions());
        assert!(preset.documents().is_none());
        assert_eq!(preset.provider_restriction(), None);
    }
}
