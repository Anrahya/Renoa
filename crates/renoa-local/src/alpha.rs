use crate::profile::AgentProfile;

const BASE_PROMPT: &str = include_str!("../prompts/alpha-v1.md");

/// Stable Host identity of Renoa's first local coding profile.
pub const ALPHA_PROFILE_ID: &str = "renoa.coding.alpha.v1";

#[must_use]
/// Returns Renoa's built-in coding profile.
///
/// # Panics
///
/// Panics only if the source-controlled Alpha identity or prompt is changed to
/// violate [`AgentProfile`] validation.
pub fn alpha_profile() -> AgentProfile {
    AgentProfile::new(ALPHA_PROFILE_ID, BASE_PROMPT)
        .expect("the built-in Alpha profile is statically valid")
        .with_workspace_instructions()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::alpha_profile;

    #[test]
    fn alpha_keeps_its_curated_prompt_and_workspace_rules() {
        let directory = tempdir().expect("temporary directory");
        fs::write(
            directory.path().join("AGENTS.md"),
            "Keep the public API small.\n",
        )
        .expect("write project instructions");

        let prompt = alpha_profile()
            .system_prompt(directory.path())
            .expect("compose Alpha prompt");

        assert!(prompt.starts_with("You are Alpha, Renoa's local coding agent."));
        assert!(prompt.contains("Keep the public API small."));
        assert!(!prompt.contains("read_file"));
        assert!(!prompt.contains("config_digest"));
        assert!(!prompt.contains(directory.path().to_string_lossy().as_ref()));
    }
}
