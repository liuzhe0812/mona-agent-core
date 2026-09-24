//! Bounded curated Markdown memory. No Runtime, Sessions, model calls or background worker.
#![forbid(unsafe_code)]
mod plugin;
mod store;
pub use plugin::{Binding, MemoryPlugin};
pub use store::{
    Backend, Change, Entry, Error, ErrorCode, FileStore, InMemoryStore, Limits, Operation, Origin,
    Result, Snapshot,
};
