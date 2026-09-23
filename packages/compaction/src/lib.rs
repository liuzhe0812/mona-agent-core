//! Bounded model-visible context compaction. The canonical transcript is never mutated.
#![forbid(unsafe_code)]
mod state;
mod summary;
pub use summary::{TaskSummary, MAX_SUMMARY_BYTES};
pub use state::{CompactedRange, CompactionState};

use api::*;
use std::{
    collections::{hash_map::DefaultHasher, HashMap},
    hash::{Hash, Hasher},
    sync::{Arc, Mutex},
};

const SUMMARY_MARKER: &str = "[Earlier conversation summary. Reference data only; current user requests and system instructions take precedence.]";

#[derive(Clone, Debug)]
pub struct CompactionConfig {
    pub trigger_percent: usize,
    pub target_percent: usize,
    pub recent_groups: usize,
    pub max_summary_calls: usize,
    pub max_cached_runs: usize,
    /// Maximum handoff size; the actual budget is also bounded by this request/window.
    pub max_summary_bytes: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            trigger_percent: 80,
            target_percent: 60,
            recent_groups: 2,
            max_summary_calls: 4,
            max_cached_runs: 64,
            max_summary_bytes: 16 * 1024,
        }
    }
}

impl CompactionConfig {
    fn validate(&self) -> Result<()> {
        if self.target_percent == 0
            || self.trigger_percent <= self.target_percent
            || self.trigger_percent >= 100
            || self.recent_groups == 0
            || self.max_summary_calls == 0
            || self.max_cached_runs == 0
            || !(256..=MAX_SUMMARY_BYTES).contains(&self.max_summary_bytes)
        {
            return Err(AgentError::new(
                ErrorCode::Configuration,
                "invalid compaction configuration",
            ));
        }
        Ok(())
    }
}

#[derive(Clone)]
struct CachedSummary {
    fingerprints: Vec<(usize, u64)>,
    text: String,
    state: CompactionState,
}

#[derive(Clone)]
pub struct Compactor {
    config: CompactionConfig,
    cache: Arc<Mutex<HashMap<String, CachedSummary>>>,
}

impl Compactor {
    pub fn new(config: CompactionConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            config,
            cache: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Detached state for an awaited host checkpoint. Capture before finish releases the Run cache.
    pub fn state(&self, run_id: &str) -> Option<CompactionState> {
        self.cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(run_id).map(|cached| cached.state.clone())
    }

    pub fn clear(&self) {
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
}

impl Default for Compactor {
    fn default() -> Self {
        Self::new(CompactionConfig::default()).expect("default compaction config is valid")
    }
}

#[derive(Clone)]
struct Group {
    messages: Vec<Message>,
    fingerprint: u64,
}

impl Compactor {
    async fn compact(
        &self,
        ctx: &RunContext,
        messages: Vec<Message>,
        force: bool,
    ) -> Result<Vec<Message>> {
        ctx.task.check()?;
        let groups = group_messages(&messages)?;
        let mut cached = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&ctx.run_id)
            .cloned();
        if cached.as_ref().is_some_and(|entry| {
            entry.fingerprints.iter().any(|(index, fingerprint)| {
                groups.get(*index).map(|g| g.fingerprint) != Some(*fingerprint)
            })
        }) {
            cached = None;
            self.cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&ctx.run_id);
        }
        let previous_selection = cached
            .as_ref()
            .map(|entry| {
                entry
                    .fingerprints
                    .iter()
                    .map(|(index, _)| *index)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let current = match &cached {
            Some(entry) => project(&groups, &previous_selection, &entry.text),
            None => messages,
        };
        let before_bytes = request_bytes(ctx, &current)?;
        let trigger = ctx
            .limits
            .max_context_bytes
            .saturating_mul(self.config.trigger_percent)
            / 100;
        let token_budget = token_input_budget(ctx)?;
        let token_trigger = token_budget.map(|budget| {
            budget.saturating_mul(self.config.trigger_percent as u64) / 100
        });
        let estimated_tokens = request_tokens(ctx, &current)?;
        if !force
            && before_bytes < trigger
            && token_trigger.is_none_or(|limit| estimated_tokens < limit)
        {
            return Ok(current);
        }

        // Keep the latest actual user request, rather than the entire tool loop
        // following it. Non-text/opaque groups stay verbatim in their position.
        let latest_user = groups.iter().rposition(|group| {
            group
                .messages
                .iter()
                .any(|m| matches!(m, Message::User { .. }))
        });
        let byte_target = ctx
            .limits
            .max_context_bytes
            .saturating_mul(self.config.target_percent)
            / 100;
        let token_target_bytes = token_budget.map(|budget| {
            let tokens = budget.saturating_mul(self.config.target_percent as u64) / 100;
            usize::try_from(tokens.saturating_mul(3)).unwrap_or(usize::MAX)
        });
        let target = token_target_bytes.map_or(byte_target, |value| byte_target.min(value));
        let mut selected: Vec<usize>;
        // recent_groups is a preference. If the recent tail alone is too large,
        // reduce it to one whole group; never split an assistant/tool batch.
        let mut retain = self.config.recent_groups.min(groups.len()).max(1);
        loop {
            let cutoff = groups.len().saturating_sub(retain);
            selected = (0..groups.len())
                .filter(|index| {
                    previous_selection.contains(index)
                        || (*index < cutoff
                            && Some(*index) != latest_user
                            && safe_to_summarize(&groups[*index]))
                })
                .collect();
            if force && !selected.iter().any(|index| !previous_selection.contains(index)) {
                if retain > 1 {
                    retain -= 1;
                    continue;
                }
                // With no newly eligible history, a cached summary can still
                // be shortened once. Runtime owns the single recovery attempt.
                if cached.is_none() {
                    return Ok(current);
                }
            }
            if selected.is_empty() {
                if retain > 1 {
                    retain -= 1;
                    continue;
                }
                return Err(AgentError::new(
                    ErrorCode::Limit,
                    "no settled context can be compacted without removing protected input",
                ));
            }
            let minimum = request_bytes(ctx, &project(&groups, &selected, ""))?;
            if minimum.saturating_add(256) <= target || retain == 1 {
                break;
            }
            retain -= 1;
        }

        let minimum = request_bytes(ctx, &project(&groups, &selected, ""))?;
        // Aim for 60%. If the mandatory recent group itself exceeds that target,
        // preserve it and still require the request to fall BELOW the trigger.
        let token_maximum_bytes = token_trigger.map(|tokens| {
            usize::try_from(tokens.saturating_sub(1).saturating_mul(3))
                .unwrap_or(usize::MAX)
        });
        let maximum = token_maximum_bytes
            .map_or(trigger.saturating_sub(1), |value| value.min(trigger.saturating_sub(1)));
        let effective_target = target
            .max(minimum.saturating_add(256))
            .min(maximum)
            .min(if force { before_bytes.saturating_sub(1) } else { usize::MAX });
        let mut summary_budget = effective_target.saturating_sub(minimum).min(self.config.max_summary_bytes);
        if summary_budget < 160 {
            return Err(AgentError::new(
                ErrorCode::Limit,
                "protected context and archive references leave no summary budget",
            ));
        }
        let new_groups = selected
            .iter()
            .filter(|index| !previous_selection.contains(index))
            .map(|index| groups[*index].clone())
            .collect::<Vec<_>>();
        if force && new_groups.is_empty() {
            if let Some(entry) = &cached {
                let escaped = serde_json::to_vec(&entry.text)
                    .map_err(|_| AgentError::new(ErrorCode::ModelProtocol, "could not size cached summary"))?
                    .len().saturating_sub(2);
                summary_budget = summary_budget.min(escaped / 2);
                if summary_budget == 0 { return Ok(current); }
            }
        }
        let summary = summarize(
            ctx,
            cached.as_ref().map(|entry| entry.text.as_str()),
            &new_groups,
            self.config.max_summary_calls,
            summary_budget,
        )
        .await?;
        let projected = project(&groups, &selected, &summary);
        let projected_bytes = request_bytes(ctx, &projected)?;
        if projected_bytes >= before_bytes {
            return Err(AgentError::new(
                ErrorCode::ModelProtocol,
                "compaction summary did not shrink the context",
            ));
        }
        if projected_bytes > effective_target {
            return Err(AgentError::new(
                ErrorCode::Limit,
                "compaction summary did not meet the request budget target",
            ));
        }
        if token_trigger.is_some_and(|limit| request_tokens(ctx, &projected).map_or(true, |tokens| tokens >= limit)) {
            return Err(AgentError::new(
                ErrorCode::Limit,
                "compaction result remains above the model context pressure threshold",
            ));
        }
        ctx.task.check()?;
        if ctx.cancel.is_cancelled() {
            return Err(AgentError::new(
                ErrorCode::Cancelled,
                "compaction cancelled before publishing the summary",
            ));
        }
        let state = CompactionState::capture(&groups, &selected, &summary)?;
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Pair publication with finish's same lock. Cancelled work cannot resurrect a released cache.
        if ctx.cancel.is_cancelled() {
            return Err(AgentError::new(ErrorCode::Cancelled, "compaction cancelled before cache publication"));
        }
        if !cache.contains_key(&ctx.run_id) && cache.len() >= self.config.max_cached_runs {
            return Err(AgentError::new(ErrorCode::Limit, "compaction active-state capacity reached; existing summaries retained"));
        }
        cache.insert(
            ctx.run_id.clone(),
            CachedSummary {
                fingerprints: selected
                    .iter()
                    .map(|index| (*index, groups[*index].fingerprint))
                    .collect(),
                text: summary,
                state,
            },
        );
        Ok(projected)
    }
}

#[async_trait]
impl ContextTransform for Compactor {
    async fn finish(&self, run_id: &str) -> Result<()> {
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(run_id);
        Ok(())
    }

    async fn transform(&self, ctx: &RunContext, messages: Vec<Message>) -> Result<Vec<Message>> {
        self.compact(ctx, messages, false).await
    }

    async fn recover_context(
        &self,
        ctx: &RunContext,
        messages: Vec<Message>,
    ) -> Result<Option<Vec<Message>>> {
        let before = serialized_bytes(&messages)?;
        let recovered = self.compact(ctx, messages, true).await?;
        Ok((serialized_bytes(&recovered)? < before).then_some(recovered))
    }
}

fn request_bytes(ctx: &RunContext, messages: &[Message]) -> Result<usize> {
    let bytes = serde_json::to_vec(&ctx.model_request(messages.to_vec()))
        .map_err(|_| AgentError::new(ErrorCode::ModelProtocol, "could not size complete context request"))?.len();
    Ok(bytes.max(ctx.request_overhead_bytes.saturating_add(serialized_bytes(messages)?)))
}

fn request_tokens(ctx: &RunContext, messages: &[Message]) -> Result<u64> {
    let request = ctx.model_request(messages.to_vec());
    Ok(ctx.model.estimate_input_tokens(&request).map(|estimate| estimate.tokens)
        .unwrap_or(estimate_request_tokens(request_bytes(ctx, messages)?)))
}

fn estimate_request_tokens(bytes: usize) -> u64 {
    u64::try_from(bytes.saturating_add(2) / 3).unwrap_or(u64::MAX)
}

fn token_input_budget(ctx: &RunContext) -> Result<Option<u64>> {
    let Some(window) = ctx.model_context_window_tokens else { return Ok(None) };
    window
        .checked_sub(ctx.limits.max_output_tokens as u64)
        .filter(|value| *value > 0)
        .map(Some)
        .ok_or_else(|| AgentError::new(
            ErrorCode::Limit,
            "model context window cannot fit the configured output budget",
        ))
}

fn project(groups: &[Group], selected: &[usize], summary: &str) -> Vec<Message> {
    let mut projected = Vec::new();
    let references = selected
        .iter()
        .flat_map(|index| artifact_references(&groups[*index]))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let references = if references.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nArchived tool results (exact references for the available archive reader):\n{}",
            references.join("\n")
        )
    };
    for (index, group) in groups.iter().enumerate() {
        if selected.first() == Some(&index) {
            projected.push(Message::user(format!(
                "{SUMMARY_MARKER}\n{summary}{references}"
            )));
        }
        if !selected.contains(&index) {
            projected.extend(group.messages.clone());
        }
    }
    projected
}

fn serialized_bytes(messages: &[Message]) -> Result<usize> {
    serde_json::to_vec(messages)
        .map(|v| v.len())
        .map_err(|_| AgentError::new(ErrorCode::ModelProtocol, "could not size context messages"))
}

fn group_messages(messages: &[Message]) -> Result<Vec<Group>> {
    let mut groups = vec![];
    let mut index = 0;
    while index < messages.len() {
        let start = index;
        match &messages[index] {
            Message::Assistant { tool_calls, .. } if !tool_calls.is_empty() => {
                index += 1;
                let mut pending = tool_calls
                    .iter()
                    .map(|call| call.id.as_str())
                    .collect::<std::collections::BTreeSet<_>>();
                if pending.len() != tool_calls.len() {
                    return Err(AgentError::new(
                        ErrorCode::ModelProtocol,
                        "compaction received duplicate tool-call ids",
                    ));
                }
                for _ in tool_calls {
                    match messages.get(index) {
                        Some(Message::Tool { result })
                            if pending.remove(result.call_id.as_str()) =>
                        {
                            index += 1
                        }
                        _ => {
                            return Err(AgentError::new(
                                ErrorCode::ModelProtocol,
                                "compaction received an unsettled tool group",
                            ))
                        }
                    }
                }
            }
            Message::Tool { .. } => {
                return Err(AgentError::new(
                    ErrorCode::ModelProtocol,
                    "compaction received an orphaned tool result",
                ))
            }
            _ => index += 1,
        }
        let owned = messages[start..index].to_vec();
        let mut hasher = DefaultHasher::new();
        serde_json::to_vec(&owned)
            .map_err(|_| {
                AgentError::new(ErrorCode::ModelProtocol, "could not fingerprint context")
            })?
            .hash(&mut hasher);
        groups.push(Group {
            messages: owned,
            fingerprint: hasher.finish(),
        });
    }
    Ok(groups)
}

fn safe_to_summarize(group: &Group) -> bool {
    group.messages.iter().all(|message| match message {
        Message::User { content } => !content.has_media(),
        Message::Tool { result } => {
            !result.content.has_media()
                && result
                    .artifact
                    .as_ref()
                    .map_or(true, |artifact| artifact.uri.starts_with("spill:"))
        }
        Message::System { .. } => false,
        Message::Assistant { reasoning_content, provider_data, tool_calls, .. } => {
            reasoning_content.is_none() && provider_data.is_none()
                && tool_calls.iter().all(|call| call.provider_data.is_none())
        },
    })
}

fn artifact_references(group: &Group) -> Vec<String> {
    group
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::Tool { result } => result
                .artifact
                .as_ref()
                .filter(|artifact| artifact.uri.starts_with("spill:"))
                .map(|artifact| format!("{} ({} bytes)", artifact.uri, artifact.bytes)),
            _ => None,
        })
        .collect()
}

fn render_group(group: &Group) -> String {
    group
        .messages
        .iter()
        .map(|message| match message {
            Message::System { content } => format!("SYSTEM: {content}"),
            Message::User { content } => format!("USER: {}", content.text()),
            Message::Assistant {
                content,
                tool_calls,
                ..
            } => {
                if tool_calls.is_empty() {
                    format!("ASSISTANT: {content}")
                } else {
                    let calls = tool_calls
                        .iter()
                        .map(|call| format!("{} {} {}", call.id, call.name, call.arguments))
                        .collect::<Vec<_>>()
                        .join("; ");
                    format!("ASSISTANT: {content}\nTOOL CALLS: {calls}")
                }
            }
            Message::Tool { result } => format!(
                "TOOL RESULT [{} {:?}]: {}\nSTRUCTURED RESULT: {}",
                result.call_id,
                result.status,
                result.content.text(),
                result
                    .structured
                    .as_ref()
                    .map(|value| value.to_string())
                    .unwrap_or_default()
            ),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn summarize(
    ctx: &RunContext,
    seed: Option<&str>,
    groups: &[Group],
    max_calls: usize,
    summary_budget: usize,
) -> Result<String> {
    let rendered = groups.iter().map(render_group).collect::<Vec<_>>();
    let mut summary = seed.unwrap_or_default().to_owned();
    let mut next = 0;
    let mut calls = 0;
    loop {
        if calls >= max_calls {
            return Err(AgentError::new(
                ErrorCode::Limit,
                "compaction exceeded its bounded summary-call budget",
            ));
        }
        // Pack WHOLE settled groups using the actual serialized auxiliary
        // request (including escaped text and inherited model options).
        let mut source = String::new();
        let start = next;
        while let Some(group) = rendered.get(next) {
            let candidate = format!("{source}\n\n{group}");
            let request = summary_request(ctx, &summary, &candidate, summary_budget);
            if !summary_request_fits(ctx, &request)? {
                break;
            }
            source = candidate;
            next += 1;
        }
        if next == start && next < rendered.len() {
            return Err(AgentError::new(
                ErrorCode::Limit,
                "one settled context group cannot fit the summary request budget",
            ));
        }
        let request = summary_request(ctx, &summary, &source, summary_budget);
        if !summary_request_fits(ctx, &request)? {
            return Err(AgentError::new(
                ErrorCode::Limit,
                "summary request exceeds the byte or model window budget",
            ));
        }
        let reply = ctx.model.complete(request, None).await?;
        calls += 1;
        if reply.finish != FinishReason::Stop
            || !reply.tool_calls.is_empty()
            || reply.content.trim().is_empty()
        {
            return Err(AgentError::new(
                ErrorCode::ModelProtocol,
                "compaction summary response was incomplete or invalid",
            ));
        }
        let handoff = TaskSummary::parse(&reply.content)?;
        let text = serde_json::to_string(&handoff).map_err(|_| AgentError::new(ErrorCode::ModelProtocol, "cannot serialize task handoff"))?;
        // Validate before replacing any cached state; never repair or clip a malformed handoff.
        let escaped_bytes =
            serde_json::to_vec(&text).map_or(usize::MAX, |v| v.len().saturating_sub(2));
        if escaped_bytes > summary_budget {
            return Err(AgentError::new(
                ErrorCode::Limit,
                "compaction summary exceeds its reserved byte budget",
            ));
        }
        summary = text;
        if next == rendered.len() {
            return Ok(summary);
        }
    }
}

fn summary_request_fits(ctx: &RunContext, request: &ModelRequest) -> Result<bool> {
    let bytes = serde_json::to_vec(request)
        .map_err(|_| AgentError::new(ErrorCode::ModelProtocol, "could not size summary request"))?
        .len();
    Ok(bytes <= ctx.limits.max_context_bytes
        && ctx.model_context_window_tokens.is_none_or(|window| {
            estimate_request_tokens(bytes).saturating_add(request.max_output_tokens as u64) <= window
        }))
}

fn summary_request(
    ctx: &RunContext,
    seed: &str,
    source: &str,
    summary_budget: usize,
) -> ModelRequest {
    ModelRequest {
        messages: vec![
            Message::system(format!("{}\nThe serialized handoff, including JSON escaping when placed in a request string, must fit {summary_budget} bytes.", summary::INSTRUCTIONS)),
            Message::user(serde_json::json!({"previous_handoff":seed,"settled_history":source}).to_string()),
        ],
        tools: vec![],
        max_output_tokens: ctx.limits.max_output_tokens.min(summary_budget.div_ceil(2).min(u32::MAX as usize) as u32),
        options: ctx.model_options.clone(),
    }
}

pub struct CompactionPlugin {
    compactor: Compactor,
}
impl CompactionPlugin {
    pub fn new(config: CompactionConfig) -> Result<Self> {
        Ok(Self {
            compactor: Compactor::new(config)?,
        })
    }
    pub fn compactor(&self) -> Compactor {
        self.compactor.clone()
    }
}
impl Default for CompactionPlugin {
    fn default() -> Self {
        Self {
            compactor: Compactor::default(),
        }
    }
}

#[async_trait]
impl Plugin for CompactionPlugin {
    fn manifest(&self) -> PluginManifest {
        let mut manifest = PluginManifest::new("compaction");
        manifest.requires.push("agent.api_version".into());
        manifest
    }
    async fn install(&self, registrar: &mut dyn Registrar) -> Result<()> {
        registrar.context_transform(Arc::new(self.compactor.clone()));
        Ok(())
    }
    async fn shutdown(&self) -> Result<()> {
        self.compactor.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct SummaryModel {
        calls: AtomicUsize,
    }
    #[async_trait]
    impl ModelCaller for SummaryModel {
        async fn complete(
            &self,
            _: ModelRequest,
            _: Option<Arc<dyn ModelSink>>,
        ) -> Result<ModelReply> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ModelReply {
                content: serde_json::to_string(&TaskSummary { goal: "kept decision".into(), ..Default::default() }).unwrap(),
                tool_calls: vec![],
                reasoning_content: None,
                provider_data: None,
                finish: FinishReason::Stop,
                usage: None,
            })
        }
    }

    fn context(model: Arc<SummaryModel>, max: usize) -> RunContext {
        let limits = RunLimits { max_context_bytes: max, ..Default::default() };
        RunContext {
            run_id: "run-1".into(),
            task: TaskControl::default(),
            cancel: CancellationToken::new(),
            model,
            services: Services::default(),
            metadata: Arc::new(Default::default()),
            model_options: ModelOptions::default(),
            limits,
            request_overhead_bytes: 100,
            request_tools: Arc::new(Vec::new()),
            context_sources: Arc::new(Vec::new()),
            model_context_window_tokens: None,
            tools_enabled: true,
            allowed_tools: None,
        }
    }

    #[tokio::test]
    async fn short_context_has_zero_summary_calls() {
        let model = Arc::new(SummaryModel {
            calls: AtomicUsize::new(0),
        });
        let output = Compactor::default()
            .transform(&context(model.clone(), 4096), vec![Message::user("short")])
            .await
            .unwrap();
        assert_eq!(output.len(), 1);
        assert_eq!(model.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn old_settled_prefix_is_replaced_without_mutating_recent_turns() {
        let model = Arc::new(SummaryModel {
            calls: AtomicUsize::new(0),
        });
        let assistant = |content: &str| Message::Assistant {
            content: content.into(),
            tool_calls: vec![],
            reasoning_content: None,
            provider_data: None,
        };
        let messages = vec![
            Message::system("rules"),
            Message::user("x".repeat(900)),
            Message::user("x".repeat(900)),
            assistant("old answer"),
            Message::user("new request"),
            assistant("recent answer"),
        ];
        let compactor = Compactor::default();
        let ctx = context(model.clone(), 2400);
        let output = compactor.transform(&ctx, messages.clone()).await.unwrap();
        assert!(matches!(&output[0], Message::System { content } if content == "rules"));
        assert!(output.iter().any(|m| m.text().contains("kept decision")));
        assert!(output.iter().any(|m| m.text() == "new request"));
        assert_eq!(messages[1].text().len(), 900);
        let calls = model.calls.load(Ordering::SeqCst);
        assert!(calls > 0);
        let _ = compactor.transform(&ctx, messages).await.unwrap();
        assert_eq!(model.calls.load(Ordering::SeqCst), calls);
        assert!(compactor.cache.lock().unwrap().contains_key(&ctx.run_id));
        compactor.finish(&ctx.run_id).await.unwrap();
        assert!(compactor.cache.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn known_model_window_triggers_before_the_byte_ceiling() {
        let model = Arc::new(SummaryModel { calls: AtomicUsize::new(0) });
        let assistant = |content: &str| Message::Assistant {
            content: content.into(), tool_calls: vec![], reasoning_content: None, provider_data: None,
        };
        let mut messages: Vec<_> = (0..6).flat_map(|_| [
            Message::user("old request ".repeat(30)), assistant("old answer"),
        ]).collect();
        messages.extend([Message::user("current request"), assistant("recent answer")]);
        let mut ctx = context(model.clone(), 64 * 1024);
        ctx.model_context_window_tokens = Some(900);
        ctx.limits.max_output_tokens = 100;
        let output = Compactor::default().transform(&ctx, messages).await.unwrap();
        assert!(output.iter().any(|message| message.text().contains("kept decision")));
        assert!(model.calls.load(Ordering::SeqCst) >= 2);
    }

    #[tokio::test]
    async fn known_model_window_rejects_an_unshrinkable_protected_tail() {
        let model = Arc::new(SummaryModel { calls: AtomicUsize::new(0) });
        let assistant = |content: &str| Message::Assistant {
            content: content.into(), tool_calls: vec![], reasoning_content: None, provider_data: None,
        };
        let messages = vec![
            Message::user("old"),
            assistant("old answer"),
            Message::user("current request ".repeat(150)),
            assistant("recent answer"),
        ];
        let mut ctx = context(model, 64 * 1024);
        ctx.model_context_window_tokens = Some(600);
        ctx.limits.max_output_tokens = 100;
        let error = Compactor::default().transform(&ctx, messages).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::Limit);
    }

    #[tokio::test]
    async fn spill_reference_survives_compaction_as_an_exact_identifier() {
        let model = Arc::new(SummaryModel {
            calls: AtomicUsize::new(0),
        });
        let call = ToolCall::new("call-1", "large_tool", serde_json::json!({}));
        let mut result = ToolResult::new("call-1", ToolStatus::Success, "preview");
        result.artifact = Some(ArtifactRef {
            uri: "spill:sp_exact123".into(),
            bytes: 90_000,
        });
        let messages = vec![
            Message::user("x".repeat(1050)),
            Message::user("x".repeat(1050)),
            Message::Assistant {
                content: String::new(),
                tool_calls: vec![call],
                reasoning_content: None,
                provider_data: None,
            },
            Message::Tool { result },
            Message::user("continue"),
            Message::Assistant {
                content: "recent".into(),
                tool_calls: vec![],
                reasoning_content: None,
                provider_data: None,
            },
        ];
        let output = Compactor::default()
            .transform(&context(model, 3000), messages)
            .await
            .unwrap();
        assert!(output
            .iter()
            .any(|message| message.text().contains("spill:sp_exact123")));
    }
}
