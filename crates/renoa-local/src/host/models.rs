use super::{HostConfig, LocalHostError};
use crate::{ModelChoice, discover_models};

pub(crate) async fn discover_enabled_models(
    host: &HostConfig,
) -> Result<Vec<ModelChoice>, LocalHostError> {
    let mut models = Vec::new();
    for provider in &host.providers {
        models.extend(
            discover_models(
                host.bridge.clone(),
                *provider,
                host.credential_store.clone(),
            )
            .await?,
        );
    }
    Ok(models)
}
