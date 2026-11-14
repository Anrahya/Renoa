//! What this computer keeps between launches, in its secret store: its key
//! and, once signed in, its credential. Nothing else is kept. The store is
//! the login Keychain on macOS and the Secret Service keyring on Linux.

use renoa_rcp_wire::DeviceCredentials;

use crate::{device, key::MacKey};

const SERVICE: &str = "live.renoa.mac";
const KEY: &str = "device-key";
const CREDENTIALS: &str = "device-credentials";

pub use store::PROBLEM;

/// This computer's key, made and kept on first use. Only a store that holds
/// no key gets a new one: any other failure, such as a locked store, leaves
/// the kept key alone.
pub fn key() -> Result<MacKey, String> {
    match store::read(KEY) {
        Ok(Some(pkcs8)) => return MacKey::from_pkcs8(&pkcs8),
        Ok(None) => {}
        Err(error) => {
            return Err(format!(
                "the {} could not read this {}'s key: {error}",
                store::NAME,
                device::KIND
            ));
        }
    }
    let key = MacKey::generate()?;
    store::write(KEY, key.pkcs8())
        .map_err(|error| format!("the {} refused the key: {error}", store::NAME))?;
    Ok(key)
}

/// The credential this computer signed in with, if it has one. A kept
/// credential that does not read as one counts as none: signing in again
/// replaces it.
pub fn credentials() -> Result<Option<DeviceCredentials>, String> {
    match store::read(CREDENTIALS) {
        Ok(kept) => Ok(kept.and_then(|kept| serde_json::from_slice(&kept).ok())),
        Err(error) => Err(format!(
            "the {} could not read this {}'s sign-in: {error}",
            store::NAME,
            device::KIND
        )),
    }
}

pub fn keep_credentials(credentials: &DeviceCredentials) -> Result<(), String> {
    let json = serde_json::to_vec(credentials).map_err(|error| error.to_string())?;
    store::write(CREDENTIALS, &json)
        .map_err(|error| format!("the {} refused the sign-in: {error}", store::NAME))
}

/// Forgets the credential, as signing out does. The key stays, so signing in
/// again is the same device.
pub fn forget_credentials() {
    store::delete(CREDENTIALS);
}

#[cfg(target_os = "macos")]
mod store {
    use security_framework::passwords::{
        delete_generic_password, get_generic_password, set_generic_password,
    };

    pub const NAME: &str = "Keychain";

    /// What the owner reads when the Keychain fails; the detail goes to the log.
    pub const PROBLEM: &str = "Renoa couldn't use this Mac's Keychain. Unlock it in Keychain Access, then open Renoa again.";

    /// The Keychain's answer when it holds no such item (`errSecItemNotFound`).
    const NOT_FOUND: i32 = -25300;

    /// The item, or none when the Keychain holds no such item.
    pub fn read(item: &str) -> Result<Option<Vec<u8>>, String> {
        match get_generic_password(super::SERVICE, item) {
            Ok(kept) => Ok(Some(kept)),
            Err(error) if error.code() == NOT_FOUND => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    pub fn write(item: &str, secret: &[u8]) -> Result<(), String> {
        set_generic_password(super::SERVICE, item, secret).map_err(|error| error.to_string())
    }

    pub fn delete(item: &str) {
        let _ = delete_generic_password(super::SERVICE, item);
    }
}

#[cfg(target_os = "linux")]
mod store {
    use keyring_core::{Entry, Error, api::CredentialStoreApi as _};
    use zbus_secret_service_keyring_store::Store;

    pub const NAME: &str = "keyring";

    /// What the owner reads when the keyring fails; the detail goes to the log.
    pub const PROBLEM: &str = "Renoa couldn't use this PC's keyring. Unlock it in Passwords and Keys, then open Renoa again.";

    /// The item in the login collection, through a fresh connection: the
    /// store is read a handful of times a launch.
    fn entry(item: &str) -> Result<Entry, Error> {
        Store::new()?.build(super::SERVICE, item, None)
    }

    /// The item, or none when the keyring holds no such item.
    pub fn read(item: &str) -> Result<Option<Vec<u8>>, String> {
        match entry(item).and_then(|entry| entry.get_secret()) {
            Ok(kept) => Ok(Some(kept)),
            Err(Error::NoEntry) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    pub fn write(item: &str, secret: &[u8]) -> Result<(), String> {
        entry(item)
            .and_then(|entry| entry.set_secret(secret))
            .map_err(|error| error.to_string())
    }

    pub fn delete(item: &str) {
        let _ = entry(item).and_then(|entry| entry.delete_credential());
    }
}
