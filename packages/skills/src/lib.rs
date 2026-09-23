//! Progressive skill discovery and loading, independent of the execution engine.
#![forbid(unsafe_code)]

mod catalog;
mod filesystem;
pub use catalog::{SkillCatalogTransform, SkillsPlugin};
pub use filesystem::LocalSkills;

use api::{async_trait, AgentError, CancellationToken, ErrorCode, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub const SKILLS_SERVICE: &str = "skills.registry";

#[derive(Clone, Debug)]
pub struct Limits {
    pub max_skills: usize,
    pub max_entries_per_root: usize,
    pub max_file_bytes: usize,
    pub max_content_bytes: usize,
    pub max_catalog_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_skills: 64,
            max_entries_per_root: 1024,
            max_file_bytes: 64 * 1024,
            max_content_bytes: 32 * 1024,
            max_catalog_bytes: 32 * 1024,
        }
    }
}
impl Limits {
    pub fn validate(&self) -> Result<()> {
        if self.max_skills == 0
            || self.max_entries_per_root == 0
            || self.max_file_bytes == 0
            || self.max_content_bytes == 0
            || self.max_catalog_bytes == 0
        {
            return Err(error(
                ErrorCode::Configuration,
                "skill limits must be positive",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
    pub model_invocable: bool,
    pub user_invocable: bool,
    /// Model-readable entrypoint selected by the provider. Local providers use
    /// an absolute `SKILL.md` path; non-filesystem providers may omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SkillDefinition {
    pub summary: SkillSummary,
    pub content: String,
}

/// A provider owns lookup and optional resource access. Providers are queried in host order.
#[async_trait]
pub trait SkillProvider: Send + Sync {
    fn id(&self) -> &str;
    async fn list(&self, cancel: CancellationToken) -> Result<Vec<SkillSummary>>;
    async fn load(&self, name: &str, cancel: CancellationToken) -> Result<Option<SkillDefinition>>;
    async fn read_resource(
        &self,
        _name: &str,
        _path: &str,
        _cancel: CancellationToken,
    ) -> Result<String> {
        Err(error(
            ErrorCode::Unsupported,
            "this skill provider does not expose local resources",
        ))
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct CatalogEntry {
    pub provider: String,
    #[serde(flatten)]
    pub summary: SkillSummary,
}

pub struct SkillRegistry {
    providers: Vec<Arc<dyn SkillProvider>>,
    limits: Limits,
}
impl SkillRegistry {
    pub fn new(providers: Vec<Arc<dyn SkillProvider>>, limits: Limits) -> Result<Self> {
        limits.validate()?;
        let mut names = BTreeSet::new();
        for provider in &providers {
            if !valid_name(provider.id()) || !names.insert(provider.id().to_owned()) {
                return Err(error(
                    ErrorCode::Configuration,
                    "invalid or duplicate skill provider ID",
                ));
            }
        }
        Ok(Self { providers, limits })
    }

    /// Invocation-neutral catalog. First provider wins duplicate names, including disabled entries.
    pub async fn list(&self, cancel: CancellationToken) -> Result<Vec<CatalogEntry>> {
        let mut entries = BTreeMap::new();
        for provider in &self.providers {
            check_cancel(&cancel)?;
            let summaries = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(cancelled()),
                result = provider.list(cancel.clone()) => result?,
            };
            for summary in summaries {
                validate_summary(&summary)?;
                entries
                    .entry(summary.name.clone())
                    .or_insert_with(|| CatalogEntry {
                        provider: provider.id().into(),
                        summary,
                    });
                if entries.len() > self.limits.max_skills {
                    return Err(error(
                        ErrorCode::Limit,
                        "skill catalog exceeds its entry limit",
                    ));
                }
            }
        }
        check_cancel(&cancel)?;
        Ok(entries.into_values().collect())
    }

    /// Trusted, invocation-neutral load; model consumers must separately enforce model_invocable.
    pub async fn load(&self, name: &str, cancel: CancellationToken) -> Result<SkillDefinition> {
        let entry = self.find(name, cancel.clone()).await?;
        self.load_entry(&entry, cancel).await
    }

    async fn load_entry(
        &self,
        entry: &CatalogEntry,
        cancel: CancellationToken,
    ) -> Result<SkillDefinition> {
        let name = &entry.summary.name;
        let provider = self.provider(&entry.provider);
        let skill = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(cancelled()),
            result = provider.load(name, cancel.clone()) => result?,
        }
        .ok_or_else(|| error(ErrorCode::Tool, "skill is unknown or no longer available"))?;
        validate_summary(&skill.summary)?;
        if &skill.summary.name != name {
            return Err(error(
                ErrorCode::Tool,
                "skill name changed; list skills again",
            ));
        }
        if skill.content.len() > self.limits.max_content_bytes {
            return Err(error(
                ErrorCode::Limit,
                "skill instructions exceed the content limit",
            ));
        }
        check_cancel(&cancel)?;
        Ok(skill)
    }

    pub async fn load_for_model(
        &self,
        name: &str,
        cancel: CancellationToken,
    ) -> Result<SkillDefinition> {
        let entry = self.find(name, cancel.clone()).await?;
        model_allowed(&entry.summary)?;
        let skill = self.load_entry(&entry, cancel).await?;
        model_allowed(&skill.summary)?;
        Ok(skill)
    }

    pub async fn read_resource_for_model(
        &self,
        name: &str,
        path: &str,
        cancel: CancellationToken,
    ) -> Result<String> {
        let entry = self.find(name, cancel.clone()).await?;
        model_allowed(&entry.summary)?;
        let provider = self.provider(&entry.provider);
        // Re-read policy before handing resource access to the same selected provider.
        let definition = self.load_entry(&entry, cancel.clone()).await?;
        model_allowed(&definition.summary)?;
        let content = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(cancelled()),
            result = provider.read_resource(name, path, cancel.clone()) => result?,
        };
        if content.len() > self.limits.max_content_bytes {
            return Err(error(
                ErrorCode::Limit,
                "skill resource exceeds the content limit",
            ));
        }
        check_cancel(&cancel)?;
        Ok(content)
    }

    async fn find(&self, name: &str, cancel: CancellationToken) -> Result<CatalogEntry> {
        if !valid_name(name) {
            return Err(error(
                ErrorCode::Tool,
                "invalid skill name; use the exact name from the catalog",
            ));
        }
        self.list(cancel)
            .await?
            .into_iter()
            .find(|s| s.summary.name == name)
            .ok_or_else(|| error(ErrorCode::Tool, "skill is unknown or no longer available"))
    }
    fn provider(&self, id: &str) -> &Arc<dyn SkillProvider> {
        self.providers
            .iter()
            .find(|p| p.id() == id)
            .expect("catalog provider is owned by this immutable registry")
    }
    pub fn limits(&self) -> &Limits {
        &self.limits
    }
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}
pub(crate) fn validate_summary(summary: &SkillSummary) -> Result<()> {
    if !valid_name(&summary.name)
        || summary.description.trim().is_empty()
        || summary.description.len() > 4096
    {
        return Err(error(
            ErrorCode::Schema,
            "skill requires a kebab-case name (1..64 bytes) and description (1..4096 bytes)",
        ));
    }
    Ok(())
}
pub(crate) fn model_allowed(summary: &SkillSummary) -> Result<()> {
    if summary.model_invocable {
        Ok(())
    } else {
        Err(error(
            ErrorCode::Policy,
            "skill is not available for model invocation",
        ))
    }
}
pub(crate) fn error(code: ErrorCode, message: &str) -> AgentError {
    AgentError::new(code, message)
}
pub(crate) fn cancelled() -> AgentError {
    error(ErrorCode::Cancelled, "skill operation cancelled")
}
pub(crate) fn check_cancel(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(cancelled())
    } else {
        Ok(())
    }
}
