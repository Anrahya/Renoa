//! Values a frame carries that grant access or reach a person, so neither end
//! ever prints them: their `Debug` is redacted and only `expose` reads them.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::DeviceId;

macro_rules! redacted {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            #[must_use]
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            #[must_use]
            pub fn expose(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!(stringify!($name), "([REDACTED])"))
            }
        }
    };
}

redacted!(
    /// A single-use token that enrolls a node or surface.
    EnrollmentToken
);
redacted!(
    /// The bearer half of a device's credentials.
    DeviceCredential
);
redacted!(
    /// A one-use browser bearer secret that opens one connection.
    ConnectionTicket
);
redacted!(
    /// A device's push token as the push provider issued it.
    PushToken
);
redacted!(
    /// A one-time link: a path on the coordinator's origin whose query holds a
    /// ticket.
    ShareLink
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCredentials {
    pub device_id: DeviceId,
    pub credential: DeviceCredential,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_secret_prints_its_value() {
        const VALUE: &str = "0123456789abcdef";
        let printed = [
            format!("{:?}", EnrollmentToken::new(VALUE)),
            format!("{:?}", DeviceCredential::new(VALUE)),
            format!("{:?}", ConnectionTicket::new(VALUE)),
            format!("{:?}", PushToken::new(VALUE)),
            format!("{:?}", ShareLink::new(VALUE)),
            format!(
                "{:?}",
                DeviceCredentials {
                    device_id: DeviceId::new(),
                    credential: DeviceCredential::new(VALUE),
                }
            ),
        ];
        for line in printed {
            assert!(!line.contains(VALUE), "{line}");
            assert!(line.contains("[REDACTED]"), "{line}");
        }
    }
}
