use crate::{PluginCredentialKind, PluginCredentialRequired, PluginProgress};
use renoa_agent::{ContentBlock, ToolOutput, ToolUpdates};

pub(super) async fn emit_required(
    updates: &ToolUpdates,
    credential_id: &str,
    credential_kind: PluginCredentialKind,
    setup_url: &str,
    expires_at_ms: i64,
) {
    let update = PluginProgress::CredentialRequired(PluginCredentialRequired {
        credential: credential_id.to_owned(),
        credential_kind,
        setup_url: setup_url.to_owned(),
        expires_at_ms,
        message: "Open the secure setup link. The credential is encrypted in the browser and saved only by the requesting Host.".to_owned(),
    });
    if let Ok(encoded) = serde_json::to_string(&update) {
        updates
            .emit(ToolOutput {
                content: vec![ContentBlock::text(encoded)],
                details: None,
                is_error: false,
            })
            .await;
    }
}
