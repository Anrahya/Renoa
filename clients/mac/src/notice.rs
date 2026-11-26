//! This computer's P-256 notice key and the envelope opening for sealed plugin
//! setup notices (`renoa.setup-notice.v1`).
//!
//! Nodes seal each setup link to the device's registered [`NoticeKey`] so the
//! coordinator only ever holds ciphertext. Only this device's kept private half
//! opens notices bound to its `notice_id` and `device_id`.

use std::{
    fmt,
    time::{SystemTime, UNIX_EPOCH},
};

use aws_lc_rs::{
    aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey},
    agreement::{ECDH_P256, PrivateKey, UnparsedPublicKey, agree},
    encoding::{AsBigEndian as _, EcPrivateKeyBin},
    hkdf::{HKDF_SHA256, Salt},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use renoa_rcp_wire::{DeviceId, HeldNotice, NoticeKey};
use serde::Deserialize;
use url::Url;
use uuid::Uuid;

/// Names the envelope wherever its bytes are bound: HKDF salt and AAD.
const DOMAIN: &[u8] = b"renoa.setup-notice.v1";
const VERSION: u8 = 1;
const POINT_BYTES: usize = 65;
const TAG_BYTES: usize = 16;

/// What the owner does at a setup link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupLinkKind {
    /// Sign in at the provider and approve access.
    Authorization,
    /// Enter a credential on Renoa's encrypted setup page.
    Credentials,
}

impl SetupLinkKind {
    pub const fn words(self) -> &'static str {
        match self {
            Self::Authorization => "Sign in there to let Renoa use it.",
            Self::Credentials => "Enter its key on a private Renoa page.",
        }
    }
}

/// A setup link a node sealed to this device.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct SetupLink {
    pub kind: SetupLinkKind,
    pub label: String,
    pub url: String,
    pub expires_at_ms: i64,
}

impl SetupLink {
    /// The host the link opens, so the card can show where Connect goes.
    #[must_use]
    pub fn host(&self) -> Option<String> {
        Url::parse(&self.url)
            .ok()
            .and_then(|parsed| parsed.host_str().map(str::to_owned))
    }
}

/// Credential setup links carry the decryption key in their URL fragment:
/// formatting the record must never print the URL.
impl fmt::Debug for SetupLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SetupLink")
            .field("kind", &self.kind)
            .field("label", &self.label)
            .field("url", &"<secret>")
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

/// A setup link waiting for the owner, under the notice that carried it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldLink {
    pub notice_id: Uuid,
    pub link: SetupLink,
}

/// A notice that cannot be opened on this device or does not hold a valid HTTPS
/// setup link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnreadableNotice;

/// This computer's P-256 ECDH key for opening setup notices.
pub struct NoticeSecret {
    private: PrivateKey,
    public: [u8; POINT_BYTES],
    scalar: Vec<u8>,
}

impl NoticeSecret {
    /// Makes a new P-256 notice key.
    pub fn generate() -> Result<Self, String> {
        let private = PrivateKey::generate(&ECDH_P256)
            .map_err(|_| format!("this {} could not make a notice key", crate::device::KIND))?;
        Self::from_private(private)
    }

    /// Reads a P-256 private scalar kept in the vault.
    pub fn from_scalar(scalar: &[u8]) -> Result<Self, String> {
        let private = PrivateKey::from_private_key(&ECDH_P256, scalar)
            .map_err(|_| "the kept notice key is not a P-256 key".to_owned())?;
        Self::from_private(private)
    }

    fn from_private(private: PrivateKey) -> Result<Self, String> {
        let public_bytes = private
            .compute_public_key()
            .map_err(|_| "could not compute the notice public key".to_owned())?;
        let public: [u8; POINT_BYTES] = public_bytes
            .as_ref()
            .try_into()
            .map_err(|_| "the notice public key is not an uncompressed P-256 point".to_owned())?;
        let scalar: EcPrivateKeyBin<'static> = private
            .as_be_bytes()
            .map_err(|_| "could not export the notice key".to_owned())?;
        Ok(Self {
            private,
            public,
            scalar: scalar.as_ref().to_vec(),
        })
    }

    /// The 32-byte big-endian private scalar kept in the vault.
    #[must_use]
    pub fn scalar(&self) -> &[u8] {
        &self.scalar
    }

    /// The base64url-encoded uncompressed P-256 public point registered with
    /// the coordinator.
    #[must_use]
    pub fn public_key(&self) -> NoticeKey {
        NoticeKey::new(URL_SAFE_NO_PAD.encode(self.public))
    }

    /// Opens `notice`, sealed to this key for `device_id`.
    pub fn open(
        &self,
        notice: &HeldNotice,
        device_id: DeviceId,
    ) -> Result<SetupLink, UnreadableNotice> {
        let envelope = URL_SAFE_NO_PAD
            .decode(&notice.sealed)
            .map_err(|_| UnreadableNotice)?;
        let (&version, rest) = envelope.split_first().ok_or(UnreadableNotice)?;
        if version != VERSION || rest.len() < POINT_BYTES + NONCE_LEN + TAG_BYTES {
            return Err(UnreadableNotice);
        }
        let (ephemeral, rest) = rest.split_at(POINT_BYTES);
        let (nonce, ciphertext) = rest.split_at(NONCE_LEN);
        let mut info = Vec::with_capacity(POINT_BYTES * 2);
        info.extend_from_slice(ephemeral);
        info.extend_from_slice(&self.public);
        let key = agree(
            &self.private,
            UnparsedPublicKey::new(&ECDH_P256, ephemeral),
            UnreadableNotice,
            |secret| {
                let prk = Salt::new(HKDF_SHA256, DOMAIN).extract(secret);
                let context = [info.as_slice()];
                let okm = prk
                    .expand(&context, &AES_256_GCM)
                    .map_err(|_| UnreadableNotice)?;
                Ok(LessSafeKey::new(UnboundKey::from(okm)))
            },
        )?;
        let mut buffer = ciphertext.to_vec();
        let plaintext = key
            .open_in_place(
                Nonce::try_assume_unique_for_key(nonce).map_err(|_| UnreadableNotice)?,
                Aad::from(associated_data(notice.notice_id, device_id)),
                &mut buffer,
            )
            .map_err(|_| UnreadableNotice)?;
        let link: SetupLink = serde_json::from_slice(plaintext).map_err(|_| UnreadableNotice)?;
        validate_link(&link)?;
        Ok(link)
    }
}

fn validate_link(link: &SetupLink) -> Result<(), UnreadableNotice> {
    if link.label.trim().is_empty()
        || link.expires_at_ms <= 0
        || link
            .url
            .chars()
            .any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(UnreadableNotice);
    }
    let parsed = Url::parse(&link.url).map_err(|_| UnreadableNotice)?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(UnreadableNotice);
    }
    Ok(())
}

/// Current wall-clock time in Unix milliseconds.
#[must_use]
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
        })
}

fn associated_data(notice_id: Uuid, device_id: DeviceId) -> Vec<u8> {
    let mut data = Vec::with_capacity(DOMAIN.len() + 32);
    data.extend_from_slice(DOMAIN);
    data.extend_from_slice(notice_id.as_bytes());
    data.extend_from_slice(device_id.as_uuid().as_bytes());
    data
}

#[cfg(test)]
pub(crate) mod tests {
    use aws_lc_rs::{
        aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey},
        agreement::{ECDH_P256, EphemeralPrivateKey, UnparsedPublicKey, agree_ephemeral},
        hkdf::{HKDF_SHA256, Salt},
        rand::{SecureRandom as _, SystemRandom},
    };
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use renoa_rcp_wire::{DeviceId, HeldNotice};
    use serde::Deserialize;
    use uuid::Uuid;

    use super::{DOMAIN, NoticeSecret, SetupLinkKind, UnreadableNotice, VERSION, associated_data};

    #[derive(Deserialize)]
    struct Vector {
        private_key: String,
        public_key: String,
        notice_id: Uuid,
        device_id: Uuid,
        sealed: String,
        plaintext: String,
    }

    pub(crate) fn seal_for_test(
        recipient: &[u8; 65],
        notice_id: Uuid,
        device_id: DeviceId,
        plaintext: &[u8],
    ) -> String {
        let random = SystemRandom::new();
        let ephemeral = EphemeralPrivateKey::generate(&ECDH_P256, &random).expect("ephemeral");
        let ephemeral_public = ephemeral.compute_public_key().expect("public");
        let mut info = ephemeral_public.as_ref().to_vec();
        info.extend_from_slice(recipient);
        let key = agree_ephemeral(
            ephemeral,
            UnparsedPublicKey::new(&ECDH_P256, recipient),
            (),
            |secret| {
                let prk = Salt::new(HKDF_SHA256, DOMAIN).extract(secret);
                let context = [info.as_slice()];
                let okm = prk.expand(&context, &AES_256_GCM).map_err(|_| ())?;
                Ok(LessSafeKey::new(UnboundKey::from(okm)))
            },
        )
        .expect("agree");
        let mut nonce = [0_u8; NONCE_LEN];
        random.fill(&mut nonce).expect("nonce");
        let mut sealed = plaintext.to_vec();
        key.seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(associated_data(notice_id, device_id)),
            &mut sealed,
        )
        .expect("seal");
        let mut envelope = Vec::new();
        envelope.push(VERSION);
        envelope.extend_from_slice(ephemeral_public.as_ref());
        envelope.extend_from_slice(&nonce);
        envelope.extend_from_slice(&sealed);
        URL_SAFE_NO_PAD.encode(envelope)
    }

    #[test]
    fn opens_the_checked_in_shared_vector() {
        let vector: Vector = serde_json::from_str(include_str!(
            "../../../crates/renoa-node/tests/setup-notice-vector.json"
        ))
        .expect("vector");
        let scalar = URL_SAFE_NO_PAD.decode(&vector.private_key).expect("scalar");
        let secret = NoticeSecret::from_scalar(&scalar).expect("notice key");
        assert_eq!(secret.public_key().as_str(), vector.public_key);
        let held = HeldNotice {
            notice_id: vector.notice_id,
            sealed: vector.sealed,
            expires_at_ms: 1_893_456_000_000,
        };
        let link = secret
            .open(&held, DeviceId::from_uuid(vector.device_id))
            .expect("vector opens");
        assert_eq!(link.kind, SetupLinkKind::Authorization);
        assert_eq!(link.label, "Notion MCP");
        assert_eq!(link.url, "https://provider.example/authorize?state=one");
        assert_eq!(link.host().as_deref(), Some("provider.example"));
        assert_eq!(link.expires_at_ms, 1_893_456_000_000);
        let expected: serde_json::Value =
            serde_json::from_str(&vector.plaintext).expect("plaintext json");
        assert_eq!(expected["url"], link.url);
    }

    #[test]
    fn rejects_wrong_device_notice_key_or_non_https_url_and_redacts_debug() {
        let secret = NoticeSecret::generate().expect("key");
        let round_trip = NoticeSecret::from_scalar(secret.scalar()).expect("from scalar");
        assert_eq!(secret.public_key(), round_trip.public_key());

        let notice_id = Uuid::new_v4();
        let device_id = DeviceId::new();
        let point = secret.public_key().point().expect("point");
        let plaintext = br#"{"kind":"credentials","label":"Google","url":"https://renoa.live/setup#v=1&key=supersecret","expires_at_ms":1893456000000}"#;
        let sealed = seal_for_test(&point, notice_id, device_id, plaintext);
        let held = HeldNotice {
            notice_id,
            sealed: sealed.clone(),
            expires_at_ms: 1_893_456_000_000,
        };

        let link = secret.open(&held, device_id).expect("opens");
        assert_eq!(link.kind, SetupLinkKind::Credentials);
        let debug = format!("{link:?}");
        assert!(
            !debug.contains("supersecret"),
            "debug must redact url: {debug}"
        );
        assert!(debug.contains("<secret>"));

        assert_eq!(secret.open(&held, DeviceId::new()), Err(UnreadableNotice));
        assert_eq!(
            secret.open(
                &HeldNotice {
                    notice_id: Uuid::new_v4(),
                    sealed,
                    expires_at_ms: 1_893_456_000_000,
                },
                device_id,
            ),
            Err(UnreadableNotice)
        );
        let other = NoticeSecret::generate().expect("other");
        assert_eq!(other.open(&held, device_id), Err(UnreadableNotice));

        for bad_url in [
            "http://example.com/setup",
            "javascript:alert(1)",
            "https://user:pass@example.com/setup",
            " https://example.com/setup ",
        ] {
            let bad = serde_json::json!({
                "kind": "authorization",
                "label": "Bad",
                "url": bad_url,
                "expires_at_ms": 1_893_456_000_000_i64,
            });
            let bad_sealed =
                seal_for_test(&point, notice_id, device_id, bad.to_string().as_bytes());
            assert_eq!(
                secret.open(
                    &HeldNotice {
                        notice_id,
                        sealed: bad_sealed,
                        expires_at_ms: 1_893_456_000_000,
                    },
                    device_id,
                ),
                Err(UnreadableNotice),
                "must reject {bad_url}"
            );
        }
    }
}
