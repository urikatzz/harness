//! uriTheKing core: talks to model backends. Knows nothing about terminals.

pub mod catalog;
pub mod provider;
pub mod providers;

pub use provider::{Message, Provider, Result, Role, Selection};
pub use providers::build_provider;
