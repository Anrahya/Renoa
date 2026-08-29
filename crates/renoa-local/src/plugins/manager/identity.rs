use sha2::{Digest as _, Sha256};

pub(super) fn integration_id(plugin_digest: &str, server_id: &str) -> String {
    let server_digest = hex(&Sha256::digest(server_id.as_bytes()));
    format!("plugin.{}.{}", &plugin_digest[..24], &server_digest[..24])
}

pub(super) fn default_connection_id(plugin_digest: &str, server_id: &str) -> String {
    format!("{}.default", integration_id(plugin_digest, server_id))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    output
}
