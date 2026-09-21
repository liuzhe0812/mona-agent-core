//! Optional Tauri 2 adapter. Enable `tauri` for native commands and plugin registration.
//! Without that feature the ack-bounded channel adapter can be tested without a WebView.
#![forbid(unsafe_code)]
mod channel;
pub use channel::*;
#[cfg(feature = "tauri")]
mod native;
#[cfg(feature = "tauri")]
pub use native::init;
