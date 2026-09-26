//! Local file-effect confinement. DSH-compatible policies, independent of Agent and Web.
//! Native Windows calls are isolated in `windows`; the public surface is safe Rust.
#![deny(unsafe_op_in_unsafe_fn)]

mod diagnostics;
mod local;
mod policy;
pub mod profiles;
pub mod runner;
#[cfg(windows)]
mod windows;

pub use diagnostics::{Diagnostic, Diagnostics};
pub use local::{Backend, BackendInfo, CommandPlan, Enforcement, LocalSandbox, Runner};
pub use policy::{canonical_target, Mode, Policy};
use serde::Serialize;
use std::fmt;
pub use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    FsSandboxDenied,
    SandboxUnavailable,
    SandboxConfiguration,
    SandboxCancelled,
    SandboxCapacity,
}
#[derive(Clone, Debug, Serialize)]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
}
impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
    pub(crate) fn unavailable(message: impl fmt::Display) -> Self {
        Self::new(ErrorCode::SandboxUnavailable, message.to_string())
    }
    pub(crate) fn config(message: impl fmt::Display) -> Self {
        Self::new(ErrorCode::SandboxConfiguration, message.to_string())
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let code = match self.code {
            ErrorCode::FsSandboxDenied => "FS_SANDBOX_DENIED",
            ErrorCode::SandboxUnavailable => "SANDBOX_UNAVAILABLE",
            ErrorCode::SandboxConfiguration => "SANDBOX_CONFIGURATION",
            ErrorCode::SandboxCancelled => "SANDBOX_CANCELLED",
            ErrorCode::SandboxCapacity => "SANDBOX_CAPACITY",
        };
        write!(f, "{code}: {}", self.message)
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
pub(crate) fn check_cancel(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(Error::new(
            ErrorCode::SandboxCancelled,
            "sandbox preparation cancelled",
        ))
    } else {
        Ok(())
    }
}
