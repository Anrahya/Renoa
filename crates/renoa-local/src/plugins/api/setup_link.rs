//! The one check a plugin setup link passes before any surface shows it.
//!
//! A `plugin_manage` progress update carries either a provider sign-in link
//! or a link to Renoa's encrypted credential page. Every surface that shows
//! one to a person reads it through [`SetupLink::from_update`], and keeps only
//! its own wording and any rule of its own.

use renoa_agent::{ContentBlock, ToolOutput};
use url::Url;

use super::{PluginCredentialKind, PluginProgress};

/// The longest link a surface shows, before and after parsing.
const MAX_URL_BYTES: usize = 16 * 1024;
/// The longest connection or credential name a label is made from.
const MAX_NAME_BYTES: usize = 128;

/// What the person does at a setup link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupLinkKind {
    /// Sign in at the provider and approve access.
    Authorization,
    /// Enter a credential of this kind on Renoa's encrypted page.
    Credentials(PluginCredentialKind),
}

impl SetupLinkKind {
    /// The kind's name on the wire and in delivery records.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Authorization => "authorization",
            Self::Credentials(_) => "credentials",
        }
    }
}

/// A setup link that passed the check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupLink {
    pub kind: SetupLinkKind,
    /// A readable name for what is being connected.
    pub label: String,
    pub url: Url,
    /// When the link stops working, in Unix milliseconds, if it says.
    pub expires_at_ms: Option<i64>,
}

/// A `plugin_manage` progress update whose link failed the check; no surface
/// shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the Host returned an invalid setup link")]
pub struct InvalidSetupLink;

impl SetupLink {
    /// The setup link in a tool update: `None` when the update is not
    /// `plugin_manage` setup progress, and an error when it is but its link
    /// fails the check.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidSetupLink`] for setup progress with an unsafe link.
    #[must_use]
    pub fn from_update(tool: &str, update: &ToolOutput) -> Option<Result<Self, InvalidSetupLink>> {
        if tool != "plugin_manage" || update.is_error {
            return None;
        }
        let [ContentBlock::Text { text }] = update.content.as_slice() else {
            return None;
        };
        let progress: PluginProgress = serde_json::from_str(text).ok()?;
        Some(Self::check(progress).ok_or(InvalidSetupLink))
    }

    fn check(progress: PluginProgress) -> Option<Self> {
        let (kind, label, url, expires_at_ms) = match progress {
            PluginProgress::AuthorizationRequired(event) => {
                if !valid_name(&event.connection) || event.message.is_empty() {
                    return None;
                }
                let label = event
                    .display_name
                    .as_deref()
                    .filter(|name| valid_display_name(name))
                    .map_or(event.connection, humanize_name);
                (
                    SetupLinkKind::Authorization,
                    label,
                    event.authorization_url,
                    event.expires_at_ms,
                )
            }
            PluginProgress::CredentialRequired(event) => {
                if !valid_name(&event.credential) || event.message.is_empty() {
                    return None;
                }
                (
                    SetupLinkKind::Credentials(event.credential_kind),
                    event.credential,
                    event.setup_url,
                    Some(event.expires_at_ms),
                )
            }
        };
        if url.len() > MAX_URL_BYTES {
            return None;
        }
        let parsed = Url::parse(&url).ok()?;
        // Parsing percent-encodes what the raw text left bare, so the bound
        // holds for the link as every surface shows it.
        if parsed.as_str().len() > MAX_URL_BYTES
            || url.chars().any(char::is_control)
            || parsed.scheme() != "https"
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || expires_at_ms.is_some_and(|expiry| expiry <= 0)
        {
            return None;
        }
        let shaped = match kind {
            SetupLinkKind::Authorization => parsed.fragment().is_none(),
            SetupLinkKind::Credentials(kind) => {
                parsed.query().is_none()
                    && parsed
                        .fragment()
                        .is_some_and(|fragment| valid_fragment(fragment, kind))
            }
        };
        shaped.then_some(Self {
            kind,
            label,
            url: parsed,
            expires_at_ms,
        })
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_NAME_BYTES && !name.bytes().any(|b| b.is_ascii_control())
}

fn valid_display_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
        })
}

fn humanize_name(name: &str) -> String {
    name.split(['.', '-'])
        .filter(|part| !part.is_empty())
        .map(|part| match part {
            "api" => "API".to_owned(),
            "mcp" => "MCP".to_owned(),
            "oauth" => "OAuth".to_owned(),
            _ => {
                let mut characters = part.chars();
                characters.next().map_or_else(String::new, |first| {
                    first.to_uppercase().chain(characters).collect()
                })
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The credential page's fragment: the version, the key that keeps the
/// credential unreadable to the coordinator, the relay capability, and for an
/// OAuth client the issuer it is bound to.
fn valid_fragment(fragment: &str, kind: PluginCredentialKind) -> bool {
    let mut version = None;
    let mut key = None;
    let mut token = None;
    let mut issuer = None;
    for (name, value) in url::form_urlencoded::parse(fragment.as_bytes()) {
        let slot = match name.as_ref() {
            "v" => &mut version,
            "key" => &mut key,
            "token" => &mut token,
            "issuer" => &mut issuer,
            _ => return false,
        };
        if slot.replace(value.into_owned()).is_some() {
            return false;
        }
    }
    version.as_deref() == Some("1")
        && key.as_deref().is_some_and(valid_secret_hex)
        && token.as_deref().is_some_and(valid_secret_hex)
        && match kind {
            PluginCredentialKind::ApiToken => issuer.is_none(),
            PluginCredentialKind::OAuthClient => issuer.as_deref().is_some_and(|issuer| {
                Url::parse(issuer).is_ok_and(|issuer| {
                    issuer.scheme() == "https"
                        && issuer.host_str().is_some()
                        && issuer.username().is_empty()
                        && issuer.password().is_none()
                        && issuer.query().is_none()
                        && issuer.fragment().is_none()
                })
            }),
        }
}

fn valid_secret_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

#[cfg(test)]
#[path = "setup_link/tests.rs"]
mod tests;
