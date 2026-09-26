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
    pub(super) fn origin_len(&self) -> Result<usize> {
        match self.body.state.get("sessions.origin") {
            None => Ok(0),
            Some(value) => {
                if value.as_object().is_none_or(|o| o.len() != 2)
                    || value.get("version").and_then(|v| v.as_u64()) != Some(1) { return Err(corrupt()); }
                value.get("messages").and_then(|v| v.as_u64())
                    .and_then(|n| usize::try_from(n).ok()).ok_or_else(corrupt)
            }
        }
    }
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
                if let Some(turn) = self.body.turns.last() { messages.push(Message::user(&turn.prompt)); }
                else if messages.len() != self.origin_len()? { return Err(corrupt()); }
            }
        } else if !self.body.turns.is_empty()
            || self.body.checkpoint.is_some()
            || self.body.compaction.is_some()
        {
            // Only an empty new session has no working-prefix identity.
            return Err(corrupt());
        }
        if !messages.is_empty() {
            api::validate_messages(&messages).map_err(|_| corrupt())?;
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
                api::validate_messages(&archive).map_err(|_| corrupt())?;
            }
            Ok(archive)
        } else {
            if !self.body.archive.is_empty() {
                return Err(corrupt());
            }
            Ok(working)
        }
    }
    pub(super) fn next_workset(&self, use_compaction: bool) -> Result<Vec<Message>> {
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
        admission_bytes: usize,
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
            .saturating_add(usize::from(!history.is_empty()));
        // Reject before replacing a usable saved workset. The host supplies the
        // admission bound of the actual execution configuration.
        if bytes > admission_bytes {
            return Err(error(
                Code::Capacity,
                "模型工作上下文超过宿主接纳上限；旧记录与摘要状态未改动，请调整装配或新建会话。",
            ));
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
    /// Seed a newly created session with a trusted, settled history and extension state.
    /// This is not a completed Run and adds no fabricated turn or usage. No replacement of
    /// existing turns is allowed. The entire seed and state are acknowledged together.
    pub fn initialize_history(&self, id: &str, revision: u64, history: Vec<Message>, state: HostState) -> Result<Header> {
        if history.iter().any(|m| matches!(m, Message::System { .. }))
            || serde_json::to_vec(&history).map_err(io_error)?.len() > 4 * 1024 * 1024 {
            return Err(error(Code::InvalidRequest, "初始历史必须是有界的非 System 正式消息。"));
        }
        if !history.is_empty() { api::validate_messages(&history).map_err(|_| error(Code::InvalidRequest, "初始历史工具配对无效。"))?; }
        if state.contains_key("sessions.origin") { return Err(error(Code::InvalidRequest, "初始历史身份由存储维护。")); }
        validate_state(&state)?;
        let _guard = self.gate.lock().map_err(io_error)?;
        let mut doc = self.load(id)?;
        Self::editable(&doc, revision)?;
        if !doc.body.turns.is_empty() || !doc.body.state.is_empty() || doc.body.base.is_some() {
            return Err(error(Code::Conflict, "只能初始化全新空会话，不能替换已有记录。"));
        }
        doc.body.state = state;
        if !history.is_empty() {
            doc.body.state.insert("sessions.origin".into(), serde_json::json!({"version":1,"messages":history.len()}));
            let fingerprint = digest(&history)?;
            doc.body.archive = history.clone();
            doc.body.base = Some(RunBase { input_messages: history.len(), input_sha256: fingerprint.clone(),
                archive_sha256: fingerprint, pending_history: Some(history) });
        }
        self.save(&mut doc, None)?;
        Ok(doc.header)
    }
    /// Mutate only auxiliary communication state, including while a Run is active.
    /// The pure callback must not re-enter Store; it receives the exact previous value.
    /// Execution policies must use idle-only update_state, never this aux.* namespace.
    pub fn update_auxiliary_state(&self, id: &str, key: &str,
        update: impl FnOnce(Option<&serde_json::Value>) -> Result<serde_json::Value>) -> Result<Header> {
        if !key.starts_with("aux.") { return Err(error(Code::InvalidRequest, "辅助状态必须使用 aux. 名称空间。")); }
        let _guard = self.gate.lock().map_err(io_error)?;
        let mut doc = self.load(id)?;
        let next = update(doc.body.state.get(key))?;
        if doc.body.state.get(key) == Some(&next) { return Ok(doc.header); }
        doc.body.state.insert(key.into(), next);
        self.save(&mut doc, None)?;
        Ok(doc.header)
    }
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
