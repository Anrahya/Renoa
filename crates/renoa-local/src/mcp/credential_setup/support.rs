use renoa_agent::{ContentBlock, ToolOutput, ToolUpdates};
use serde::Serialize;

#[derive(Serialize)]
struct CredentialUpdate<'a> {
    status: &'static str,
    credential: &'a str,
    credential_kind: &'static str,
    setup_url: &'a str,
    expires_at_ms: i64,
    message: &'static str,
}

pub(super) async fn emit_required(
    updates: &ToolUpdates,
    credential_id: &str,
    kind: &'static str,
    setup_url: &str,
    expires_at_ms: i64,
) {
    let update = CredentialUpdate {
        status: "credential_required",
        credential: credential_id,
        credential_kind: kind,
        setup_url,
        expires_at_ms,
        message: "Open the secure setup link. The credential is encrypted in the browser and saved only by the requesting Host.",
    };
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
