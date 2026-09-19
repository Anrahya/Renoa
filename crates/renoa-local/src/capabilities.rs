//! The Host capability inventory and preset tool baselines.
//!
//! Every selectable capability has exactly one name, defined here. A runtime
//! binds a capability only when the agent's exact tool selection contains its
//! name, so no policy is inferred from identities, prefixes, or preset ids.

use std::collections::BTreeSet;

/// Host-owned workspace tools. A test proves this matches what
/// [`crate::LocalWorkspace`] actually binds.
pub(crate) const WORKSPACE_TOOL_NAMES: &[&str] = &[
    "read_file",
    "edit_file",
    "write_file",
    "bash",
    "grep",
    "find",
    "git_changes",
    "git_diff",
    "git_show",
];

pub(crate) const EXTENSION_MANAGE: &str = "extension_manage";
pub(crate) const AGENT_MANAGE: &str = "agent_manage";
pub(crate) const ROUTINE_MANAGE: &str = "routine_manage";
pub(crate) const ROUTINE_RESULTS: &str = "routine_results";
pub(crate) const TOOL_SEARCH: &str = "tool_search";
pub(crate) const TOOL_LOAD: &str = "tool_load";
pub(crate) const TOOL_EXECUTE: &str = "tool_execute";
pub(crate) const SKILL_SEARCH: &str = "skill_search";
pub(crate) const SKILL_LOAD: &str = "skill_load";
pub(crate) const AGENT_DOCUMENTS: &str = "agent_documents";

/// Extension capabilities the Alpha preset seeds.
pub(crate) const ALPHA_EXTENSIONS: &[&str] = &[
    EXTENSION_MANAGE,
    TOOL_SEARCH,
    TOOL_LOAD,
    TOOL_EXECUTE,
    SKILL_SEARCH,
    SKILL_LOAD,
];

/// Extension capabilities the Arcee preset seeds.
pub(crate) const ARCEE_EXTENSIONS: &[&str] = &[
    EXTENSION_MANAGE,
    AGENT_MANAGE,
    ROUTINE_MANAGE,
    ROUTINE_RESULTS,
    TOOL_SEARCH,
    TOOL_LOAD,
    TOOL_EXECUTE,
    SKILL_SEARCH,
    SKILL_LOAD,
    AGENT_DOCUMENTS,
];

/// Extension capabilities every caller-defined specialist keeps, matching the
/// capability set specialists received before tool selection became explicit.
pub(crate) const SPECIALIST_EXTENSIONS: &[&str] = &[
    ROUTINE_MANAGE,
    ROUTINE_RESULTS,
    TOOL_SEARCH,
    TOOL_LOAD,
    TOOL_EXECUTE,
    SKILL_SEARCH,
    SKILL_LOAD,
];

/// Whether one name is a Host capability.
#[must_use]
pub(crate) fn is_selectable(name: &str) -> bool {
    WORKSPACE_TOOL_NAMES.contains(&name) || extension_names().contains(&name)
}

/// Whether one definition can exercise a selectable capability.
///
/// The document capability edits the agent's own prompt files, so a definition
/// without documents cannot consume it: the runtime would drop the binding
/// without a trace while the stored selection still named it.
#[must_use]
pub(crate) fn is_consumable(name: &str, documents: Option<crate::AgentDocuments>) -> bool {
    name != AGENT_DOCUMENTS || documents.is_some()
}

/// Every selectable capability name, for callers that enumerate the vocabulary.
#[must_use]
pub(crate) fn selectable_names() -> Vec<&'static str> {
    let mut names = WORKSPACE_TOOL_NAMES.to_vec();
    names.extend_from_slice(extension_names());
    names
}

#[must_use]
pub(crate) fn workspace_tool_names() -> BTreeSet<String> {
    WORKSPACE_TOOL_NAMES
        .iter()
        .map(|name| (*name).to_owned())
        .collect()
}

fn extension_names() -> &'static [&'static str] {
    &[
        EXTENSION_MANAGE,
        AGENT_MANAGE,
        ROUTINE_MANAGE,
        ROUTINE_RESULTS,
        TOOL_SEARCH,
        TOOL_LOAD,
        TOOL_EXECUTE,
        SKILL_SEARCH,
        SKILL_LOAD,
        AGENT_DOCUMENTS,
    ]
}

/// How a preset expands into an exact stored tool selection.
#[derive(Clone, Copy)]
pub(crate) enum PresetToolBaseline {
    /// Every Host workspace tool plus these extension names.
    WorkspacePlus(&'static [&'static str]),
    /// Only the caller's exact names plus these extension names.
    CallerPlus(&'static [&'static str]),
}

/// Expands a preset baseline and caller selection into exact capability names.
#[must_use]
pub(crate) fn baseline_selection(
    baseline: PresetToolBaseline,
    caller: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut selection = caller.clone();
    match baseline {
        PresetToolBaseline::WorkspacePlus(extensions) => {
            selection.extend(workspace_tool_names());
            selection.extend(extensions.iter().map(|name| (*name).to_owned()));
        }
        PresetToolBaseline::CallerPlus(extensions) => {
            selection.extend(extensions.iter().map(|name| (*name).to_owned()));
        }
    }
    selection
}

#[cfg(test)]
mod tests {
    use super::{
        AGENT_MANAGE, ARCEE_EXTENSIONS, ROUTINE_MANAGE, WORKSPACE_TOOL_NAMES, is_selectable,
    };

    #[test]
    fn the_declared_workspace_names_match_the_real_workspace_bindings() {
        let directory = tempfile::tempdir().expect("temporary workspace");
        let workspace = crate::LocalWorkspace::open(directory.path()).expect("open workspace");
        let bound: Vec<String> = workspace
            .kernel_tool_bindings()
            .into_iter()
            .map(|binding| binding.tool_name().to_owned())
            .collect();
        for name in WORKSPACE_TOOL_NAMES {
            assert!(
                bound.contains(&(*name).to_owned()),
                "declared workspace tool `{name}` is not bound by LocalWorkspace: {bound:?}"
            );
        }
        assert_eq!(
            bound.len(),
            WORKSPACE_TOOL_NAMES.len(),
            "LocalWorkspace binds a tool this inventory does not declare: {bound:?}"
        );
    }

    #[test]
    fn the_inventory_is_exact_and_closed() {
        assert!(is_selectable(AGENT_MANAGE));
        assert!(is_selectable(ROUTINE_MANAGE));
        assert!(is_selectable("bash"));
        assert!(!is_selectable("renoa.bot.manage"));
        assert!(!is_selectable("all"));
        for extension in ARCEE_EXTENSIONS {
            assert!(is_selectable(extension), "preset names must be selectable");
        }
    }
}
