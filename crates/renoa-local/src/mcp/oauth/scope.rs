pub(crate) const MAX_OAUTH_SCOPE_BYTES: usize = 4 * 1_024;

pub(crate) fn validate_oauth_scope(value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > MAX_OAUTH_SCOPE_BYTES {
        return Err(format!(
            "OAuth scope must be 1-{MAX_OAUTH_SCOPE_BYTES} bytes"
        ));
    }
    let mut token_is_empty = true;
    for byte in value.bytes() {
        if byte == b' ' {
            if token_is_empty {
                return Err("OAuth scope must contain single-space-separated tokens".to_owned());
            }
            token_is_empty = true;
            continue;
        }
        if !(byte == 0x21 || (0x23..=0x5b).contains(&byte) || (0x5d..=0x7e).contains(&byte)) {
            return Err("OAuth scope contains a character forbidden by RFC 6749".to_owned());
        }
        token_is_empty = false;
    }
    if token_is_empty {
        return Err("OAuth scope must not end with a separator".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_oauth_scope;

    #[test]
    fn accepts_url_and_provider_style_scope_tokens() {
        validate_oauth_scope(
            "tweet.read tweet.write offline.access https://www.googleapis.com/auth/drive.file",
        )
        .expect("valid OAuth scopes");
    }

    #[test]
    fn rejects_ambiguous_or_non_protocol_scope_text() {
        for invalid in [
            "",
            " read",
            "read ",
            "read  write",
            "read\nwrite",
            "read\\write",
        ] {
            assert!(
                validate_oauth_scope(invalid).is_err(),
                "accepted {invalid:?}"
            );
        }
    }
}
