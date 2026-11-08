//! uriTheKing core: talks to model backends. Knows nothing about terminals.

pub mod agent;
pub mod catalog;
pub mod context;
pub mod permissions;
pub mod provider;
pub mod providers;
pub mod session;
pub mod toolbox;

pub use provider::{Message, Provider, Result, Role, Selection};
pub use providers::build_provider;
pub use toolbox::Toolbox;
pub use permissions::Policy;
