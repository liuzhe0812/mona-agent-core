// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 OpenAI
// Adapted from openai/codex, commit aa380897f67b91e1a47d530d7286d497b6726d3f,
// codex-rs/core/src/agent/control/execution.rs (AgentExecutionLimiter/LocalExecutionPermit).
// Mona modifications: fixed explicit limit, atomic reservation rather than separate
// capacity check/increment, public-runtime-independent errors, and local regression tests.
use api::{AgentError, ErrorCode, Result};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
pub(crate) struct AgentExecutionLimiter {
    active: AtomicUsize,
    max_threads: usize,
}
pub(crate) struct LocalExecutionPermit {
    limiter: Arc<AgentExecutionLimiter>,
}
impl Drop for LocalExecutionPermit {
    fn drop(&mut self) {
        self.limiter.active.fetch_sub(1, Ordering::AcqRel);
    }
}
impl AgentExecutionLimiter {
    pub fn new(max_threads: usize) -> Arc<Self> {
        Arc::new(Self {
            active: AtomicUsize::new(0),
            max_threads,
        })
    }
    pub fn reserve(self: &Arc<Self>) -> Result<LocalExecutionPermit> {
        self.active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < self.max_threads).then_some(n + 1)
            })
            .map_err(|_| {
                AgentError::new(
                    ErrorCode::Limit,
                    "parallel child limit reached; wait for an existing child",
                )
            })?;
        Ok(LocalExecutionPermit {
            limiter: self.clone(),
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reservation_is_atomic_and_drop_releases_capacity() {
        let limiter = AgentExecutionLimiter::new(2);
        let a = limiter.reserve().unwrap();
        let b = limiter.reserve().unwrap();
        assert!(limiter.reserve().is_err());
        drop(a);
        assert!(limiter.reserve().is_ok());
        drop(b);
        assert_eq!(limiter.active.load(Ordering::Acquire), 0);
    }
}
