use crate::invalid;
use api::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelRoute {
    pub provider_id: String,
    pub model_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Role {
    pub description: String,
    pub instructions: String,
    /// None inherits the parent's tool ceiling; an explicit set can only narrow it.
    pub tools: Option<BTreeSet<String>>,
    /// Trusted model identity only. Credentials and endpoints remain with the host.
    pub model: Option<ModelRoute>,
}
impl Default for Role {
    fn default() -> Self {
        Self { description: "General delegated task".into(),
        instructions: "Complete only the delegated task. Report facts, affected files, validation and unresolved issues. Other agents may share this workspace: do not revert their work.".into(), tools: None, model: None }
    }
}
impl Role {
    pub fn validate(&self) -> Result<()> {
        if self.description.len() > 512
            || self.instructions.len() > 8192
            || self
                .tools
                .as_ref()
                .is_some_and(|names| names.len() > 128 || names.iter().any(|n| !name(n)))
            || self.model.as_ref().is_some_and(|m| {
                !name(&m.provider_id)
                    || m.model_id.is_empty()
                    || m.model_id.len() > 512
                    || m.model_id.chars().any(char::is_control)
            })
        {
            return Err(invalid("invalid role instructions, model or tool ceiling"));
        }
        Ok(())
    }
}
pub(crate) fn name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub max_parallel: usize,
    pub max_children: usize,
    pub max_depth: u32,
    pub max_active_parents: usize,
    pub roles: BTreeMap<String, Role>,
}
impl Default for Config {
    fn default() -> Self {
        let explorer = Role {
            description: "Read-only investigation and review".into(),
            tools: Some(
                [
                    "read",
                    "grep",
                    "find",
                    "ls",
                    "memory_read",
                    "session_search",
                    "session_read",
                ]
                .into_iter()
                .map(String::from)
                .collect(),
            ),
            ..Role::default()
        };
        Self {
            max_parallel: 4,
            max_children: 64,
            max_depth: 1,
            max_active_parents: 128,
            roles: BTreeMap::from([
                ("default".into(), Role::default()),
                (
                    "worker".into(),
                    Role {
                        description: "Implement an explicitly owned change and verify it".into(),
                        ..Role::default()
                    },
                ),
                ("explorer".into(), explorer),
            ]),
        }
    }
}
impl Config {
    pub fn validate(&self) -> Result<()> {
        if !(1..=16).contains(&self.max_parallel)
            || !(1..=128).contains(&self.max_children)
            || !(1..=4).contains(&self.max_depth)
            || !(1..=1024).contains(&self.max_active_parents)
            || self.roles.is_empty()
            || self.roles.len() > 16
            || !self.roles.contains_key("default")
            || self.roles.keys().any(|s| !name(s))
        {
            return Err(invalid("invalid subagent capacity or roles"));
        }
        for role in self.roles.values() {
            role.validate()?;
        }
        Ok(())
    }
}
