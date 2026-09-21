//! Trusted model configuration and opaque provider replay data, NOT UI payloads.
use crate::{AgentError, ErrorCode, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MAX_PROVIDER_DATA_BYTES: usize = 64 * 1024;
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderData {
    /// Adapter-owned namespace. The consuming adapter must explicitly accept it.
    pub namespace: String,
    /// Complete snapshot, not a JSON merge patch. Array order and signatures survive.
    pub value: Value,
}
impl std::fmt::Debug for ProviderData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderData").field("namespace", &self.namespace).field("value", &"<opaque>").finish()
    }
}
impl ProviderData {
    pub fn validate(&self) -> Result<()> {
        if self.namespace.is_empty() || self.namespace.len() > 128
            || !self.namespace.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')) {
            return Err(AgentError::new(ErrorCode::Schema, "invalid provider-data namespace"));
        }
        if serde_json::to_vec(&self.value).map_or(true, |v| v.len() > MAX_PROVIDER_DATA_BYTES) {
            return Err(AgentError::new(ErrorCode::Limit, "provider data exceeds its byte limit"));
        }
        Ok(())
    }
}

/// Small common set. Unsupported fields MUST be rejected by an adapter, not ignored.
/// No credentials, endpoint URLs, tool execution policy or token limit overrides.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ModelOptions {
    pub model: Option<String>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    /// None = inherit; Some([]) = explicitly clear the adapter's stop list.
    pub stop: Option<Vec<String>>,
    pub provider_options: Option<ProviderData>,
}
impl ModelOptions {
    pub fn validate(&self) -> Result<()> {
        if self.model.as_ref().is_some_and(|id| id.is_empty() || id.len() > 512) {
            return Err(AgentError::new(ErrorCode::Configuration, "invalid model selector"));
        }
        if self.temperature.is_some_and(|x| !x.is_finite() || !(0.0..=2.0).contains(&x))
            || self.top_p.is_some_and(|x| !x.is_finite() || x <= 0.0 || x > 1.0) {
            return Err(AgentError::new(ErrorCode::Configuration, "invalid generation sampling options"));
        }
        if self.stop.as_ref().is_some_and(|v| v.len() > 4 || v.iter().any(|s| s.is_empty() || s.len() > 1024)) {
            return Err(AgentError::new(ErrorCode::Configuration, "stop must have at most 4 nonempty bounded strings"));
        }
        if let Some(data) = &self.provider_options { data.validate()?; }
        Ok(())
    }
    pub fn inherit(&self, defaults: &Self) -> Self {
        Self {
            model: self.model.clone().or_else(|| defaults.model.clone()),
            temperature: self.temperature.or(defaults.temperature),
            top_p: self.top_p.or(defaults.top_p),
            stop: self.stop.clone().or_else(|| defaults.stop.clone()),
            provider_options: self.provider_options.clone().or_else(|| defaults.provider_options.clone()),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolTarget { Assistant, ToolCall { index: usize } }
