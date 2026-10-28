//! The Android apps the passkey relying party serves. Each app contributes the
//! origin its assertions carry, and the Digital Asset Links file that lets
//! Android offer it a passkey for the relying party.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
    routing::get,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;
use url::Url;

use crate::{ControlError, coordinator::CoordinatorState};

const ASSET_LINKS_PATH: &str = "/.well-known/assetlinks.json";
const MAX_APPS: usize = 4;
const MAX_FINGERPRINTS: usize = 4;
const MAX_PACKAGE_CHARS: usize = 255;
const FINGERPRINT_BYTES: usize = 32;

/// The configured apps. Empty means the coordinator trusts no app origin and
/// serves no asset links.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AndroidApps(Vec<AndroidApp>);

#[derive(Debug, Clone, PartialEq, Eq)]
struct AndroidApp {
    package_name: String,
    /// SHA-256 digests of the app's signing certificates: one per build
    /// signature, such as a debug and a release certificate.
    certificates: Vec<[u8; FINGERPRINT_BYTES]>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AppsFile {
    apps: Vec<AppEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AppEntry {
    package_name: String,
    sha256_cert_fingerprints: Vec<String>,
}

impl AndroidApps {
    /// Parses the configuration file: `{"apps": [{"packageName": "...",
    /// "sha256CertFingerprints": ["AA:BB:..."]}]}`, with each fingerprint as
    /// `keytool` prints it.
    ///
    /// # Errors
    ///
    /// Returns an error naming the entry that is not a valid package name or
    /// SHA-256 certificate fingerprint, or that repeats another.
    pub fn from_json(json: &str) -> Result<Self, ControlError> {
        let file: AppsFile = serde_json::from_str(json)
            .map_err(|error| ControlError::invalid(format!("android apps file: {error}")))?;
        if file.apps.is_empty() || file.apps.len() > MAX_APPS {
            return Err(ControlError::invalid(format!(
                "android apps file: \"apps\" needs 1 to {MAX_APPS} entries, found {}",
                file.apps.len()
            )));
        }
        let mut apps: Vec<AndroidApp> = Vec::new();
        for entry in file.apps {
            let app = AndroidApp::parse(entry)?;
            let repeated = apps.iter().any(|known| {
                known.package_name == app.package_name
                    || known
                        .certificates
                        .iter()
                        .any(|certificate| app.certificates.contains(certificate))
            });
            if repeated {
                return Err(ControlError::invalid(format!(
                    "android apps file: {} repeats a package name or certificate",
                    app.package_name
                )));
            }
            apps.push(app);
        }
        Ok(Self(apps))
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The origin an assertion made by each app's certificate carries, which
    /// the relying party accepts by exact match.
    pub(crate) fn origins(&self) -> Result<Vec<Url>, ControlError> {
        self.0
            .iter()
            .flat_map(|app| &app.certificates)
            .map(|certificate| {
                let origin = format!(
                    "android:apk-key-hash:{}",
                    URL_SAFE_NO_PAD.encode(certificate)
                );
                Url::parse(&origin).map_err(|error| {
                    ControlError::invalid(format!("android app origin is invalid: {error}"))
                })
            })
            .collect()
    }

    fn asset_links(&self) -> serde_json::Value {
        self.0
            .iter()
            .map(|app| {
                serde_json::json!({
                    "relation": ["delegate_permission/common.get_login_creds"],
                    "target": {
                        "namespace": "android_app",
                        "package_name": app.package_name,
                        "sha256_cert_fingerprints": app
                            .certificates
                            .iter()
                            .map(fingerprint)
                            .collect::<Vec<_>>(),
                    },
                })
            })
            .collect()
    }
}

impl AndroidApp {
    fn parse(entry: AppEntry) -> Result<Self, ControlError> {
        let package_name = entry.package_name;
        if !valid_package_name(&package_name) {
            return Err(ControlError::invalid(format!(
                "android apps file: \"{package_name}\" is not an Android package name \
                 (dot-separated segments of letters, digits and underscores, \
                 each starting with a letter)"
            )));
        }
        let count = entry.sha256_cert_fingerprints.len();
        if count == 0 || count > MAX_FINGERPRINTS {
            return Err(ControlError::invalid(format!(
                "android apps file: {package_name} needs 1 to {MAX_FINGERPRINTS} \
                 \"sha256CertFingerprints\", found {count}"
            )));
        }
        let mut certificates = Vec::new();
        for value in &entry.sha256_cert_fingerprints {
            let certificate = parse_fingerprint(value).ok_or_else(|| {
                ControlError::invalid(format!(
                    "android apps file: {package_name} has a sha256CertFingerprints entry \
                     that is not 32 colon-separated hex bytes like \"14:6D:E9:...\""
                ))
            })?;
            if certificates.contains(&certificate) {
                return Err(ControlError::invalid(format!(
                    "android apps file: {package_name} lists a certificate twice"
                )));
            }
            certificates.push(certificate);
        }
        Ok(Self {
            package_name,
            certificates,
        })
    }
}

fn valid_package_name(name: &str) -> bool {
    name.len() <= MAX_PACKAGE_CHARS
        && name.split('.').count() >= 2
        && name.split('.').all(|segment| {
            segment
                .chars()
                .next()
                .is_some_and(|first| first.is_ascii_alphabetic())
                && segment
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '_')
        })
}

fn parse_fingerprint(value: &str) -> Option<[u8; FINGERPRINT_BYTES]> {
    let mut bytes = [0_u8; FINGERPRINT_BYTES];
    let mut groups = value.split(':');
    for byte in &mut bytes {
        let group = groups.next()?;
        if group.len() != 2 || !group.bytes().all(|digit| digit.is_ascii_hexdigit()) {
            return None;
        }
        *byte = u8::from_str_radix(group, 16).ok()?;
    }
    groups.next().is_none().then_some(bytes)
}

fn fingerprint(certificate: &[u8; FINGERPRINT_BYTES]) -> String {
    certificate
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

pub(crate) fn routes() -> Router<Arc<CoordinatorState>> {
    Router::new().route(ASSET_LINKS_PATH, get(asset_links))
}

async fn asset_links(State(state): State<Arc<CoordinatorState>>) -> Response {
    let mut response = Json(state.android_apps.asset_links()).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=3600"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}
