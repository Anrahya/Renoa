use std::time::{Duration, SystemTime};

use renoa_protocol::{PrincipalId, SurfaceRef};
use url::Url;
use uuid::Uuid;
use webauthn_rs::{
    Webauthn, WebauthnBuilder,
    prelude::{
        CreationChallengeResponse, PublicKeyCredential, RegisterPublicKeyCredential,
        RequestChallengeResponse,
    },
};

use crate::{
    ConnectionTicket, ControlError, PasskeyBootstrapToken, control_log,
    device_key::{DeviceRegistration, check_registration_proof},
    device_store::SignedInDevice,
    store::ControlStore,
};
use crate::{browser_sessions::SESSION_LIFETIME, identity::BrowserSessionToken};

const CEREMONY_LIFETIME: Duration = Duration::from_mins(5);
const TICKET_LIFETIME: Duration = Duration::from_mins(1);
const MAX_SURFACE_BYTES: usize = 64;

#[derive(Clone)]
pub(crate) struct BrowserIdentity {
    webauthn: Webauthn,
    origin: String,
}

pub(crate) struct CeremonyOptions<T> {
    pub(crate) ceremony_id: Uuid,
    pub(crate) options: T,
}

#[derive(Clone)]
pub(crate) struct TicketGrant {
    pub(crate) ticket: ConnectionTicket,
    pub(crate) expires_at: SystemTime,
    pub(crate) session: BrowserSessionToken,
    pub(crate) session_expires_at: SystemTime,
}

/// What a finished ceremony grants: a browser's ticket and remembered login,
/// or the credential of the device an app signed in.
pub(crate) enum SignIn {
    Browser(TicketGrant),
    Device(SignedInDevice),
}

impl BrowserIdentity {
    /// `app_origins` are extra origins whose assertions are accepted, each
    /// compared by exact equality, such as an Android app's
    /// `android:apk-key-hash:` origin.
    pub(crate) fn new(
        rp_id: &str,
        rp_origin: &str,
        app_origins: &[Url],
    ) -> Result<Self, ControlError> {
        let origin = Url::parse(rp_origin)
            .map_err(|error| ControlError::invalid(format!("invalid passkey origin: {error}")))?;
        validate_origin(&origin)?;
        let mut builder = WebauthnBuilder::new(rp_id, &origin)
            .map_err(|_| ControlError::invalid("passkey RP ID does not match its origin"))?
            .rp_name("Renoa");
        for app_origin in app_origins {
            builder = builder.append_allowed_origin(app_origin);
        }
        let webauthn = builder
            .build()
            .map_err(|_| ControlError::invalid("invalid passkey configuration"))?;
        Ok(Self {
            webauthn,
            origin: origin.origin().ascii_serialization(),
        })
    }

    pub(crate) fn origin(&self) -> &str {
        &self.origin
    }

    pub(crate) async fn start_registration(
        &self,
        store: &ControlStore,
        bootstrap_token: PasskeyBootstrapToken,
        surface: SurfaceRef,
        device: Option<DeviceRegistration>,
    ) -> Result<CeremonyOptions<CreationChallengeResponse>, ControlError> {
        let now = SystemTime::now();
        let bootstrap = store
            .load_registration_bootstrap(bootstrap_token.clone(), now)
            .await?;
        if let Some(device) = &device {
            // Refuse before the one-use bootstrap is spent.
            store
                .check_device_registration(bootstrap.principal_id, surface.clone(), device.clone())
                .await?;
        }
        let excluded = bootstrap
            .passkeys
            .iter()
            .map(|passkey| passkey.cred_id().clone())
            .collect::<Vec<_>>();
        let account = bootstrap.principal_id.to_string();
        let (options, state) = self
            .webauthn
            .start_passkey_registration(
                bootstrap.principal_id.as_uuid(),
                &account,
                "Renoa",
                (!excluded.is_empty()).then_some(excluded),
            )
            .map_err(|error| {
                ControlError::store(format!("passkey registration could not start: {error}"))
            })?;
        let ceremony_id = Uuid::new_v4();
        store
            .save_registration_ceremony(
                bootstrap_token,
                bootstrap.principal_id,
                surface,
                ceremony_id,
                state,
                device,
                expiry(now, CEREMONY_LIFETIME)?,
                now,
            )
            .await?;
        Ok(CeremonyOptions {
            ceremony_id,
            options,
        })
    }

    pub(crate) async fn finish_registration(
        &self,
        store: &ControlStore,
        ceremony_id: Uuid,
        credential: RegisterPublicKeyCredential,
        device_proof: Option<&str>,
    ) -> Result<SignIn, ControlError> {
        let now = SystemTime::now();
        let ceremony = store.claim_registration_ceremony(ceremony_id, now).await?;
        check_registration_proof(ceremony_id, ceremony.device.as_ref(), device_proof)?;
        let passkey = self
            .webauthn
            .finish_passkey_registration(&credential, &ceremony.state)
            .map_err(|_| ControlError::authentication_failed())?;
        if let Some(registration) = ceremony.device {
            let signed_in = store
                .store_registration_and_device(
                    ceremony.principal_id,
                    ceremony.surface,
                    passkey,
                    registration,
                    now,
                )
                .await?;
            log_device_sign_in(ceremony.principal_id, &signed_in);
            return Ok(SignIn::Device(signed_in));
        }
        let grant = new_ticket(now)?;
        store
            .store_registration_and_ticket(
                ceremony.principal_id,
                ceremony.surface,
                passkey,
                grant.clone(),
                now,
            )
            .await?;
        Ok(SignIn::Browser(grant))
    }

    pub(crate) async fn start_authentication(
        &self,
        store: &ControlStore,
        principal_id: PrincipalId,
        surface: SurfaceRef,
        device: Option<DeviceRegistration>,
    ) -> Result<CeremonyOptions<RequestChallengeResponse>, ControlError> {
        // No device pre-check here, unlike `start_registration`: this request
        // carries no secret, so an answer about the key (registered to another
        // sign-in, signed out) or the account (at its device limit) would
        // reach anyone who can name the principal. The transaction in
        // `finish_authentication` refuses a conflicting sign-in after the
        // passkey assertion has proved the person, and writes nothing.
        let passkeys = store.load_passkeys_for_authentication(principal_id).await?;
        let (options, state) = self
            .webauthn
            .start_passkey_authentication(&passkeys)
            .map_err(|error| {
                ControlError::store(format!("passkey authentication could not start: {error}"))
            })?;
        let now = SystemTime::now();
        let ceremony_id = Uuid::new_v4();
        store
            .save_authentication_ceremony(
                principal_id,
                surface,
                ceremony_id,
                state,
                device,
                expiry(now, CEREMONY_LIFETIME)?,
                now,
            )
            .await?;
        Ok(CeremonyOptions {
            ceremony_id,
            options,
        })
    }

    pub(crate) async fn finish_authentication(
        &self,
        store: &ControlStore,
        ceremony_id: Uuid,
        credential: PublicKeyCredential,
        device_proof: Option<&str>,
    ) -> Result<SignIn, ControlError> {
        let now = SystemTime::now();
        let ceremony = store
            .claim_authentication_ceremony(ceremony_id, now)
            .await?;
        check_registration_proof(ceremony_id, ceremony.device.as_ref(), device_proof)?;
        let authentication = self
            .webauthn
            .finish_passkey_authentication(&credential, &ceremony.state)
            .map_err(|_| ControlError::authentication_failed())?;
        if let Some(registration) = ceremony.device {
            let signed_in = store
                .update_passkey_and_store_device(
                    ceremony.principal_id,
                    ceremony.surface,
                    authentication,
                    registration,
                    now,
                )
                .await?;
            log_device_sign_in(ceremony.principal_id, &signed_in);
            return Ok(SignIn::Device(signed_in));
        }
        let grant = new_ticket(now)?;
        store
            .update_passkey_and_store_ticket(
                ceremony.principal_id,
                ceremony.surface,
                authentication,
                grant.clone(),
                now,
            )
            .await?;
        Ok(SignIn::Browser(grant))
    }
}

fn log_device_sign_in(principal_id: PrincipalId, signed_in: &SignedInDevice) {
    control_log::event(
        "info",
        "device_signed_in",
        &serde_json::json!({
            "device_id": signed_in.credentials.device_id,
            "principal_id": principal_id,
            "adopted": signed_in.adopted,
        }),
    );
}

pub(crate) fn parse_surface(value: String) -> Result<SurfaceRef, ControlError> {
    if value.is_empty()
        || value.len() > MAX_SURFACE_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(ControlError::invalid(
            "surface must be 1-64 ASCII letters, digits, dots, dashes, or underscores",
        ));
    }
    Ok(SurfaceRef::new(value))
}

fn new_ticket(now: SystemTime) -> Result<TicketGrant, ControlError> {
    Ok(TicketGrant {
        ticket: ConnectionTicket::generate()?,
        expires_at: expiry(now, TICKET_LIFETIME)?,
        session: BrowserSessionToken::generate()?,
        session_expires_at: expiry(now, SESSION_LIFETIME)?,
    })
}

fn expiry(now: SystemTime, lifetime: Duration) -> Result<SystemTime, ControlError> {
    now.checked_add(lifetime)
        .ok_or_else(|| ControlError::store("identity expiry exceeds system time range"))
}

fn validate_origin(origin: &Url) -> Result<(), ControlError> {
    let localhost = origin.host_str() == Some("localhost");
    if origin.host_str().is_none()
        || (origin.scheme() != "https" && !(localhost && origin.scheme() == "http"))
        || !origin.username().is_empty()
        || origin.password().is_some()
        || origin.query().is_some()
        || origin.fragment().is_some()
        || origin.path() != "/"
    {
        return Err(ControlError::invalid(
            "passkey origin must be an HTTPS origin (or HTTP localhost) without a path",
        ));
    }
    Ok(())
}
