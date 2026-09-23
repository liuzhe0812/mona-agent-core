//! Pi-inspired filesystem and shell tools for the formal Agent host.
#![forbid(unsafe_code)]

mod capture;
mod edit;
mod archive;
mod read;
mod search;
mod shell;
mod support;
mod write;

pub use archive::OutputArchive;
pub use edit::EditTool;
pub use read::{ReadExtension, ReadTool};
pub use search::{FindTool, GrepTool, LsTool};
pub use shell::{ShellConfig, ShellKind, ShellTool};
pub use write::WriteTool;

use api::Tool;
use std::{path::PathBuf, sync::Arc};

#[derive(Clone)]
pub struct ToolConfig {
    pub cwd: PathBuf,
    pub shell: ShellConfig,
    pub max_read_bytes: usize,
    pub max_read_lines: usize,
    pub max_command_bytes: usize,
    pub max_search_results: usize,
    pub read_extensions: Vec<Arc<dyn ReadExtension>>,
    /// Optional output-retention capability supplied together with its read extension.
    pub output_archive: Option<Arc<dyn OutputArchive>>,
}

impl ToolConfig {
    pub fn new(cwd: impl Into<PathBuf>, shell: impl Into<ShellConfig>) -> Self {
        Self {
            cwd: cwd.into(),
            shell: shell.into(),
            max_read_bytes: 50 * 1024,
            max_read_lines: 2_000,
            max_command_bytes: 50 * 1024,
            max_search_results: 1_000,
            read_extensions: Vec::new(),
            output_archive: None,
        }
    }
}

pub fn core_tools(config: &ToolConfig) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(ReadTool::new(config.clone())),
        Arc::new(ShellTool::new(config.clone())),
        Arc::new(EditTool::new(config.clone())),
        Arc::new(WriteTool::new(config.clone())),
    ]
}

pub fn optional_tool(name: &str, config: &ToolConfig) -> Option<Arc<dyn Tool>> {
    match name {
        "grep" => Some(Arc::new(GrepTool::new(config.clone()))),
        "find" => Some(Arc::new(FindTool::new(config.clone()))),
        "ls" => Some(Arc::new(LsTool::new(config.clone()))),
        _ => None,
    }
}
