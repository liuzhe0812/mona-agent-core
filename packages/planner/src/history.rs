//! Recover only native plan-tool results, never assistant prose, summaries or quoted search results.
use crate::{state::invalid, PlanSnapshot, MAX_PLAN_BYTES};
use api::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const PLAN_SEED_KEY: &str = "planner.seed";
pub const PLAN_READ: &str = "plan_read";
pub const PLAN_UPDATE: &str = "plan_update";
pub const PLAN_SUBMIT: &str = "plan_submit";
pub(crate) fn is_plan_tool(name: &str) -> bool {
    matches!(name, PLAN_READ | PLAN_UPDATE | PLAN_SUBMIT)
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PlanRecord {
    pub run_id: String,
    pub call_id: String,
    pub state: PlanSnapshot,
}

/// Binds a validated state to a new Run. The caller owns session authorization and must not
/// accept this metadata from an untrusted client. No live Run, tool permission or history is changed.
pub fn bind_state(request: &mut RunRequest, state: &PlanSnapshot) -> Result<()> {
    state.validate()?;
    let encoded = serde_json::to_string(state).map_err(|_| invalid("plan encoding failed"))?;
    let other_bytes: usize = request
        .metadata
        .iter()
        .filter(|(k, _)| k.as_str() != PLAN_SEED_KEY)
        .map(|(k, v)| k.len().saturating_add(v.len()))
        .sum();
    if other_bytes
        .saturating_add(PLAN_SEED_KEY.len())
        .saturating_add(encoded.len())
        > 16 * 1024
    {
        return Err(invalid("plan seed does not fit the Run metadata budget"));
    }
    request.metadata.insert(PLAN_SEED_KEY.into(), encoded);
    Ok(())
}
pub(crate) fn seed(metadata: &BTreeMap<String, String>) -> Result<Option<PlanSnapshot>> {
    metadata
        .get(PLAN_SEED_KEY)
        .map(|encoded| {
            if encoded.len() > MAX_PLAN_BYTES {
                return Err(invalid("plan seed exceeds limit"));
            }
            let state: PlanSnapshot =
                serde_json::from_str(encoded).map_err(|_| invalid("invalid plan seed"))?;
            state.validate()?;
            Ok(state)
        })
        .transpose()
}
fn calls(messages: &[Message]) -> BTreeMap<&str, &str> {
    messages
        .iter()
        .filter_map(|m| match m {
            Message::Assistant { tool_calls, .. } => Some(tool_calls),
            _ => None,
        })
        .flatten()
        .filter(|c| is_plan_tool(&c.name))
        .map(|c| (c.id.as_str(), c.name.as_str()))
        .collect()
}
fn record(result: &ToolResult, names: &BTreeMap<&str, &str>) -> Result<Option<PlanRecord>> {
    if !names.contains_key(result.call_id.as_str()) || result.status != ToolStatus::Success {
        return Ok(None);
    }
    if result.truncated {
        return Err(invalid("cannot recover a truncated plan result"));
    }
    let value = result
        .structured
        .as_ref()
        .and_then(|v| v.get("planner"))
        .ok_or_else(|| invalid("confirmed plan result is missing its structured snapshot"))?;
    if serde_json::to_vec(value).map_or(true, |v| v.len() > MAX_PLAN_BYTES + 1024) {
        return Err(invalid("plan record exceeds limit"));
    }
    let r: PlanRecord = serde_json::from_value(value.clone())
        .map_err(|_| invalid("invalid plan result snapshot"))?;
    if r.call_id != result.call_id || r.run_id.is_empty() || r.run_id.len() > 256 {
        return Err(invalid("plan result identity does not match its tool call"));
    }
    r.state.validate()?;
    Ok(Some(r))
}
/// Reads a host-authorized, complete history. Use the full archive, not a UI snapshot or summary.
/// `None` means there is no confirmed plan result, not that a plan was successfully cleared.
pub fn recover_history(messages: &[Message]) -> Result<Option<PlanSnapshot>> {
    validate_messages(messages)?;
    let names = calls(messages);
    let mut latest = None;
    for message in messages {
        if let Message::Tool { result } = message {
            if let Some(r) = record(result, &names)? {
                latest = Some(r.state);
            }
        }
    }
    Ok(latest)
}
/// Recover a checkpoint supplied by a trusted, acknowledged sink. Includes settled results in an
/// unfinished batch. Intent/Unknown is never promoted to success; there is no tool replay.
pub fn recover_checkpoint(cp: &RunCheckpoint) -> Result<Option<PlanSnapshot>> {
    if cp.schema_version != CHECKPOINT_VERSION {
        return Err(invalid("unsupported checkpoint version"));
    }
    let names = calls(&cp.transcript);
    let base = seed(&cp.metadata)?;
    let mut latest = base.clone();
    let mut current: BTreeMap<u64, PlanSnapshot> = BTreeMap::new();
    for result in cp
        .transcript
        .iter()
        .filter_map(|m| match m {
            Message::Tool { result } => Some(result),
            _ => None,
        })
        .chain(cp.pending_tools.values().filter_map(|s| match s {
            CheckpointToolState::Settled { result } => Some(result),
            _ => None,
        }))
    {
        let Some(r) = record(result, &names)? else {
            continue;
        };
        if r.run_id == cp.run_id {
            if current
                .get(&r.state.revision)
                .is_some_and(|old| old != &r.state)
            {
                return Err(invalid("conflicting plan snapshots share a revision"));
            }
            current.insert(r.state.revision, r.state);
        } else if base.is_none() {
            latest = Some(r.state);
        }
    }
    for state in current.into_values() {
        if latest
            .as_ref()
            .is_some_and(|old| old.revision > state.revision)
        {
            return Err(invalid("confirmed plan revision predates this Run's seed"));
        }
        if latest
            .as_ref()
            .is_some_and(|old| old.revision == state.revision && old != &state)
        {
            return Err(invalid("plan seed conflicts with the confirmed snapshot"));
        }
        latest = Some(state);
    }
    Ok(latest)
}
