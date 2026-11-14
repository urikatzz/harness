//! umango core: talks to model backends. Knows nothing about terminals.

pub mod agent;
pub mod audit;
pub mod catalog;
pub mod context;
pub mod error;
pub mod eval;
pub mod interrupt;
pub mod layout;
pub mod memory;
pub mod permissions;
pub mod prompt;
pub mod provider;
pub mod providers;
pub mod session;
pub mod tools;

pub use error::{Error, Result};
pub use layout::Layout;
pub use permissions::Policy;
pub use provider::{Kind, Message, Provider, Reply, Role, Selection};
pub use providers::build_provider;
pub use tools::{Tool, Toolbox};

mod paths;
mod process;
mod sandbox;
#[cfg(test)]
mod test_support;

use std::path::Path;

/// Where this run reads and writes; see [`layout`]. The project is the directory the harness
/// started in. Read once, so later `cd`s or env changes can't move it.
pub fn layout() -> &'static Layout {
    static LAYOUT: std::sync::OnceLock<Layout> = std::sync::OnceLock::new();
    LAYOUT.get_or_init(|| Layout::discover(Path::new(".")))
}
