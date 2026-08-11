use serde::{Deserialize, Serialize};

/// Harness configuration frozen by Renoa's reference runtime.
///
/// This is not part of RCP. Other harnesses define and persist their own
/// configuration behind their node adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedAgent {
    pub instructions: String,
    pub capability_grants: Vec<String>,
}
