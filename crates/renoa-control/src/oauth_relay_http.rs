use std::{str::FromStr as _, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::{
        DefaultBodyLimit, FromRequest, Path, RawQuery, Request, State, rejection::JsonRejection,
    },
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use renoa_oauth_relay_protocol::{
    AcknowledgeOAuthRelayRequest, AcknowledgeOAuthRelayResponse, CreateOAuthRelayRequest,
    CreateOAuthRelayResponse, DEVICE_ID_HEADER, OAUTH_CALLBACK_PATH, OAUTH_RELAY_VERSION,
    OAUTH_RELAYS_PATH, OAuthRelayErrorResponse, OAuthRelayId,
};
use sha2::{Digest as _, Sha256};
use url::Url;

use crate::{
    DeviceCredential, DeviceCredentials, DeviceId, PeerIdentity,
    coordinator::{ControlErrorKind, CoordinatorState},
    oauth_relay_store::{OAuthCallbackAdmission, OAuthCallbackResult},
};

const RELAY_LIFETIME: Duration = Duration::from_mins(10);
const MAX_RELAY_BODY_BYTES: usize = 16 * 1024;
const MAX_CALLBACK_QUERY_BYTES: usize = 32 * 1024;
const MAX_CALLBACK_VALUE_BYTES: usize = 16 * 1024;
const CLIENT_METADATA_PATH: &str = "/v1/oauth/client-metadata.json";

pub(crate) fn routes() -> Router<Arc<CoordinatorState>> {
    Router::new()
        .route(OAUTH_RELAYS_PATH, post(create_relay))
        .route(
            &format!("{OAUTH_RELAYS_PATH}/{{relay_id}}"),
            get(relay_status),
        )
        .route(
            &format!("{OAUTH_RELAYS_PATH}/{{relay_id}}/acknowledge"),
            post(acknowledge_relay),
        )
        .route(OAUTH_CALLBACK_PATH, get(provider_callback))
        .route(CLIENT_METADATA_PATH, get(client_metadata))
        .layer(DefaultBodyLimit::max(MAX_RELAY_BODY_BYTES))
}

async fn client_metadata(State(state): State<Arc<CoordinatorState>>) -> Response {
    let Some(redirect_uri) = state.oauth_callback_uri.as_deref() else {
        return internal_error();
    };
    let Ok(callback) = Url::parse(redirect_uri) else {
        return internal_error();
    };
    let Ok(client_id) = callback.join(CLIENT_METADATA_PATH) else {
        return internal_error();
    };
    let client_uri = callback.origin().ascii_serialization();
    secure_json(
        StatusCode::OK,
        &serde_json::json!({
            "client_id": client_id,
            "redirect_uris": [redirect_uri],
            "token_endpoint_auth_method": "none",
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "application_type": "web",
            "client_name": "Renoa",
            "client_uri": client_uri,
            "software_id": "renoa",
            "software_version": env!("CARGO_PKG_VERSION")
        }),
    )
}

async fn create_relay(State(state): State<Arc<CoordinatorState>>, request: Request) -> Response {
    let device_id = match authenticate_node(&state, request.headers()).await {
        Ok(device_id) => device_id,
        Err(response) => return response,
    };
    let request = match json_request::<CreateOAuthRelayRequest>(request, &state).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    if request.version != OAUTH_RELAY_VERSION || !valid_digest(&request.state_digest) {
        return invalid_request();
    }
    let Some(expires_at) = std::time::SystemTime::now().checked_add(RELAY_LIFETIME) else {
        return internal_error();
    };
    match state
        .store
        .create_oauth_relay(
            device_id,
            request.relay_id,
            request.state_digest,
            expires_at,
        )
        .await
    {
        Ok(relay) => {
            let Some(redirect_uri) = state.oauth_callback_uri.as_deref() else {
                return internal_error();
            };
            secure_json(
                StatusCode::OK,
                &CreateOAuthRelayResponse {
                    version: OAUTH_RELAY_VERSION,
                    relay_id: relay.relay_id,
                    redirect_uri: redirect_uri.to_owned(),
                    expires_at_ms: relay.expires_at_ms,
                },
            )
        }
        Err(error) => relay_error(&error),
    }
}

async fn relay_status(
    State(state): State<Arc<CoordinatorState>>,
    Path(relay_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let device_id = match authenticate_node(&state, &headers).await {
        Ok(device_id) => device_id,
        Err(response) => return response,
    };
    let Ok(relay_id) = OAuthRelayId::from_str(&relay_id) else {
        return invalid_request();
    };
    match state.store.oauth_relay_status(device_id, relay_id).await {
        Ok(status) => secure_json(StatusCode::OK, &status),
        Err(error) => relay_error(&error),
    }
}

async fn acknowledge_relay(
    State(state): State<Arc<CoordinatorState>>,
    Path(relay_id): Path<String>,
    request: Request,
) -> Response {
    let device_id = match authenticate_node(&state, request.headers()).await {
        Ok(device_id) => device_id,
        Err(response) => return response,
    };
    let Ok(relay_id) = OAuthRelayId::from_str(&relay_id) else {
        return invalid_request();
    };
    let request = match json_request::<AcknowledgeOAuthRelayRequest>(request, &state).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    if request.version != OAUTH_RELAY_VERSION {
        return invalid_request();
    }
    match state
        .store
        .acknowledge_oauth_relay(device_id, relay_id)
        .await
    {
        Ok(()) => secure_json(
            StatusCode::OK,
            &AcknowledgeOAuthRelayResponse {
                version: OAUTH_RELAY_VERSION,
            },
        ),
        Err(error) => relay_error(&error),
    }
}

async fn provider_callback(
    State(state): State<Arc<CoordinatorState>>,
    RawQuery(query): RawQuery,
) -> Response {
    let Some(query) = query.filter(|query| query.len() <= MAX_CALLBACK_QUERY_BYTES) else {
        return callback_failure();
    };
    let Some(callback) = parse_callback(&query) else {
        return callback_failure();
    };
    let state_digest = hex_sha256(callback.state.as_bytes());
    let result = match (&callback.authorization_code, &callback.oauth_error) {
        (Some(code), None) => OAuthCallbackResult::Authorized {
            authorization_code: code,
            issuer: callback.issuer.as_deref(),
        },
        (None, Some(error)) => OAuthCallbackResult::Rejected { error },
        _ => return callback_failure(),
    };
    match state
        .store
        .record_oauth_callback(state_digest, result)
        .await
    {
        Ok(OAuthCallbackAdmission::Authorized) => callback_page(
            StatusCode::OK,
            "Authorization received",
            "Renoa received the authorization. You can close this tab.",
        ),
        Ok(OAuthCallbackAdmission::Rejected) => callback_page(
            StatusCode::OK,
            "Authorization cancelled",
            "Authorization was not completed. You can close this tab.",
        ),
        Err(_) => callback_failure(),
    }
}

struct ProviderCallback {
    state: String,
    authorization_code: Option<String>,
    issuer: Option<String>,
    oauth_error: Option<String>,
}

fn parse_callback(query: &str) -> Option<ProviderCallback> {
    let mut state = None;
    let mut authorization_code = None;
    let mut issuer = None;
    let mut oauth_error = None;
    for (name, value) in url::form_urlencoded::parse(query.as_bytes()) {
        if value.len() > MAX_CALLBACK_VALUE_BYTES
            || value.bytes().any(|byte| byte.is_ascii_control())
        {
            return None;
        }
        let slot = match name.as_ref() {
            "state" => &mut state,
            "code" => &mut authorization_code,
            "iss" => &mut issuer,
            "error" => &mut oauth_error,
            _ => continue,
        };
        if slot.replace(value.into_owned()).is_some() {
            return None;
        }
    }
    let state = state.filter(|value| valid_state(value))?;
    if authorization_code.is_some() == oauth_error.is_some() {
        return None;
    }
    if authorization_code.as_ref().is_some_and(String::is_empty)
        || oauth_error
            .as_deref()
            .is_some_and(|error| !valid_oauth_error(error))
        || issuer.as_deref().is_some_and(|value| !valid_issuer(value))
        || (oauth_error.is_some() && issuer.is_some())
    {
        return None;
    }
    Some(ProviderCallback {
        state,
        authorization_code,
        issuer,
        oauth_error,
    })
}

pub(crate) async fn authenticate_node(
    state: &CoordinatorState,
    headers: &HeaderMap,
) -> Result<DeviceId, Response> {
    let Some(device_id) = exact_header(headers, DEVICE_ID_HEADER)
        .and_then(|value| value.parse().ok())
        .map(DeviceId::from_uuid)
    else {
        return Err(authentication_failed());
    };
    let Some(credential) = exact_header(headers, header::AUTHORIZATION.as_str())
        .and_then(parse_bearer)
        .and_then(|value| DeviceCredential::from_encoded(value.to_owned()))
    else {
        return Err(authentication_failed());
    };
    match state
        .store
        .authenticate_device(DeviceCredentials {
            device_id,
            credential,
        })
        .await
    {
        Ok(authenticated) if matches!(authenticated.peer, PeerIdentity::Node { .. }) => {
            Ok(authenticated.device_id)
        }
        Ok(_) | Err(_) => Err(authentication_failed()),
    }
}

fn exact_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    values.next().is_none().then_some(value)
}

fn parse_bearer(value: &str) -> Option<&str> {
    let (scheme, credential) = value.split_once(' ')?;
    (scheme.eq_ignore_ascii_case("bearer")
        && !credential.is_empty()
        && !credential.bytes().any(|byte| byte.is_ascii_whitespace()))
    .then_some(credential)
}

pub(crate) async fn json_request<T: serde::de::DeserializeOwned>(
    request: Request,
    state: &Arc<CoordinatorState>,
) -> Result<T, Response> {
    Json::<T>::from_request(request, state)
        .await
        .map(|Json(value)| value)
        .map_err(|_error: JsonRejection| invalid_request())
}

fn valid_state(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn valid_digest(value: &str) -> bool {
    valid_state(value)
}

fn valid_oauth_error(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn valid_issuer(value: &str) -> bool {
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    let loopback = url.scheme() == "http"
        && url.host_str().is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost")
                || host
                    .trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|address| address.is_loopback())
        });
    (url.scheme() == "https" || loopback)
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}

fn hex_sha256(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn relay_error(error: &crate::ControlError) -> Response {
    let (status, code, message) = match error.kind() {
        ControlErrorKind::Authentication => (
            StatusCode::UNAUTHORIZED,
            "authentication_failed",
            "authentication failed",
        ),
        ControlErrorKind::Capacity => (
            StatusCode::TOO_MANY_REQUESTS,
            "capacity_exceeded",
            "too many OAuth callback relays are active",
        ),
        ControlErrorKind::Conflict => (StatusCode::CONFLICT, "conflict", "relay conflict"),
        ControlErrorKind::Invalid => (
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "invalid OAuth relay request",
        ),
        ControlErrorKind::NotFound => (
            StatusCode::NOT_FOUND,
            "not_found",
            "OAuth relay was not found",
        ),
        ControlErrorKind::Store => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "OAuth relay service failed",
        ),
    };
    secure_json(
        status,
        &OAuthRelayErrorResponse {
            code: code.to_owned(),
            message: message.to_owned(),
        },
    )
}

fn invalid_request() -> Response {
    secure_json(
        StatusCode::BAD_REQUEST,
        &OAuthRelayErrorResponse {
            code: "invalid_request".to_owned(),
            message: "invalid OAuth relay request".to_owned(),
        },
    )
}

fn authentication_failed() -> Response {
    secure_json(
        StatusCode::UNAUTHORIZED,
        &OAuthRelayErrorResponse {
            code: "authentication_failed".to_owned(),
            message: "authentication failed".to_owned(),
        },
    )
}

fn internal_error() -> Response {
    secure_json(
        StatusCode::INTERNAL_SERVER_ERROR,
        &OAuthRelayErrorResponse {
            code: "internal".to_owned(),
            message: "OAuth relay service failed".to_owned(),
        },
    )
}

fn callback_failure() -> Response {
    callback_page(
        StatusCode::BAD_REQUEST,
        "Authorization not accepted",
        "This authorization link is invalid, expired, or already used differently.",
    )
}

fn callback_page(status: StatusCode, title: &'static str, message: &'static str) -> Response {
    let body = format!(
        "<!doctype html><html lang=en><meta charset=utf-8><meta name=viewport content=\"width=device-width,initial-scale=1\"><title>{title}</title><body><main><h1>{title}</h1><p>{message}</p></main></body></html>"
    );
    let mut response = (status, Html(body)).into_response();
    secure_headers(response.headers_mut());
    response
}

pub(crate) fn secure_json<T: serde::Serialize>(status: StatusCode, value: &T) -> Response {
    let mut response = (status, Json(value)).into_response();
    secure_headers(response.headers_mut());
    response
}

pub(crate) fn secure_headers(headers: &mut HeaderMap) {
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; frame-ancestors 'none'"),
    );
}
