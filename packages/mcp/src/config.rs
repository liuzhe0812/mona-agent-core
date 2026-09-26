use crate::{bounded, error};
use api::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub servers: BTreeMap<String, Server>,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Server {
    #[serde(default)]
    pub enabled: bool,
    pub transport: TransportConfig,
    #[serde(default = "timeout")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub allow_tools: Option<BTreeSet<String>>,
    #[serde(default)]
    pub deny_tools: BTreeSet<String>,
    /// Host declaration, NOT inferred from an untrusted readOnlyHint.
    #[serde(default)]
    pub read_only_tools: BTreeSet<String>,
}
fn timeout() -> u64 {
    60_000
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TransportConfig {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
        #[serde(default)]
        cwd: Option<String>,
    },
    StreamableHttp {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
        #[serde(default)]
        allow_http: bool,
    },
}
fn name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}
impl Config {
    pub fn validate(&self) -> Result<()> {
        bounded(self, 256 * 1024, "configuration")?;
        if self.servers.len() > 16 {
            return Err(error("MCP supports at most 16 configured servers"));
        }
        for (id, server) in &self.servers {
            if !name(id) {
                return Err(error(
                    "MCP server id must be 1..32 ASCII letters, digits, _ or -",
                ));
            }
            server.validate()?;
        }
        Ok(())
    }
}
impl Server {
    pub fn permits(&self, raw: &str) -> bool {
        self.allow_tools.as_ref().is_none_or(|s| s.contains(raw)) && !self.deny_tools.contains(raw)
    }
    pub fn validate(&self) -> Result<()> {
        if !(100..=300_000).contains(&self.timeout_ms) {
            return Err(error("MCP timeout_ms must be 100..300000"));
        }
        for names in [
            self.allow_tools.as_ref(),
            Some(&self.deny_tools),
            Some(&self.read_only_tools),
        ]
        .into_iter()
        .flatten()
        {
            if names.len() > 128
                || names
                    .iter()
                    .any(|s| s.is_empty() || s.len() > 256 || s.chars().any(char::is_control))
            {
                return Err(error("invalid MCP tool filter"));
            }
        }
        match &self.transport {
            TransportConfig::Stdio {
                command,
                args,
                env,
                cwd,
            } => {
                if command.is_empty()
                    || command.len() > 4096
                    || command.chars().any(char::is_control)
                    || args.len() > 128
                    || args.iter().any(|a| a.len() > 8192 || a.contains('\0'))
                {
                    return Err(error("invalid MCP executable or arguments"));
                }
                if cwd.as_ref().is_some_and(|s| {
                    s.len() > 4096 || s.contains('\0') || !std::path::Path::new(s).is_absolute()
                }) {
                    return Err(error("MCP cwd must be an explicit absolute directory"));
                }
                if env.len() > 64
                    || env.iter().any(|(k, v)| {
                        k.is_empty()
                            || k.len() > 128
                            || k.contains(['=', '\0'])
                            || v.len() > 8192
                            || v.contains('\0')
                    })
                {
                    return Err(error("invalid MCP environment override"));
                }
            }
            TransportConfig::StreamableHttp {
                url,
                headers,
                allow_http,
            } => {
                let u = url::Url::parse(url).map_err(|_| error("invalid MCP endpoint"))?;
                let loopback = matches!(
                    u.host_str(),
                    Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
                );
                if url.len() > 4096
                    || u.host_str().is_none()
                    || !u.username().is_empty()
                    || u.password().is_some()
                    || u.fragment().is_some()
                    || u.query().is_some()
                    || (u.scheme() != "https"
                        && !(u.scheme() == "http" && (*allow_http || loopback)))
                {
                    return Err(error("MCP endpoint must be HTTPS (or explicitly allowed HTTP), without URL credentials, query or fragment; use headers for credentials"));
                }
                if headers.len() > 32 {
                    return Err(error("too many MCP headers"));
                }
                let mut seen = BTreeSet::new();
                for (k, v) in headers {
                    let lower = k.to_ascii_lowercase();
                    if !seen.insert(lower.clone())
                        || matches!(
                            lower.as_str(),
                            "host"
                                | "content-length"
                                | "transfer-encoding"
                                | "connection"
                                | "accept"
                                | "content-type"
                                | "mcp-session-id"
                                | "mcp-protocol-version"
                        )
                        || lower.starts_with("mcp-")
                        || http::HeaderName::try_from(k.as_str()).is_err()
                        || v.len() > 8192
                        || http::HeaderValue::try_from(v.as_str()).is_err()
                    {
                        return Err(error("invalid or protocol-owned MCP header"));
                    }
                }
            }
        }
        Ok(())
    }
}
/// Deterministic server-qualified names, with an identity hash when normalization is lossy.
pub fn public_name(server: &str, raw: &str) -> String {
    let original = format!("mcp__{server}__{raw}");
    if original.len() <= 64
        && original
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return original;
    }
    let normalized: String = original
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(50)
        .collect();
    format!(
        "{normalized}__{}",
        &format!("{:x}", Sha256::digest(original.as_bytes()))[..12]
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_are_bounded_stable_and_collision_resistant() {
        assert_eq!(public_name("a", "search"), "mcp__a__search");
        assert_ne!(public_name("a", "a.b"), public_name("a", "a_b"));
        assert_ne!(public_name("a", "é"), public_name("a", "_"));
        assert!(public_name("a", &"x".repeat(200)).len() <= 64);
    }
    #[test]
    fn credentials_and_protocol_headers_cannot_hide_in_endpoints() {
        for url in [
            "https://key@example.com/mcp",
            "https://example.com/mcp?key=secret",
            "http://example.com/mcp",
        ] {
            let c:Config=serde_json::from_value(serde_json::json!({"servers":{"a":{"transport":{"type":"streamable-http","url":url}}}})).unwrap();
            assert!(c.validate().is_err());
        }
    }
}
