//! Code-owned creation presets.
//!
//! A preset is a versioned creation seed. Creation snapshots it into the
//! agent's own operational definition, and runtime resolution never consults a
//! preset again. Changing preset content requires a new preset id.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use crate::{
    AgentBehavior, AgentDefinitionError, AgentDocuments, AgentPresetId, AutomaticCompaction,
    ModelProvider, TurnTiming, WorkspaceInstructions,
    capabilities::{self, PresetToolBaseline},
    documents::DocumentDefaults,
};

/// Renoa's built-in coding profile seed.
pub(crate) const ALPHA_PRESET_ID: &str = "renoa.coding.alpha.v1";
/// Renoa's personal operator profile seed.
pub(crate) const ARCEE_PRESET_ID: &str = "renoa.personal.arcee.v1";
/// The seed every caller-defined specialist agent is created from.
pub(crate) const SPECIALIST_PRESET_ID: &str = "renoa.specialist.v1";

const ALPHA_INSTRUCTIONS: &str = include_str!("../prompts/alpha-v1.md");
const ARCEE_INSTRUCTIONS: &str = include_str!("../prompts/arcee-v1/system.md");
const ARCEE_SOUL: &str = include_str!("../prompts/arcee-v1/SOUL.md");
const ARCEE_USER: &str = include_str!("../prompts/arcee-v1/USER.md");

const ARCEE_COMPACTION_TRIGGER: u64 = 400_000;
const ARCEE_COMPACTION_TARGET: u64 = 40_000;

/// Where a preset's instructions come from.
#[derive(Clone, Copy)]
pub(crate) enum PresetInstructions {
    /// The preset owns the instructions and rejects caller overrides.
    Fixed(&'static str),
    /// The caller supplies the instructions.
    CallerSupplied,
}

/// One immutable creation seed.
pub(crate) struct AgentPreset {
    id: AgentPresetId,
    description: &'static str,
    instructions: PresetInstructions,
    behavior: AgentBehavior,
    documents: Option<AgentDocuments>,
    document_defaults: Option<DocumentDefaults>,
    provider_restriction: Option<ModelProvider>,
    tool_baseline: PresetToolBaseline,
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

    #[must_use]
    pub(crate) const fn document_defaults(&self) -> Option<DocumentDefaults> {
        self.document_defaults
    }

    #[must_use]
    pub(crate) const fn provider_restriction(&self) -> Option<ModelProvider> {
        self.provider_restriction
    }

    #[must_use]
    pub(crate) const fn tool_baseline(&self) -> PresetToolBaseline {
        self.tool_baseline
    }

    /// Resolves the instructions this preset stores for a new agent.
    ///
    /// # Errors
    ///
    /// Rejects a missing caller value or a caller override of a fixed preset.
    pub(crate) fn instructions(
        &self,
        supplied: Option<&str>,
    ) -> Result<String, AgentDefinitionError> {
        match self.instructions {
            PresetInstructions::Fixed(text) => match supplied {
                Some(_) => Err(AgentDefinitionError::InstructionsNotAllowed {
                    preset: self.id.as_str().to_owned(),
                }),
                None => Ok(text.to_owned()),
            },
            PresetInstructions::CallerSupplied => supplied.map(str::to_owned).ok_or_else(|| {
                AgentDefinitionError::InstructionsRequired {
                    preset: self.id.as_str().to_owned(),
                }
            }),
        }
    }
}

static PRESETS: LazyLock<BTreeMap<AgentPresetId, AgentPreset>> = LazyLock::new(|| {
    let presets = [
        AgentPreset {
            // Static preset ids are constants in this file, so construction
            // cannot fail at runtime.
            id: preset_id(ALPHA_PRESET_ID),
            description: "Renoa's coding agent for this workspace: curated coding instructions, project instructions, and the Host workspace tools.",
            instructions: PresetInstructions::Fixed(ALPHA_INSTRUCTIONS),
            behavior: AgentBehavior {
                turn_timing: TurnTiming::Off,
                workspace_instructions: WorkspaceInstructions::ProjectAgentsFile,
                automatic_compaction: None,
            },
            documents: None,
            document_defaults: None,
            provider_restriction: None,
            tool_baseline: PresetToolBaseline::WorkspacePlus(capabilities::ALPHA_EXTENSIONS),
        },
        AgentPreset {
            id: preset_id(ARCEE_PRESET_ID),
            description: "Renoa's personal operator: curation-owned instructions with SOUL and USER documents, Host turn timing, automatic compaction, and the OpenCode Go provider.",
            instructions: PresetInstructions::Fixed(ARCEE_INSTRUCTIONS),
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
            document_defaults: Some(DocumentDefaults {
                soul: ARCEE_SOUL,
                user: ARCEE_USER,
            }),
            provider_restriction: Some(ModelProvider::OpenCodeGo),
            tool_baseline: PresetToolBaseline::WorkspacePlus(capabilities::ARCEE_EXTENSIONS),
        },
        AgentPreset {
            id: preset_id(SPECIALIST_PRESET_ID),
            description: "A caller-defined specialist: you supply its instructions, and it starts with only the capabilities you select.",
            instructions: PresetInstructions::CallerSupplied,
            behavior: AgentBehavior {
                turn_timing: TurnTiming::HostClock,
                workspace_instructions: WorkspaceInstructions::Off,
                automatic_compaction: None,
            },
            documents: None,
            document_defaults: None,
            provider_restriction: None,
            tool_baseline: PresetToolBaseline::CallerPlus(capabilities::SPECIALIST_EXTENSIONS),
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
    use super::{ALPHA_PRESET_ID, ARCEE_PRESET_ID, SPECIALIST_PRESET_ID, preset};
    use crate::{AgentDefinitionError, AgentPresetId};

    #[test]
    fn every_static_preset_id_resolves() {
        for id in [ALPHA_PRESET_ID, ARCEE_PRESET_ID, SPECIALIST_PRESET_ID] {
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
    fn fixed_instruction_presets_reject_overrides_and_caller_presets_require_them() {
        let alpha = AgentPresetId::new(ALPHA_PRESET_ID).expect("portable preset id");
        let alpha = preset(&alpha).expect("registered preset");
        assert!(alpha.instructions(None).is_ok());
        assert!(matches!(
            alpha.instructions(Some("replace the curated prompt")),
            Err(AgentDefinitionError::InstructionsNotAllowed { .. })
        ));

        let specialist = AgentPresetId::new(SPECIALIST_PRESET_ID).expect("portable preset id");
        let specialist = preset(&specialist).expect("registered preset");
        assert_eq!(
            specialist
                .instructions(Some("Do the job."))
                .expect("caller text"),
            "Do the job."
        );
        assert!(matches!(
            specialist.instructions(None),
            Err(AgentDefinitionError::InstructionsRequired { .. })
        ));
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
        assert!(preset.document_defaults().is_some());
    }

    #[test]
    fn the_specialist_preset_defers_instructions_to_the_caller() {
        let id = AgentPresetId::new(SPECIALIST_PRESET_ID).expect("portable preset id");
        let preset = preset(&id).expect("registered preset");
        assert!(preset.behavior().uses_turn_timing());
        assert!(!preset.behavior().loads_project_instructions());
        assert!(preset.documents().is_none());
        assert_eq!(preset.provider_restriction(), None);
    }
}
