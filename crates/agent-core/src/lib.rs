//! Default ReAct base and a transactional, startup-only plugin host.
#![forbid(unsafe_code)]
mod checkpoint;
mod engine;
mod events;
mod gate;
mod host;
mod model;
mod tools;
mod validation;

pub use engine::Engine;
pub use agent_api::RunHandle;
pub use host::{Host, HostBuilder};
pub use validation::validate_messages;
