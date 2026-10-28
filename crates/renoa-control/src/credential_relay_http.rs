use std::{
    str::FromStr as _,
    sync::{Arc, LazyLock},
    time::Duration,
};

use axum::{
    Router,
    body::Body,
    extract::{DefaultBodyLimit, Path, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use renoa_credential_relay_protocol::{
    AcknowledgeCredentialRelayRequest, AcknowledgeCredentialRelayResponse,
    CREDENTIAL_RELAY_VERSION, CREDENTIAL_RELAYS_PATH, CREDENTIAL_SETUP_SCRIPT_PATH,
    CREDENTIAL_SETUP_STYLE_PATH, CreateCredentialRelayRequest, CreateCredentialRelayResponse,
    CredentialRelayErrorResponse, CredentialRelayId, SubmitCredentialRelayRequest,
    SubmitCredentialRelayResponse,
};

use crate::{
    ControlError,
    coordinator::{ControlErrorKind, CoordinatorState},
    oauth_relay_http::{authenticate_node, json_request, secure_headers, secure_json},
};

const RELAY_LIFETIME: Duration = Duration::from_mins(15);
const MAX_RELAY_BODY_BYTES: usize = 160 * 1024;
const SETUP_SCRIPT: &str = include_str!("credential_setup.js");
const SETUP_LAYOUT: &str = include_str!("credential_setup.css");
const SETUP_PAGE: &str = include_str!("credential_setup.html");
const MARK_PLACEHOLDER: &str = "<!--renoa-mark-->";

// The design system owns these; the page serves them as they are rather than keeping copies.
const DESIGN_TOKENS: &str = include_str!("../../../design/system/tokens.css");
const DESIGN_COMPONENTS: &str = include_str!("../../../design/system/components/bundle.css");
const DESIGN_MARK: &str = include_str!("../../../design/system/assets/Logos/renoa-mark-ink.svg");

static SETUP_STYLE: LazyLock<String> =
    LazyLock::new(|| format!("{DESIGN_TOKENS}\n{DESIGN_COMPONENTS}\n{SETUP_LAYOUT}"));
static SETUP_HTML: LazyLock<String> =
    LazyLock::new(|| SETUP_PAGE.replace(MARK_PLACEHOLDER, DESIGN_MARK));

pub(crate) fn routes() -> Router<Arc<CoordinatorState>> {
    Router::new()
        .route(CREDENTIAL_RELAYS_PATH, post(create_relay))
        .route(
            &format!("{CREDENTIAL_RELAYS_PATH}/{{relay_id}}"),
            get(relay_status),
        )
        .route(
            &format!("{CREDENTIAL_RELAYS_PATH}/{{relay_id}}/form"),
            get(relay_form),
        )
        .route(
            &format!("{CREDENTIAL_RELAYS_PATH}/{{relay_id}}/setup"),
            get(setup_page),
        )
        .route(
            &format!("{CREDENTIAL_RELAYS_PATH}/{{relay_id}}/submit"),
            post(submit_relay),
        )
        .route(
            &format!("{CREDENTIAL_RELAYS_PATH}/{{relay_id}}/acknowledge"),
            post(acknowledge_relay),
        )
        .route(CREDENTIAL_SETUP_SCRIPT_PATH, get(setup_script))
        .route(CREDENTIAL_SETUP_STYLE_PATH, get(setup_style))
        .layer(DefaultBodyLimit::max(MAX_RELAY_BODY_BYTES))
}

async fn create_relay(State(state): State<Arc<CoordinatorState>>, request: Request) -> Response {
    let device_id = match authenticate_node(&state, request.headers()).await {
        Ok(device_id) => device_id,
        Err(response) => return *response,
    };
    let request = match json_request::<CreateCredentialRelayRequest>(request, &state).await {
        Ok(request) => request,
        Err(response) => return *response,
    };
    if request.version != CREDENTIAL_RELAY_VERSION {
        return invalid_request();
    }
    let Some(expires_at) = std::time::SystemTime::now().checked_add(RELAY_LIFETIME) else {
        return internal_error();
    };
    match state
        .store
        .create_credential_relay(
            device_id,
            request.relay_id,
            request.credential_id,
            request.kind,
            request.capability_digest,
            expires_at,
        )
        .await
    {
        Ok(relay) => secure_json(
            StatusCode::OK,
            &CreateCredentialRelayResponse {
                version: CREDENTIAL_RELAY_VERSION,
                relay_id: relay.relay_id,
                expires_at_ms: relay.expires_at_ms,
            },
        ),
        Err(error) => relay_error(&error),
    }
}

async fn relay_form(
    State(state): State<Arc<CoordinatorState>>,
    Path(relay_id): Path<String>,
) -> Response {
    let Ok(relay_id) = CredentialRelayId::from_str(&relay_id) else {
        return invalid_request();
    };
    match state.store.credential_relay_form(relay_id).await {
        Ok(form) => secure_json(StatusCode::OK, &form),
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
        Err(response) => return *response,
    };
    let Ok(relay_id) = CredentialRelayId::from_str(&relay_id) else {
        return invalid_request();
    };
    match state
        .store
        .credential_relay_status(device_id, relay_id)
        .await
    {
        Ok(status) => secure_json(StatusCode::OK, &status),
        Err(error) => relay_error(&error),
    }
}

async fn submit_relay(
    State(state): State<Arc<CoordinatorState>>,
    Path(relay_id): Path<String>,
    request: Request,
) -> Response {
    let Ok(relay_id) = CredentialRelayId::from_str(&relay_id) else {
        return invalid_request();
    };
    let request = match json_request::<SubmitCredentialRelayRequest>(request, &state).await {
        Ok(request) => request,
        Err(response) => return *response,
    };
    if request.version != CREDENTIAL_RELAY_VERSION {
        return invalid_request();
    }
    match state
        .store
        .submit_credential_relay(
            relay_id,
            request.capability,
            request.nonce,
            request.ciphertext,
        )
        .await
    {
        Ok(()) => secure_json(
            StatusCode::OK,
            &SubmitCredentialRelayResponse {
                version: CREDENTIAL_RELAY_VERSION,
            },
        ),
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
        Err(response) => return *response,
    };
    let Ok(relay_id) = CredentialRelayId::from_str(&relay_id) else {
        return invalid_request();
    };
    let request = match json_request::<AcknowledgeCredentialRelayRequest>(request, &state).await {
        Ok(request) => request,
        Err(response) => return *response,
    };
    if request.version != CREDENTIAL_RELAY_VERSION {
        return invalid_request();
    }
    match state
        .store
        .acknowledge_credential_relay(device_id, relay_id)
        .await
    {
        Ok(()) => secure_json(
            StatusCode::OK,
            &AcknowledgeCredentialRelayResponse {
                version: CREDENTIAL_RELAY_VERSION,
            },
        ),
        Err(error) => relay_error(&error),
    }
}

async fn setup_page(Path(relay_id): Path<String>) -> Response {
    if CredentialRelayId::from_str(&relay_id).is_err() {
        return invalid_request();
    }
    let mut response = (StatusCode::OK, Html(SETUP_HTML.as_str())).into_response();
    page_headers(response.headers_mut());
    response
}

async fn setup_script() -> Response {
    asset(SETUP_SCRIPT, "text/javascript; charset=utf-8")
}

async fn setup_style() -> Response {
    asset(SETUP_STYLE.as_str(), "text/css; charset=utf-8")
}

fn asset(body: &'static str, content_type: &'static str) -> Response {
    let mut response = Response::new(Body::from(body));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    secure_headers(response.headers_mut());
    response
}

fn relay_error(error: &ControlError) -> Response {
    let (status, code, message) = match error.kind() {
        ControlErrorKind::Authentication => (
            StatusCode::UNAUTHORIZED,
            "authentication_failed",
            "authentication failed",
        ),
        ControlErrorKind::Capacity => (
            StatusCode::TOO_MANY_REQUESTS,
            "capacity_exceeded",
            "too many credential relays are active",
        ),
        ControlErrorKind::Conflict => (StatusCode::CONFLICT, "conflict", "relay conflict"),
        ControlErrorKind::Invalid => (
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "invalid credential relay request",
        ),
        ControlErrorKind::NotFound => (
            StatusCode::NOT_FOUND,
            "not_found",
            "credential relay was not found",
        ),
        ControlErrorKind::NodeOffline | ControlErrorKind::Store => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "credential relay service failed",
        ),
    };
    secure_json(
        status,
        &CredentialRelayErrorResponse {
            code: code.to_owned(),
            message: message.to_owned(),
        },
    )
}

fn invalid_request() -> Response {
    secure_json(
        StatusCode::BAD_REQUEST,
        &CredentialRelayErrorResponse {
            code: "invalid_request".to_owned(),
            message: "invalid credential relay request".to_owned(),
        },
    )
}

fn internal_error() -> Response {
    secure_json(
        StatusCode::INTERNAL_SERVER_ERROR,
        &CredentialRelayErrorResponse {
            code: "internal".to_owned(),
            message: "credential relay service failed".to_owned(),
        },
    )
}

fn page_headers(headers: &mut HeaderMap) {
    secure_headers(headers);
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; form-action 'none'; base-uri 'none'; frame-ancestors 'none'",
        ),
    );
}
