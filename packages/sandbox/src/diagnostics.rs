use crate::Backend;
use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Diagnostic {
    RunnerFailure,
    Denied,
    CommandFailure,
}

/// Bounded stderr-only evidence. Classification is guidance, NEVER retry authority.
/// Long/fragmented lines are bounded without treating stdout as launcher evidence.
pub struct Diagnostics {
    backend: Backend,
    line: Vec<u8>,
    runner: bool,
    denied: bool,
}
impl Diagnostics {
    pub fn new(backend: Backend) -> Self {
        Self {
            backend,
            line: Vec::new(),
            runner: false,
            denied: false,
        }
    }
    pub fn push(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if byte == b'\n' {
                self.finish_line();
            } else if self.line.len() < 8192 {
                self.line.push(byte);
            }
        }
    }
    fn finish_line(&mut self) {
        let line = String::from_utf8_lossy(&self.line).trim().to_lowercase();
        let (signature, denial): (&str, &[&str]) = match self.backend {
            Backend::Bubblewrap => ("bwrap: ", &["read-only file system"]),
            Backend::Landlock => ("mona-landlock-run: ", &["permission denied"]),
            Backend::Seatbelt => ("sandbox-exec: ", &["operation not permitted"]),
            Backend::WindowsAcl => (
                "mona-windows-acl-run: ",
                &[
                    "access is denied",
                    "access to the path",
                    "permission denied",
                    "operation not permitted",
                ],
            ),
        };
        if !line.contains(": cleanup:")
            && line != "mona-landlock-run: partial enforcement (older landlock abi)"
        {
            self.runner |= line.contains(signature);
        }
        self.denied |= denial.iter().any(|s| line.contains(s));
        self.line.clear();
    }
    pub fn classify(mut self, code: Option<i32>, success: bool) -> Option<Diagnostic> {
        self.finish_line();
        if success {
            return None;
        }
        let runner_code = match self.backend {
            Backend::Landlock => code == Some(125),
            Backend::WindowsAcl => code == Some(127),
            _ => true,
        };
        Some(if self.runner && runner_code {
            Diagnostic::RunnerFailure
        } else if self.denied {
            Diagnostic::Denied
        } else {
            Diagnostic::CommandFailure
        })
    }
}
