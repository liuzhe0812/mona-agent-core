//! Serializable, model-neutral replacement state. Full source messages stay in the host archive.
use crate::{group_messages, project, safe_to_summarize, Group};
use api::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactionState {
    pub version: u32,
    pub source_messages: usize,
    pub ranges: Vec<CompactedRange>,
    pub summary: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactedRange {
    pub start: usize,
    pub end: usize,
    pub sha256: String,
}
fn invalid() -> AgentError {
    AgentError::new(
        ErrorCode::ModelProtocol,
        "compaction state is invalid or its source history changed",
    )
}
fn fingerprint(messages: &[Message]) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(messages).map_err(|_| invalid())?)
    ))
}
impl CompactionState {
    pub(crate) fn capture(groups: &[Group], selected: &[usize], summary: &str) -> Result<Self> {
        crate::TaskSummary::parse(summary)?;
        let mut offset = 0;
        let mut ranges = Vec::new();
        for (index, group) in groups.iter().enumerate() {
            if matches!(group.messages.first(), Some(Message::System { .. })) {
                continue;
            }
            let end = offset + group.messages.len();
            if selected.contains(&index) {
                ranges.push(CompactedRange {
                    start: offset,
                    end,
                    sha256: fingerprint(&group.messages)?,
                });
            }
            offset = end;
        }
        if ranges.len() > 16384 {
            return Err(AgentError::new(
                ErrorCode::Limit,
                "too many compacted source groups",
            ));
        }
        Ok(Self {
            version: 2,
            source_messages: offset,
            ranges,
            summary: summary.into(),
        })
    }
    /// Apply only to the corresponding system-free working transcript and a new settled tail.
    /// Ranges, hashes and whole tool groups are checked; this never rewrites the host archive.
    pub fn project(&self, messages: &[Message]) -> Result<Vec<Message>> {
        if self.version != 2
            || crate::TaskSummary::parse(&self.summary).is_err()
            || self.summary.len() > crate::MAX_SUMMARY_BYTES
            || self.ranges.is_empty()
            || self.ranges.len() > 16384
            || self.source_messages > messages.len()
            || messages.iter().any(|m| matches!(m, Message::System { .. }))
        {
            return Err(invalid());
        }
        let groups = group_messages(messages)?;
        let mut selected = Vec::new();
        let mut offset = 0;
        let mut range = 0;
        for (index, group) in groups.iter().enumerate() {
            let end = offset + group.messages.len();
            if let Some(span) = self.ranges.get(range) {
                if span.start < offset || span.start >= span.end || span.end > self.source_messages
                {
                    return Err(invalid());
                }
                if span.start == offset {
                    if span.end != end
                        || !safe_to_summarize(group)
                        || fingerprint(&group.messages)? != span.sha256
                    {
                        return Err(invalid());
                    }
                    selected.push(index);
                    range += 1;
                } else if span.start < end {
                    return Err(invalid());
                }
            }
            offset = end;
        }
        if range != self.ranges.len() {
            return Err(invalid());
        }
        Ok(project(&groups, &selected, &self.summary))
    }
}
