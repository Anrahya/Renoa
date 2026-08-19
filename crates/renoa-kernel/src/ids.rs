use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! kernel_id {
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
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

kernel_id!(AgentId);
kernel_id!(SessionId);
kernel_id!(CommandId);
kernel_id!(OperationId);
kernel_id!(EffectId);
kernel_id!(EventId);
