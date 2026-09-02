use renoa_credential_relay_protocol::CredentialRelayKind;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use serde::{Deserialize, Serialize};

use super::state::CredentialSetupState;
use crate::mcp::{
    McpCredentialError, McpHostError,
    oauth::{SensitiveString, secret_store::validate_issuer},
};

const MAX_SECRET_BYTES: usize = 16 * 1024;
const MAX_PLAINTEXT_BYTES: usize = 64 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApiTokenInput {
    schema_version: u32,
    value: SensitiveString,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OAuthClientInput {
    schema_version: u32,
    issuer: String,
    client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_secret: Option<SensitiveString>,
}

pub(super) fn decrypt_and_validate(
    state: &CredentialSetupState,
    nonce: &str,
    ciphertext: &str,
) -> Result<Vec<u8>, McpHostError> {
    let key_bytes = decode_array::<32>(state.key.expose())?;
    let nonce = decode_array::<12>(nonce)?;
    let mut encrypted = decode(ciphertext)?;
    if encrypted.len() > MAX_PLAINTEXT_BYTES + AES_256_GCM.tag_len() {
        encrypted.fill(0);
        return Err(McpCredentialError::SetupInvalid.into());
    }
    let key = UnboundKey::new(&AES_256_GCM, &key_bytes)
        .map(LessSafeKey::new)
        .map_err(|_| McpCredentialError::SetupInvalid)?;
    let aad = aad(state);
    let plaintext = key
        .open_in_place(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(&aad),
            &mut encrypted,
        )
        .map_err(|_| McpCredentialError::SetupInvalid)?;
    let result = match state.kind {
        CredentialRelayKind::ApiToken => validate_api_token(plaintext),
        CredentialRelayKind::OAuthClient => validate_oauth_client(plaintext),
    };
    encrypted.fill(0);
    result
}

fn validate_api_token(plaintext: &[u8]) -> Result<Vec<u8>, McpHostError> {
    let input = serde_json::from_slice::<ApiTokenInput>(plaintext)
        .map_err(|_| McpCredentialError::SetupInvalid)?;
    let value = input.value.expose();
    if input.schema_version != 1
        || value.is_empty()
        || value.len() > MAX_SECRET_BYTES
        || !value.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(McpCredentialError::SetupInvalid.into());
    }
    Ok(value.as_bytes().to_vec())
}

fn validate_oauth_client(plaintext: &[u8]) -> Result<Vec<u8>, McpHostError> {
    let mut input = serde_json::from_slice::<OAuthClientInput>(plaintext)
        .map_err(|_| McpCredentialError::SetupInvalid)?;
    if input.schema_version != 1
        || input.client_id.is_empty()
        || input.client_id.len() > MAX_SECRET_BYTES
        || input
            .client_secret
            .as_ref()
            .is_some_and(|secret| secret.is_empty() || secret.len() > MAX_SECRET_BYTES)
    {
        return Err(McpCredentialError::SetupInvalid.into());
    }
    input.issuer = validate_issuer(&input.issuer).map_err(|_| McpCredentialError::SetupInvalid)?;
    serde_json::to_vec(&input).map_err(McpHostError::from)
}

fn aad(state: &CredentialSetupState) -> Vec<u8> {
    format!(
        "renoa credential relay v1\0{}\0{}\0{}",
        state.relay_id,
        state.credential_id,
        match state.kind {
            CredentialRelayKind::ApiToken => "api_token",
            CredentialRelayKind::OAuthClient => "oauth_client",
        }
    )
    .into_bytes()
}

fn decode_array<const N: usize>(value: &str) -> Result<[u8; N], McpHostError> {
    let decoded = decode(value)?;
    decoded.try_into().map_err(|mut bytes: Vec<u8>| {
        bytes.fill(0);
        McpCredentialError::SetupInvalid.into()
    })
}

fn decode(value: &str) -> Result<Vec<u8>, McpHostError> {
    if value.is_empty() || !value.len().is_multiple_of(2) {
        return Err(McpCredentialError::SetupInvalid.into());
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    for pair in value.as_bytes().chunks_exact(2) {
        let high = digit(pair[0]).ok_or(McpCredentialError::SetupInvalid)?;
        let low = digit(pair[1]).ok_or(McpCredentialError::SetupInvalid)?;
        bytes.push((high << 4) | low);
    }
    Ok(bytes)
}

fn digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use renoa_credential_relay_protocol::CredentialRelayKind;
    use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};

    use super::{aad, decode_array, decrypt_and_validate};
    use crate::mcp::credential_setup::state::CredentialSetupState;

    #[test]
    fn browser_format_round_trips_api_tokens_and_oauth_clients() {
        for (kind, plaintext, expected) in [
            (
                CredentialRelayKind::ApiToken,
                br#"{"schema_version":1,"value":"exa-secret"}"#.as_slice(),
                b"exa-secret".as_slice(),
            ),
            (
                CredentialRelayKind::OAuthClient,
                br#"{"schema_version":1,"issuer":"https://accounts.example/","client_id":"client-one","client_secret":"client-secret"}"#.as_slice(),
                br#"{"schema_version":1,"issuer":"https://accounts.example","client_id":"client-one","client_secret":"client-secret"}"#.as_slice(),
            ),
        ] {
            let state = CredentialSetupState::new("credential.one".to_owned(), kind)
                .expect("create setup state");
            let nonce = [7_u8; 12];
            let mut ciphertext = plaintext.to_vec();
            let key = LessSafeKey::new(
                UnboundKey::new(
                    &AES_256_GCM,
                    &decode_array::<32>(state.key.expose()).expect("decode generated key"),
                )
                .expect("create encryption key"),
            );
            key.seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(aad(&state)),
                &mut ciphertext,
            )
            .expect("encrypt browser payload");
            let actual = decrypt_and_validate(&state, &hex(&nonce), &hex(&ciphertext))
                .expect("decrypt browser payload");
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn ciphertext_is_bound_to_its_relay_and_credential() {
        let state =
            CredentialSetupState::new("credential.one".to_owned(), CredentialRelayKind::ApiToken)
                .expect("create first setup state");
        let different =
            CredentialSetupState::new("credential.two".to_owned(), CredentialRelayKind::ApiToken)
                .expect("create second setup state");
        let nonce = [9_u8; 12];
        let mut ciphertext = br#"{"schema_version":1,"value":"secret"}"#.to_vec();
        let key = LessSafeKey::new(
            UnboundKey::new(
                &AES_256_GCM,
                &decode_array::<32>(state.key.expose()).expect("decode generated key"),
            )
            .expect("create encryption key"),
        );
        key.seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(aad(&state)),
            &mut ciphertext,
        )
        .expect("encrypt browser payload");
        assert!(decrypt_and_validate(&different, &hex(&nonce), &hex(&ciphertext)).is_err());
    }

    fn hex(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut encoded = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        encoded
    }
}
