//! Conversation archive and bounded model workset are separate; neither is a UI snapshot.
use super::*;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RunBase {
    pub input_messages: usize,
    pub input_sha256: String,
    pub archive_sha256: String,
    // Needed only between durable admission and the first acknowledged checkpoint.
    pub pending_history: Option<Vec<Message>>,
}
fn digest(messages: &[Message]) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(messages).map_err(io_error)?)
    ))
}
impl Document {
    pub(super) fn working_history(&self) -> Result<Vec<Message>> {
        let mut messages = self
            .body
            .checkpoint
            .as_ref()
            .map(|cp| cp.transcript.clone())
            .or_else(|| {
                self.body
                    .base
                    .as_ref()
                    .and_then(|base| base.pending_history.clone())
            })
            .unwrap_or_default();
        if let Some(cp) = &self.body.checkpoint {
            let settled: BTreeSet<_> = messages
                .iter()
                .filter_map(|m| match m {
                    Message::Tool { result } => Some(result.call_id.clone()),
                    _ => None,
                })
                .collect();
            let missing: Vec<_> = messages
                .iter()
                .flat_map(|m| match m {
                    Message::Assistant { tool_calls, .. } => tool_calls
                        .iter()
                        .filter(|c| !settled.contains(&c.id))
                        .cloned()
                        .collect(),
                    _ => Vec::new(),
                })
                .collect();
            for call in missing {
                let result = match cp.pending_tools.get(&call.id) {
                    Some(CheckpointToolState::Settled { result }) if result.call_id == call.id => result.clone(),
                    Some(CheckpointToolState::IntentRecorded) => ToolResult::new(&call.id, ToolStatus::Unknown,
                        "Previous process stopped after durable tool intent. External effects may have occurred; inspect before any new action. Not automatically replayed."),
                    Some(CheckpointToolState::Pending) => ToolResult::new(&call.id, ToolStatus::Skipped,
                        "Previous process stopped before durable tool intent; this call was not dispatched."),
                    _ => return Err(corrupt()),
                };
                messages.push(Message::Tool { result });
            }
        }
        messages.retain(|message| !matches!(message, Message::System { .. }));
        if self.body.base.is_some() {
            if self.body.checkpoint.is_none() {
                let turn = self.body.turns.last().ok_or_else(corrupt)?;
                messages.push(Message::user(&turn.prompt));
            }
        } else {
            // Read-only migration of version-1 full-transcript conversations.
            let next = self.body.checkpoint_turn.map_or(0, |n| n + 1);
            for turn in self.body.turns.iter().skip(next) {
                messages.push(Message::user(&turn.prompt));
            }
        }
        if !messages.is_empty() {
            runtime::validate_messages(&messages).map_err(|_| corrupt())?;
        }
        Ok(messages)
    }
    /// Reconstruct full acknowledged facts. Summaries never overwrite this archive.
    pub fn history(&self) -> Result<Vec<Message>> {
        let working = self.working_history()?;
        if let Some(base) = &self.body.base {
            let prefix = working.get(..base.input_messages).ok_or_else(corrupt)?;
            if digest(prefix)? != base.input_sha256
                || digest(&self.body.archive)? != base.archive_sha256
                || self
                    .body
                    .archive
                    .iter()
                    .any(|m| matches!(m, Message::System { .. }))
            {
                return Err(corrupt());
            }
            let mut archive = self.body.archive.clone();
            archive.extend_from_slice(&working[base.input_messages..]);
            if !archive.is_empty() {
                runtime::validate_messages(&archive).map_err(|_| corrupt())?;
            }
            Ok(archive)
        } else {
            if !self.body.archive.is_empty() {
                return Err(corrupt());
            }
            Ok(working)
        }
    }
    fn next_workset(&self, use_compaction: bool) -> Result<Vec<Message>> {
        if !use_compaction {
            return self.history();
        }
        let working = self.working_history()?;
        #[cfg(feature = "compaction")]
        if let Some(value) = &self.body.compaction {
            let state: compaction::CompactionState =
                serde_json::from_value(value.clone()).map_err(|_| corrupt())?;
            return state.project(&working).map_err(|_| corrupt());
        }
        Ok(working)
    }
    pub(super) fn begin_workset(
        &mut self,
        use_compaction: bool,
        prompt: &str,
    ) -> Result<Vec<Message>> {
        let history = self.next_workset(use_compaction)?;
        let bytes = serde_json::to_vec(&history)
            .map_err(io_error)?
            .len()
            .saturating_add(
                serde_json::to_vec(&Message::user(prompt))
                    .map_err(io_error)?
                    .len(),
            )
            .saturating_add(1);
        // The formal host uses the API default admission bound. Reject BEFORE replacing
        // a usable saved workset (notably when compaction was temporarily disabled).
        if bytes > RunLimits::default().max_initial_history_bytes {
            return Err(error(Code::Capacity, "模型工作上下文超过本地宿主的 4 MiB 接纳上限；旧记录与摘要状态未改动，请启用压缩或新建会话。"));
        }
        let archive = self.history()?;
        self.body.base = Some(RunBase {
            input_messages: history.len(),
            input_sha256: digest(&history)?,
            archive_sha256: digest(&archive)?,
            pending_history: Some(history.clone()),
        });
        self.body.archive = archive;
        self.body.checkpoint = None;
        self.body.checkpoint_turn = None;
        self.body.compaction = None;
        Ok(history)
    }
}
impl Store {
    #[cfg(feature = "compaction")]
    pub fn attach_compactor(&self, compactor: compaction::Compactor) -> Result<()> {
        let mut installed = self.compactor.lock().map_err(io_error)?;
        if installed.is_some() {
            return Err(error(Code::Conflict, "会话压缩器已装配。"));
        }
        *installed = Some(compactor);
        Ok(())
    }
    pub(super) fn compaction_enabled(&self) -> bool {
        #[cfg(feature = "compaction")]
        {
            self.compactor.lock().map(|c| c.is_some()).unwrap_or(false)
        }
        #[cfg(not(feature = "compaction"))]
        {
            false
        }
    }
    pub(super) fn compaction_state(&self, run_id: &str) -> Result<Option<serde_json::Value>> {
        #[cfg(feature = "compaction")]
        {
            self.compactor
                .lock()
                .map_err(io_error)?
                .as_ref()
                .and_then(|c| c.state(run_id))
                .map(|state| serde_json::to_value(state).map_err(io_error))
                .transpose()
        }
        #[cfg(not(feature = "compaction"))]
        {
            let _ = run_id;
            Ok(None)
        }
    }
    /// The caller's run must belong to this conversation. Only structured, acknowledged
    /// artifact references grant access; a URI in user text/summary is not authority.
    pub fn artifact_owner(&self, session: &str, run: &str, uri: &str) -> Result<Option<String>> {
        let doc = self.get(session)?;
        if !doc
            .body
            .turns
            .iter()
            .any(|t| t.run_id.as_deref() == Some(run))
        {
            return Err(error(Code::NotFound, "归档不属于当前会话。"));
        }
        let history = doc.history()?;
        // The first acknowledged reference owns the file. Later reads may carry the
        // same reference, but must not redirect it into the reading Run's directory.
        for turn in &doc.body.turns {
            let messages = history.get(turn.start..turn.end).ok_or_else(corrupt)?;
            if messages.iter().any(|message| matches!(message, Message::Tool { result } if result.artifact.as_ref().is_some_and(|a| a.uri == uri))) {
                return Ok(turn.run_id.clone());
            }
        }
        Ok(None)
    }
    /// Host-only history lookup for source discovery, never an HTTP raw-transcript response.
    pub fn source_history(&self, session: &str) -> Result<Vec<Message>> {
        self.get(session)?.history()
    }
}
