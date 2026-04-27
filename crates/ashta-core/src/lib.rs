pub mod event;
pub mod symbol;

// Re-exports publics — l'API de la crate
pub use event::{Event, EventKind};
pub use symbol::SymbolId;
