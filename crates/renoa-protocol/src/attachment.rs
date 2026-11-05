//! A file the principal uploaded to its Host and attached to a command's text.

use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

/// One attached file: the upload that holds its bytes, and the name and media
/// type the surface shows it under. RCP checks the shape only; the executor
/// finds the upload and decides what the agent receives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Fields", rename_all = "camelCase")]
pub struct Attachment {
    upload_id: Uuid,
    name: String,
    mime_type: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Fields {
    upload_id: Uuid,
    name: String,
    mime_type: String,
}

impl TryFrom<Fields> for Attachment {
    type Error = String;

    fn try_from(fields: Fields) -> Result<Self, Self::Error> {
        Self::new(fields.upload_id, fields.name, fields.mime_type)
    }
}

impl Attachment {
    /// The most attachments one command may carry.
    pub const MAX_PER_COMMAND: usize = 10;
    /// The most bytes a name may have: a file name on Linux file systems.
    pub const MAX_NAME_BYTES: usize = 255;
    /// The most bytes a media type may have: a 127-byte type and subtype
    /// (RFC 6838) and the slash between them.
    pub const MAX_MIME_TYPE_BYTES: usize = 255;

    /// # Errors
    ///
    /// Returns what is wrong with an empty or overlong name, a name with a
    /// control character, or a media type that is not `type/subtype` in
    /// printable ASCII.
    pub fn new(upload_id: Uuid, name: String, mime_type: String) -> Result<Self, String> {
        if name.is_empty()
            || name.len() > Self::MAX_NAME_BYTES
            || name.chars().any(char::is_control)
        {
            return Err(format!(
                "an attachment name must be 1 to {} bytes with no control character",
                Self::MAX_NAME_BYTES
            ));
        }
        let valid_type = mime_type.len() <= Self::MAX_MIME_TYPE_BYTES
            && mime_type.bytes().all(|byte| byte.is_ascii_graphic())
            && mime_type.split_once('/').is_some_and(|(kind, subtype)| {
                !kind.is_empty() && !subtype.is_empty() && !subtype.contains('/')
            });
        if !valid_type {
            return Err(format!(
                "an attachment media type must be type/subtype in printable ASCII, at most {} bytes",
                Self::MAX_MIME_TYPE_BYTES
            ));
        }
        Ok(Self {
            upload_id,
            name,
            mime_type,
        })
    }

    #[must_use]
    pub const fn upload_id(&self) -> Uuid {
        self.upload_id
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn mime_type(&self) -> &str {
        &self.mime_type
    }
}

/// Reads a command's attachments, refusing more than
/// [`Attachment::MAX_PER_COMMAND`].
pub(crate) fn bounded<'de, D>(deserializer: D) -> Result<Vec<Attachment>, D::Error>
where
    D: Deserializer<'de>,
{
    let attachments = Vec::<Attachment>::deserialize(deserializer)?;
    if attachments.len() > Attachment::MAX_PER_COMMAND {
        return Err(serde::de::Error::custom(format!(
            "a command carries at most {} attachments",
            Attachment::MAX_PER_COMMAND
        )));
    }
    Ok(attachments)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::CommandInput;

    use super::Attachment;

    fn input(attachments: &serde_json::Value) -> Result<CommandInput, serde_json::Error> {
        serde_json::from_value(json!({
            "type": "text",
            "text": "look",
            "attachments": attachments,
        }))
    }

    fn one(name: &str, mime_type: &str) -> serde_json::Value {
        json!({
            "uploadId": "00000000-0000-0000-0000-000000000001",
            "name": name,
            "mimeType": mime_type,
        })
    }

    #[test]
    fn text_input_without_attachments_keeps_its_old_shape() {
        let old = json!({ "type": "text", "text": "hello" });
        let decoded: CommandInput = serde_json::from_value(old.clone()).expect("old input");
        assert!(decoded.attachments().is_empty());
        assert_eq!(serde_json::to_value(&decoded).expect("encode"), old);
    }

    #[test]
    fn an_attachment_round_trips_in_camel_case() {
        let wire = json!([one("fridge.jpg", "image/jpeg")]);
        let decoded = input(&wire).expect("attachment");
        assert_eq!(decoded.attachments()[0].name(), "fridge.jpg");
        assert_eq!(
            serde_json::to_value(&decoded).expect("encode")["attachments"],
            wire
        );
    }

    #[test]
    fn malformed_attachments_are_refused() {
        let at_most = vec![one("a.txt", "text/plain"); Attachment::MAX_PER_COMMAND];
        input(&json!(at_most)).expect("the most attachments");
        let over = vec![one("a.txt", "text/plain"); Attachment::MAX_PER_COMMAND + 1];
        assert!(input(&json!(over)).is_err());
        let longest = "n".repeat(Attachment::MAX_NAME_BYTES);
        input(&json!([one(&longest, "text/plain")])).expect("the longest name");
        for (name, mime_type) in [
            ("", "text/plain"),
            (&*"n".repeat(Attachment::MAX_NAME_BYTES + 1), "text/plain"),
            ("a\nb", "text/plain"),
            ("a", "text"),
            ("a", "/plain"),
            ("a", "text/"),
            ("a", "text/plain/x"),
            ("a", "text/pla in"),
            (
                "a",
                &*format!("t/{}", "x".repeat(Attachment::MAX_MIME_TYPE_BYTES - 1)),
            ),
        ] {
            assert!(
                input(&json!([one(name, mime_type)])).is_err(),
                "{name:?} {mime_type:?}"
            );
        }
        let mut unknown = one("a", "text/plain");
        unknown["size"] = json!(3);
        assert!(input(&json!([unknown])).is_err());
    }
}
