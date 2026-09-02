use std::str;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub(crate) struct SensitiveString {
    bytes: Vec<u8>,
}

impl SensitiveString {
    pub(crate) fn expose(&self) -> &str {
        str::from_utf8(&self.bytes).expect("sensitive string originated as valid UTF-8")
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.bytes.len()
    }
}

impl<'de> Deserialize<'de> for SensitiveString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(|value| Self {
            bytes: value.into_bytes(),
        })
    }
}

impl Serialize for SensitiveString {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.expose())
    }
}

impl Drop for SensitiveString {
    fn drop(&mut self) {
        self.bytes.fill(0);
    }
}
