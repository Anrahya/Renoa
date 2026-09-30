//! The Host's selectable native capability catalog.
//!
//! This catalog owns each exact runtime name. Versioned presets select explicit
//! variants, so adding a capability cannot silently change an existing preset.
//! Agent definitions persist the resolved names consumed by the runtime and
//! frozen by the kernel.

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
            pub(crate) const fn name(self) -> &'static str {
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
}

pub(crate) const PLUGIN_MANAGE: &str = "plugin_manage";
pub(crate) const AGENT_MANAGE: &str = "agent_manage";
pub(crate) const AUTOMATION_MANAGE: &str = "automation_manage";
pub(crate) const AUTOMATION_RESULTS: &str = "automation_results";
pub(crate) const PLUGIN_SEARCH: &str = "plugin_search";
pub(crate) const TOOL_EXECUTE: &str = "tool_execute";
pub(crate) const SKILL_SEARCH: &str = "skill_search";
pub(crate) const SKILL_LOAD: &str = "skill_load";
pub(crate) const AGENT_DOCUMENTS: &str = "agent_documents";

fn catalog() -> impl Iterator<Item = BuiltInCapability> {
    BUILT_IN_CAPABILITIES.iter().copied()
}

/// Whether one name is in the Host-native capability catalog.
#[must_use]
pub(crate) fn is_selectable(name: &str) -> bool {
    catalog().any(|capability| capability.name() == name)
}

/// Every selectable capability name, in catalog order.
#[must_use]
pub(crate) fn selectable_names() -> Vec<&'static str> {
    catalog().map(BuiltInCapability::name).collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{BuiltInCapability, catalog, is_selectable};

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
        assert!(!is_selectable(super::AGENT_MANAGE));
        assert!(!is_selectable(super::AUTOMATION_MANAGE));
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
