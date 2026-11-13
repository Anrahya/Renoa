use std::collections::BTreeSet;

use super::{
    ALPHA_PRESET_ID, APP_PRESET_ID, ARCEE_PRESET_ID, GENERAL_PRESET_ID, PRESETS,
    SHIPPED_SEED_INSTRUCTION_DIGESTS, is_seed_instructions, preset, seeds, sha256_hex,
};
use crate::{AgentDefinitionError, AgentPresetId};

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
        "f2008bea4848986c0ea821e8cd6f4505370904dee21417825db8f68735a9be73",
    ),
    (
        ARCEE_PRESET_ID,
        "6bc02c9e1151d06b4b53bb41e9155d0d2672084bcf086342b3af41aad30fb066",
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
    sha256_hex(encoded.as_bytes())
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

/// A preset that keeps USER.md draws its line the way `agent_documents`
/// does: what is unlikely to change soon there, anything else worth
/// knowing in the notebook.
#[test]
fn a_preset_with_user_md_sends_everything_else_to_memory_note() {
    for preset in super::catalog() {
        let seed = preset.seed();
        if !seed.documents.is_some_and(|documents| documents.user) {
            continue;
        }
        let id = preset.id().as_str();
        for owned in [
            "unlikely to change soon",
            "anything else worth knowing next time with memory_note",
        ] {
            assert!(seed.instructions.contains(owned), "{id}: {owned}");
        }
        for retired in [
            "goals, commitments",
            "schedule information",
            "rarely change",
        ] {
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

#[test]
fn every_active_preset_seed_is_in_shipped_digests_without_duplicates() {
    let unique: BTreeSet<&str> = SHIPPED_SEED_INSTRUCTION_DIGESTS.iter().copied().collect();
    assert_eq!(
        unique.len(),
        SHIPPED_SEED_INSTRUCTION_DIGESTS.len(),
        "duplicate digest in SHIPPED_SEED_INSTRUCTION_DIGESTS"
    );
    for entry in super::catalog() {
        let digest = sha256_hex(entry.seed().instructions.as_bytes());
        assert!(
            unique.contains(digest.as_str()),
            "preset {} seed digest {digest} is missing from SHIPPED_SEED_INSTRUCTION_DIGESTS",
            entry.id()
        );
        assert!(is_seed_instructions(entry.seed().instructions));
    }
}

#[test]
fn seed_recognition_distinguishes_active_and_retired_seeds_from_custom_text() {
    let current_app = preset(&AgentPresetId::new(APP_PRESET_ID).expect("preset id"))
        .expect("preset")
        .seed()
        .instructions;
    assert!(is_seed_instructions(current_app));
    assert!(!is_seed_instructions("Custom job text."));

    let current_alpha = preset(&AgentPresetId::new(ALPHA_PRESET_ID).expect("preset id"))
        .expect("preset")
        .seed()
        .instructions;
    assert!(is_seed_instructions(current_alpha));
    assert!(!is_seed_instructions("Custom alpha text."));
}
