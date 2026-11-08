//! What this Mac keeps between launches, in the login Keychain: its key and,
//! once signed in, its credential. Nothing else is kept.

use renoa_rcp_wire::DeviceCredentials;
use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};

use crate::key::MacKey;

const SERVICE: &str = "live.renoa.mac";
const KEY: &str = "device-key";
const CREDENTIALS: &str = "device-credentials";

/// The Keychain's answer when it holds no such item (`errSecItemNotFound`).
const NOT_FOUND: i32 = -25300;

/// This Mac's key, made and kept on first use. Only a Keychain that holds no
/// key gets a new one: any other failure, such as a locked Keychain, leaves
/// the kept key alone.
pub fn key() -> Result<MacKey, String> {
    match get_generic_password(SERVICE, KEY) {
        Ok(pkcs8) => return MacKey::from_pkcs8(&pkcs8),
        Err(error) if error.code() == NOT_FOUND => {}
        Err(error) => {
            return Err(format!(
                "the Keychain could not read this Mac's key: {error}"
            ));
        }
    }
    let key = MacKey::generate()?;
    set_generic_password(SERVICE, KEY, key.pkcs8())
        .map_err(|error| format!("the Keychain refused the key: {error}"))?;
    Ok(key)
}

/// The credential this Mac signed in with, if it has one. A kept credential
/// that does not read as one counts as none: signing in again replaces it.
pub fn credentials() -> Result<Option<DeviceCredentials>, String> {
    match get_generic_password(SERVICE, CREDENTIALS) {
        Ok(kept) => Ok(serde_json::from_slice(&kept).ok()),
        Err(error) if error.code() == NOT_FOUND => Ok(None),
        Err(error) => Err(format!(
            "the Keychain could not read this Mac's sign-in: {error}"
        )),
    }
}

pub fn keep_credentials(credentials: &DeviceCredentials) -> Result<(), String> {
    let json = serde_json::to_vec(credentials).map_err(|error| error.to_string())?;
    set_generic_password(SERVICE, CREDENTIALS, &json)
        .map_err(|error| format!("the Keychain refused the sign-in: {error}"))
}

/// Forgets the credential, as signing out does. The key stays, so signing in
/// again is the same device.
pub fn forget_credentials() {
    let _ = delete_generic_password(SERVICE, CREDENTIALS);
}
