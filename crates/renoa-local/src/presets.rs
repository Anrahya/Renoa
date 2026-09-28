//! Code-owned creation presets.
//!
//! A preset is a versioned creation seed. Creation snapshots it into the
//! agent's own operational definition, and runtime resolution never consults a
//! preset again. Changing preset content requires a new preset id.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use crate::{
    AgentBehavior, AgentDefinitionError, AgentDocuments, AgentPresetId, AutomaticCompaction,
    ModelProvider, TurnTiming, WorkspaceInstructions, capabilities::BuiltInCapability,
};

/// Renoa's built-in coding agent seed.
pub(crate) const ALPHA_PRESET_ID: &str = "renoa.coding.alpha.v3";
/// Renoa's personal operator agent seed.
pub(crate) const ARCEE_PRESET_ID: &str = "renoa.personal.arcee.v3";
/// The seed general purpose agents can use.
pub(crate) const GENERAL_PRESET_ID: &str = "renoa.general.v1";

const ALPHA_INSTRUCTIONS: &str = include_str!("../prompts/alpha-v1.md");
const ARCEE_INSTRUCTIONS: &str = include_str!("../prompts/arcee-v1/system.md");
const ARCEE_SOUL: &str = include_str!("../prompts/arcee-v1/SOUL.md");

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

const GENERAL_CAPABILITY_BASELINE: &[BuiltInCapability] = &[];

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
    pub(crate) const fn capability_baseline(&self) -> &'static [BuiltInCapability] {
        self.capability_baseline
    }

    pub(crate) fn instructions(&self, supplied: Option<&str>) -> String {
        supplied.unwrap_or(self.instructions).to_owned()
    }
}

static PRESETS: LazyLock<BTreeMap<AgentPresetId, AgentPreset>> = LazyLock::new(|| {
    let presets = [
        AgentPreset {
            // Static preset ids are constants in this file, so construction
            // cannot fail at runtime.
            id: preset_id(ALPHA_PRESET_ID),
            description: "Renoa's coding agent for this workspace: curated coding instructions, project instructions, and the Host workspace tools.",
            instructions: ALPHA_INSTRUCTIONS,
            behavior: AgentBehavior {
                turn_timing: TurnTiming::Off,
                workspace_instructions: WorkspaceInstructions::ProjectAgentsFile,
                automatic_compaction: None,
            },
            documents: None,
            soul_default: None,
            provider_restriction: None,
            capability_baseline: ALPHA_CAPABILITY_BASELINE,
        },
        AgentPreset {
            id: preset_id(ARCEE_PRESET_ID),
            description: "Renoa's personal operator: curation-owned instructions with SOUL and USER documents, Host turn timing, automatic compaction, and the OpenCode Go provider.",
            instructions: ARCEE_INSTRUCTIONS,
            behavior: AgentBehavior {
                turn_timing: TurnTiming::HostClock,
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
        },
        AgentPreset {
            id: preset_id(GENERAL_PRESET_ID),
            description: "A general purpose agent with helpful assistant instructions and no machine tools. Explicit settings replace template defaults.",
            instructions: "You are a helpful assistant. Complete the assigned task and report the result clearly.",
            behavior: AgentBehavior {
                turn_timing: TurnTiming::HostClock,
                workspace_instructions: WorkspaceInstructions::Off,
                automatic_compaction: None,
            },
            documents: None,
            soul_default: None,
            provider_restriction: None,
            capability_baseline: GENERAL_CAPABILITY_BASELINE,
        },
    ];
    presets
        .into_iter()
        .map(|preset| (preset.id.clone(), preset))
        .collect()
});

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
    use super::{ALPHA_PRESET_ID, ARCEE_PRESET_ID, GENERAL_PRESET_ID, preset};
    use crate::{AgentDefinitionError, AgentPresetId};

    #[test]
    fn every_static_preset_id_resolves() {
        for id in [ALPHA_PRESET_ID, ARCEE_PRESET_ID, GENERAL_PRESET_ID] {
            let id = AgentPresetId::new(id).expect("portable preset id");
            let preset = preset(&id).expect("registered preset");
            assert_eq!(preset.id(), &id);
        }
        let unregistered = AgentPresetId::new("renoa.unregistered.v1").expect("portable preset id");
        assert!(matches!(
            preset(&unregistered),
            Err(AgentDefinitionError::UnknownPreset { .. })
        ));
    }

    #[test]
    fn templates_supply_defaults_and_accept_explicit_instructions() {
        for id in [ALPHA_PRESET_ID, ARCEE_PRESET_ID, GENERAL_PRESET_ID] {
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
        assert!(behavior.uses_turn_timing());
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
        assert!(preset.behavior().uses_turn_timing());
        assert!(!preset.behavior().loads_project_instructions());
        assert!(preset.documents().is_none());
        assert_eq!(preset.provider_restriction(), None);
    }
}
