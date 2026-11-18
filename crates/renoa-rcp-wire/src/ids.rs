use std::fmt;

use md5::{Digest as _, Md5};
use serde::{Deserialize, Serialize};
use uuid::{Builder, Uuid};

macro_rules! uuid_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        #[allow(
            clippy::new_without_default,
            reason = "identity creation should remain explicit at call sites"
        )]
        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            #[must_use]
            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            #[must_use]
            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

uuid_id!(NodeId);
uuid_id!(DeviceId);
uuid_id!(TaskEventId);
uuid_id!(TaskId);

/// The task holding the owner's conversation with the agent `target` on
/// `node_id`: what the owner's apps open for that chat, and where a message
/// from another agent is delivered. It is the name-based UUID (MD5, version 3)
/// of `"renoa app chat v1\n{node_id}\n{target}"`, the same as Java's
/// `UUID.nameUUIDFromBytes`. The Android app derives it in Kotlin
/// (`chatTaskId`); a test there and one here pin the same vector.
#[must_use]
pub fn app_chat_task_id(node_id: NodeId, target: &str) -> TaskId {
    let name = format!("renoa app chat v1\n{node_id}\n{target}");
    let digest: [u8; 16] = Md5::digest(name.as_bytes()).into();
    TaskId::from_uuid(Builder::from_md5_bytes(digest).into_uuid())
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{NodeId, app_chat_task_id};

    /// The same vector as `ChatJournalTest` in the Android app.
    #[test]
    fn the_app_chat_task_id_matches_the_android_app() {
        let node = NodeId::from_uuid(
            Uuid::parse_str("00000000-0000-4000-8000-000000000001").expect("node id"),
        );
        assert_eq!(
            app_chat_task_id(node, "renoa").to_string(),
            "0ae02263-c173-3623-8a84-ed64ab6d1fbf"
        );
        assert_ne!(
            app_chat_task_id(node, "renoa"),
            app_chat_task_id(node, "kaede")
        );
    }
}
