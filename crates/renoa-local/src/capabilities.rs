//! The Host's selectable native capability catalog.
//!
//! This catalog owns each exact runtime name. Versioned presets select explicit
//! variants, so adding a capability cannot silently change an existing preset.
//! Agent definitions persist the resolved names consumed by the runtime and
//! frozen by the kernel.

use std::collections::BTreeSet;

// One declaration generates both the closed type and its complete iterable
// catalog, so a new variant cannot become an unlisted selectable capability.
macro_rules! define_built_in_capabilities {
    ($($variant:ident => $name:literal),+ $(,)?) => {
        /// One selectable native tool capability.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub(crate) enum BuiltInCapability {
            $($variant),+
        }

        impl BuiltInCapability {
            /// Exact identity stored in an agent definition and bound at runtime.
            #[must_use]
            const fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $name),+
                }
            }
        }

        const BUILT_IN_CAPABILITIES: &[BuiltInCapability] = &[
            $(BuiltInCapability::$variant),+
        ];
    };
}

define_built_in_capabilities! {
    ReadFile => "read_file",
    EditFile => "edit_file",
    WriteFile => "write_file",
    Bash => "bash",
    Grep => "grep",
    Find => "find",
    GitChanges => "git_changes",
    GitDiff => "git_diff",
    GitShow => "git_show",
    PluginManage => "plugin_manage",
    AgentManage => "agent_manage",
    RoutineManage => "routine_manage",
    RoutineResults => "routine_results",
    PluginSearch => "plugin_search",
    ToolLoad => "tool_load",
    ToolExecute => "tool_execute",
    CodeMode => "code_mode",
    SkillSearch => "skill_search",
    SkillLoad => "skill_load",
    AgentDocuments => "agent_documents",
}

pub(crate) const PLUGIN_MANAGE: &str = BuiltInCapability::PluginManage.name();
pub(crate) const AGENT_MANAGE: &str = BuiltInCapability::AgentManage.name();
pub(crate) const ROUTINE_MANAGE: &str = BuiltInCapability::RoutineManage.name();
pub(crate) const ROUTINE_RESULTS: &str = BuiltInCapability::RoutineResults.name();
pub(crate) const PLUGIN_SEARCH: &str = BuiltInCapability::PluginSearch.name();
pub(crate) const TOOL_LOAD: &str = BuiltInCapability::ToolLoad.name();
pub(crate) const TOOL_EXECUTE: &str = BuiltInCapability::ToolExecute.name();
pub(crate) const CODE_MODE: &str = BuiltInCapability::CodeMode.name();
pub(crate) const SKILL_SEARCH: &str = BuiltInCapability::SkillSearch.name();
pub(crate) const SKILL_LOAD: &str = BuiltInCapability::SkillLoad.name();
pub(crate) const AGENT_DOCUMENTS: &str = BuiltInCapability::AgentDocuments.name();

fn catalog() -> impl Iterator<Item = BuiltInCapability> {
    BUILT_IN_CAPABILITIES.iter().copied()
}

/// Whether one name is in the Host-native capability catalog.
#[must_use]
pub(crate) fn is_selectable(name: &str) -> bool {
    catalog().any(|capability| capability.name() == name)
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

/// Every selectable capability name, in catalog order.
#[must_use]
pub(crate) fn selectable_names() -> Vec<&'static str> {
    catalog().map(BuiltInCapability::name).collect()
}

/// Expands a preset's pinned components and caller selection into runtime names.
#[must_use]
pub(crate) fn baseline_selection(
    baseline: &[BuiltInCapability],
    caller: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut selection = caller.clone();
    selection.extend(
        baseline
            .iter()
            .map(|capability| capability.name().to_owned()),
    );
    selection
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{AGENT_MANAGE, BuiltInCapability, ROUTINE_MANAGE, catalog, is_selectable};

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct ExpectedCapability {
        capability: BuiltInCapability,
        name: &'static str,
    }

    const EXPECTED_CAPABILITIES: &[ExpectedCapability] = &[
        expected(BuiltInCapability::ReadFile, "read_file"),
        expected(BuiltInCapability::EditFile, "edit_file"),
        expected(BuiltInCapability::WriteFile, "write_file"),
        expected(BuiltInCapability::Bash, "bash"),
        expected(BuiltInCapability::Grep, "grep"),
        expected(BuiltInCapability::Find, "find"),
        expected(BuiltInCapability::GitChanges, "git_changes"),
        expected(BuiltInCapability::GitDiff, "git_diff"),
        expected(BuiltInCapability::GitShow, "git_show"),
        expected(BuiltInCapability::PluginManage, "plugin_manage"),
        expected(BuiltInCapability::AgentManage, "agent_manage"),
        expected(BuiltInCapability::RoutineManage, "routine_manage"),
        expected(BuiltInCapability::RoutineResults, "routine_results"),
        expected(BuiltInCapability::PluginSearch, "plugin_search"),
        expected(BuiltInCapability::ToolLoad, "tool_load"),
        expected(BuiltInCapability::ToolExecute, "tool_execute"),
        expected(BuiltInCapability::CodeMode, "code_mode"),
        expected(BuiltInCapability::SkillSearch, "skill_search"),
        expected(BuiltInCapability::SkillLoad, "skill_load"),
        expected(BuiltInCapability::AgentDocuments, "agent_documents"),
    ];

    const fn expected(capability: BuiltInCapability, name: &'static str) -> ExpectedCapability {
        ExpectedCapability { capability, name }
    }

    #[test]
    fn workspace_runtime_has_the_exact_native_capability_bindings() {
        let directory = tempfile::tempdir().expect("temporary workspace");
        let workspace = crate::LocalWorkspace::open(directory.path()).expect("open workspace");
        let bound: BTreeSet<String> = workspace
            .kernel_tool_bindings()
            .into_iter()
            .map(|binding| binding.tool_name().to_owned())
            .collect();
        let expected: BTreeSet<String> = [
            "read_file",
            "edit_file",
            "write_file",
            "bash",
            "grep",
            "find",
            "git_changes",
            "git_diff",
            "git_show",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        assert_eq!(bound, expected);
    }

    #[test]
    fn selection_accepts_known_names_and_rejects_names_outside_the_catalog() {
        assert!(is_selectable(AGENT_MANAGE));
        assert!(is_selectable(ROUTINE_MANAGE));
        assert!(is_selectable("bash"));
        assert!(!is_selectable("renoa.bot.manage"));
        assert!(!is_selectable("all"));
    }

    #[test]
    fn native_capabilities_have_the_exact_names() {
        let actual: Vec<ExpectedCapability> = catalog()
            .map(|capability| expected(capability, capability.name()))
            .collect();
        assert_eq!(actual, EXPECTED_CAPABILITIES);

        let unique_names: BTreeSet<&str> = catalog().map(BuiltInCapability::name).collect();
        assert_eq!(unique_names.len(), EXPECTED_CAPABILITIES.len());
    }
}
