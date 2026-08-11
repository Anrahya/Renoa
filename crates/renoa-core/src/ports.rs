use std::{future::Future, pin::Pin};

use thiserror::Error;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

macro_rules! port_error {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Error)]
        #[error("{message}")]
        pub struct $name {
            message: String,
        }

        impl $name {
            #[must_use]
            pub fn new(message: impl Into<String>) -> Self {
                Self {
                    message: message.into(),
                }
            }
        }
    };
}

port_error!(ModelError);
port_error!(StoreError);
