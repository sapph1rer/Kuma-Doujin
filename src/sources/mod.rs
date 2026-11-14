//! Source provider interfaces and the registry. Concrete adapters go in `adapters/`
//! once each provider's terms have been verified (see ROADMAP.md).

pub mod adapters;
pub mod health;
pub mod registry;
pub mod traits;
