use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::{
    rand::SystemRandom,
    signature::{RSA_PKCS1_SHA256, RsaKeyPair},
};
use std::{io, path::Path};

pub(super) struct AppAuth {
    client_id: String,
    key: RsaKeyPair,
}

impl AppAuth {
    pub(super) fn load(client_id: &str, path: &Path) -> Result<Self, io::Error> {
        let bytes = crate::github_review::private_credential(path, 16 * 1024)?;
        let key = RsaKeyPair::from_der(&bytes)
            .map_err(|_| io::Error::other("invalid GitHub App RSA DER key"))?;
        Ok(Self {
            client_id: client_id.to_owned(),
            key,
        })
    }

    pub(super) fn jwt(&self) -> Result<String, io::Error> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_secs();
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
        let claims = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&serde_json::json!({
                "iat":now.saturating_sub(60), "exp":now + 540, "iss":self.client_id
            }))
            .map_err(io::Error::other)?,
        );
        let unsigned = format!("{header}.{claims}");
        let mut signature = vec![0; self.key.public().modulus_len()];
        self.key
            .sign(
                &RSA_PKCS1_SHA256,
                &SystemRandom::new(),
                unsigned.as_bytes(),
                &mut signature,
            )
            .map_err(|_| io::Error::other("GitHub App JWT signing failed"))?;
        Ok(format!("{unsigned}.{}", URL_SAFE_NO_PAD.encode(signature)))
    }
}
