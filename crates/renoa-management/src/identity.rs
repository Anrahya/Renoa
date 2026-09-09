use std::{net::SocketAddr, time::Duration};

use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use renoa_protocol::PrincipalId;
use serde::Deserialize;

use crate::ManagementError;

/// Calls the configured local identity service; never reads its private database.
pub(super) struct IdentityClient {
    client: reqwest::Client,
    endpoint: String,
}

pub(super) struct Authenticated {
    pub principal: PrincipalId,
    pub renewal: Option<HeaderValue>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Identity {
    principal_id: PrincipalId,
}

impl IdentityClient {
    pub fn new(address: SocketAddr) -> Result<Self, ManagementError> {
        if !address.ip().is_loopback() {
            return Err(ManagementError::PublicIdentity);
        }
        Ok(Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(10))
                .build()?,
            endpoint: format!("http://{address}/v1/identity/session"),
        })
    }

    pub async fn authenticate(
        &self,
        headers: &HeaderMap,
    ) -> Result<Option<Authenticated>, ManagementError> {
        if !headers.contains_key(header::COOKIE) {
            return Ok(None);
        }
        let mut request = self.client.get(&self.endpoint);
        for cookie in headers.get_all(header::COOKIE) {
            request = request.header(header::COOKIE, cookie);
        }
        let response = request.send().await?;
        if response.status() == StatusCode::UNAUTHORIZED {
            return Ok(None);
        }
        if response.status() != StatusCode::OK {
            return Err(ManagementError::IdentityUnavailable);
        }
        let renewal = response.headers().get(header::SET_COOKIE).cloned();
        let identity: Identity = response.json().await?;
        Ok(Some(Authenticated {
            principal: identity.principal_id,
            renewal,
        }))
    }
}
