//! Explicit model protocol adapters; local execution, no hosted accounts or remote session ownership.
#![forbid(unsafe_code)]
mod chat;
mod sse;
mod config;
mod native;
mod responses;
mod messages;
#[cfg(test)]
mod native_tests;
pub use config::{create_model, ModelCapabilities, Protocol, ProviderConfig};
pub use responses::ResponsesModel;
pub use messages::MessagesModel;
pub use chat::{ChatConfig, ChatModel, OutputTokenField};
