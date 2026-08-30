use std::collections::BTreeMap;

use super::LocalHostError;
use crate::{AgentProfile, AgentProfileId};

pub(super) fn collect_profiles(
    profiles: Vec<AgentProfile>,
) -> Result<BTreeMap<AgentProfileId, AgentProfile>, LocalHostError> {
    if profiles.is_empty() {
        return Err(LocalHostError::Configuration(
            "at least one agent profile must be registered".to_owned(),
        ));
    }
    let mut registered = BTreeMap::new();
    for profile in profiles {
        let id = profile.id().clone();
        if registered.insert(id.clone(), profile).is_some() {
            return Err(LocalHostError::Configuration(format!(
                "agent profile `{id}` is registered more than once"
            )));
        }
    }
    Ok(registered)
}

#[cfg(test)]
mod tests {
    use super::collect_profiles;
    use crate::{AgentProfile, LocalHostError};

    #[test]
    fn host_requires_one_unique_profile_identity() {
        assert!(matches!(
            collect_profiles(Vec::new()),
            Err(LocalHostError::Configuration(_))
        ));
        let first = AgentProfile::new("renoa.test.v1", "First.").expect("valid profile");
        let duplicate = AgentProfile::new("renoa.test.v1", "Second.").expect("valid profile");
        assert!(matches!(
            collect_profiles(vec![first, duplicate]),
            Err(LocalHostError::Configuration(_))
        ));
    }
}
