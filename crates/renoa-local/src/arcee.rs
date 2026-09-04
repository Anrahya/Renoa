use std::{num::NonZeroU64, path::Path};

use crate::{
    AgentProfile, AgentProfileError, ModelProvider,
    profile::{ProfileDocumentDefaults, ProfileDocuments},
};

const SYSTEM_PROMPT: &str = include_str!("../prompts/arcee-v1/system.md");
const DEFAULT_SOUL: &str = include_str!("../prompts/arcee-v1/SOUL.md");
const DEFAULT_USER: &str = include_str!("../prompts/arcee-v1/USER.md");
const AUTOMATIC_COMPACTION_INPUT_TOKENS: NonZeroU64 = NonZeroU64::new(400_000).unwrap();
const POST_COMPACTION_INPUT_TOKENS: NonZeroU64 = NonZeroU64::new(40_000).unwrap();

/// Stable Host identity of Renoa's personal operator profile.
pub const ARCEE_PROFILE_ID: &str = "renoa.personal.arcee.v1";

/// Creates Arcee's built-in profile and seeds its owner-editable documents.
///
/// Existing `SOUL.md` and `USER.md` files are preserved. Renoa reads both
/// files again for every newly admitted turn.
///
/// # Errors
///
/// Returns invalid profile data, unsafe paths, or profile-document storage failures.
pub fn arcee_profile(data_directory: impl AsRef<Path>) -> Result<AgentProfile, AgentProfileError> {
    let id = crate::AgentProfileId::new(ARCEE_PROFILE_ID)?;
    let documents = ProfileDocuments::initialize(
        data_directory.as_ref(),
        &id,
        ProfileDocumentDefaults {
            soul: DEFAULT_SOUL,
            user: DEFAULT_USER,
        },
    )?;
    Ok(AgentProfile::new(ARCEE_PROFILE_ID, SYSTEM_PROMPT)?
        .with_documents(documents)
        .with_model_provider(ModelProvider::OpenCodeGo)
        .with_turn_timing()
        .with_automatic_compaction(
            AUTOMATIC_COMPACTION_INPUT_TOKENS,
            POST_COMPACTION_INPUT_TOKENS,
        )
        .with_workspace_instructions())
}

#[cfg(test)]
mod tests {
    use std::{fs, num::NonZeroU64};

    use tempfile::tempdir;

    use super::{ARCEE_PROFILE_ID, arcee_profile};
    use crate::ModelProvider;

    #[test]
    fn arcee_composes_system_soul_user_and_workspace_context() {
        let directory = tempdir().expect("temporary directory");
        let workspace = directory.path().join("workspace");
        fs::create_dir(&workspace).expect("create workspace");
        fs::write(
            workspace.join("AGENTS.md"),
            "Keep production data intact.\n",
        )
        .expect("write workspace instructions");
        let profile = arcee_profile(directory.path()).expect("create Arcee profile");

        let prompt = profile
            .system_prompt(&workspace)
            .expect("compose Arcee prompt");

        assert_eq!(profile.id().as_str(), ARCEE_PROFILE_ID);
        assert_eq!(profile.model_provider(), Some(ModelProvider::OpenCodeGo));
        assert!(profile.uses_turn_timing());
        let automatic_compaction = profile
            .automatic_compaction()
            .expect("Arcee automatic compaction policy");
        assert_eq!(
            automatic_compaction.trigger_input_tokens,
            NonZeroU64::new(400_000).expect("non-zero trigger")
        );
        assert_eq!(
            automatic_compaction.target_input_tokens,
            NonZeroU64::new(40_000).expect("non-zero target")
        );
        assert!(prompt.starts_with("You are Arcee, Renoa's personal operator."));
        assert!(prompt.contains("<soul source=\"SOUL.md\" revision=\""));
        assert!(prompt.contains("<user_profile source=\"USER.md\" revision=\""));
        assert!(prompt.contains("Do not open with praise, validation, or agreement."));
        assert!(
            prompt.contains("Before installing an extension, reuse a matching enabled connection")
        );
        assert!(prompt.contains("Keep production data intact."));
        assert!(!prompt.contains(workspace.to_string_lossy().as_ref()));
    }

    #[test]
    fn arcee_reloads_profile_documents_for_the_next_turn() {
        let directory = tempdir().expect("temporary directory");
        let workspace = directory.path().join("workspace");
        fs::create_dir(&workspace).expect("create workspace");
        let profile = arcee_profile(directory.path()).expect("create Arcee profile");
        let before = profile
            .system_prompt(&workspace)
            .expect("compose initial prompt");
        let user = directory
            .path()
            .join("profiles")
            .join(ARCEE_PROFILE_ID)
            .join("USER.md");
        fs::write(&user, "The user works at night.\n").expect("update user profile");

        let after = profile
            .system_prompt(&workspace)
            .expect("compose updated prompt");

        assert_ne!(before, after);
        assert!(after.contains("The user works at night."));
    }
}
