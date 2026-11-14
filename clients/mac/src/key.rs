//! This computer's device key: a P-256 key made here and kept by the vault in
//! its PKCS#8 form.

use renoa_rcp_client::{ClientError, DeviceKey};
use ring::{
    rand::SystemRandom,
    signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair as _},
};

pub struct MacKey {
    pair: EcdsaKeyPair,
    pkcs8: Vec<u8>,
}

impl MacKey {
    /// Makes a new key.
    pub fn generate() -> Result<Self, String> {
        let document =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &SystemRandom::new())
                .map_err(|_| format!("this {} could not make a key", crate::device::KIND))?;
        Self::from_pkcs8(document.as_ref())
    }

    /// Reads a key the vault kept.
    pub fn from_pkcs8(pkcs8: &[u8]) -> Result<Self, String> {
        let pair =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8, &SystemRandom::new())
                .map_err(|_| "the kept key is not a P-256 key".to_owned())?;
        Ok(Self {
            pair,
            pkcs8: pkcs8.to_vec(),
        })
    }

    /// The form the vault keeps.
    pub fn pkcs8(&self) -> &[u8] {
        &self.pkcs8
    }
}

impl DeviceKey for MacKey {
    fn public_key(&self) -> [u8; 65] {
        self.pair
            .public_key()
            .as_ref()
            .try_into()
            .expect("a P-256 public key is a 65-byte uncompressed point")
    }

    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, ClientError> {
        self.pair
            .sign(&SystemRandom::new(), message)
            .map(|signature| signature.as_ref().to_vec())
            .map_err(|_| {
                ClientError::Key(format!("this {}'s key could not sign", crate::device::KIND))
            })
    }
}
