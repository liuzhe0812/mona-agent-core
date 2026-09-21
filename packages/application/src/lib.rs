//! Transport-independent application API. One instance per authority/data-isolation domain.
//! Does not depend on runtime, providers, Axum, or Tauri.
#![forbid(unsafe_code)]
mod protocol;
mod service;
mod subscription;

pub use protocol::*;
pub use service::{AgentApplication, ApplicationConfig};
pub use subscription::Subscription;
