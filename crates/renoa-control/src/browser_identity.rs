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

use crate::{ConnectionTicket, ControlError, PasskeyBootstrapToken, store::ControlStore};

const CEREMONY_LIFETIME: Duration = Duration::from_mins(5);
const TICKET_LIFETIME: Duration = Duration::from_mins(1);
const MAX_SURFACE_BYTES: usize = 64;

#[derive(Clone)]
pub(crate) struct BrowserIdentity {
    webauthn: Webauthn,
}

pub(crate) struct CeremonyOptions<T> {
    pub(crate) ceremony_id: Uuid,
    pub(crate) options: T,
}

pub(crate) struct TicketGrant {
    pub(crate) ticket: ConnectionTicket,
    pub(crate) expires_at: SystemTime,
}

impl BrowserIdentity {
    pub(crate) fn new(rp_id: &str, rp_origin: &str) -> Result<Self, ControlError> {
        let origin = Url::parse(rp_origin)
            .map_err(|error| ControlError::invalid(format!("invalid passkey origin: {error}")))?;
        validate_origin(&origin)?;
        let webauthn = WebauthnBuilder::new(rp_id, &origin)
            .map_err(|_| ControlError::invalid("passkey RP ID does not match its origin"))?
            .rp_name("Renoa")
            .build()
            .map_err(|_| ControlError::invalid("invalid passkey configuration"))?;
        Ok(Self { webauthn })
    }

    pub(crate) async fn start_registration(
        &self,
        store: &ControlStore,
        bootstrap_token: PasskeyBootstrapToken,
        surface: SurfaceRef,
    ) -> Result<CeremonyOptions<CreationChallengeResponse>, ControlError> {
        let now = SystemTime::now();
        let bootstrap = store
            .load_registration_bootstrap(bootstrap_token.clone(), now)
            .await?;
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
    ) -> Result<TicketGrant, ControlError> {
        let now = SystemTime::now();
        let ceremony = store.claim_registration_ceremony(ceremony_id, now).await?;
        let passkey = self
            .webauthn
            .finish_passkey_registration(&credential, &ceremony.state)
            .map_err(|_| ControlError::authentication_failed())?;
        let grant = new_ticket(now)?;
        store
            .store_registration_and_ticket(
                ceremony.principal_id,
                ceremony.surface,
                passkey,
                grant.ticket.clone(),
                grant.expires_at,
                now,
            )
            .await?;
        Ok(grant)
    }

    pub(crate) async fn start_authentication(
        &self,
        store: &ControlStore,
        principal_id: PrincipalId,
        surface: SurfaceRef,
    ) -> Result<CeremonyOptions<RequestChallengeResponse>, ControlError> {
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
    ) -> Result<TicketGrant, ControlError> {
        let now = SystemTime::now();
        let ceremony = store
            .claim_authentication_ceremony(ceremony_id, now)
            .await?;
        let authentication = self
            .webauthn
            .finish_passkey_authentication(&credential, &ceremony.state)
            .map_err(|_| ControlError::authentication_failed())?;
        let grant = new_ticket(now)?;
        store
            .update_passkey_and_store_ticket(
                ceremony.principal_id,
                ceremony.surface,
                authentication,
                grant.ticket.clone(),
                grant.expires_at,
                now,
            )
            .await?;
        Ok(grant)
    }
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
