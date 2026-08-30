//! Durable shared library for immutable Renoa Agent Plugin packages.

mod blob;
mod schema;
mod server;
mod store;

pub use server::Registry;
pub use store::RegistryError;
