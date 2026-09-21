//! Text/tool-call Chat Completions adapter; no hosted account or credentials bundled.
#![forbid(unsafe_code)]
mod chat;
mod sse;
pub use chat::{ChatConfig, ChatModel, OutputTokenField};
