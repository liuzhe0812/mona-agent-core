//! Read-only presentation metrics. No private model data, added calls, or execution decisions.
use api::{ContextTransform, ContextUsage, Message, RunContext, RunStatistics};
use serde::Serialize;

#[derive(Clone, Serialize)]
pub struct ContextReading {
    pub run_id: String,
    pub tokens: Option<u64>,
    pub capacity: Option<u64>,
    pub provider_anchored: bool,
    pub observed_at: u64,
    pub system_tokens: Option<u64>,
    pub tool_tokens: Option<u64>,
    pub message_tokens: Option<u64>,
}
#[derive(Default)]
pub struct ContextMeter;
fn price<T: Serialize>(value: &T) -> Option<u64> {
    serde_json::to_vec(value).ok().map(|bytes| (bytes.len() as u64).saturating_add(2) / 3)
}
impl ContextMeter {
    pub fn session(
        &self,
        store: &sessions::Store,
        id: &str,
    ) -> sessions::SessionResult<Statistics> {
        statistics(&store.get(id)?)
    }
}
#[api::async_trait]
impl ContextTransform for ContextMeter {
    async fn transform(
        &self,
        ctx: &RunContext,
        messages: Vec<Message>,
    ) -> api::Result<Vec<Message>> {
        let request = ctx.model_request(messages.clone());
        let estimate = ctx.model.estimate_input_tokens(&request);
        // First requests have no provider anchor. Display-only approximation, explicitly marked,
        // using the same bytes/3 convention as the existing compaction extension, never a quota.
        let tokens = estimate.map(|e| e.tokens).or_else(|| {
            serde_json::to_vec(&request)
                .ok()
                .map(|bytes| (bytes.len() as u64).saturating_add(2) / 3)
        });
        // Composition is a single, explicitly approximate density estimate.
        let system = request.messages.iter().filter(|m| matches!(m, Message::System { .. })).collect::<Vec<_>>();
        let conversation = request.messages.iter().filter(|m| !matches!(m, Message::System { .. })).collect::<Vec<_>>();
        ctx.task.record_context(ContextUsage {
            tokens,
            capacity: ctx.model_context_window_tokens.filter(|n| *n > 0),
            provider_anchored: estimate.is_some_and(|e| e.provider_anchored),
            observed_at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default().as_millis().min(u64::MAX as u128) as u64,
            system_tokens: if system.is_empty() { Some(0) } else { price(&system) },
            tool_tokens: if request.tools.is_empty() { Some(0) } else { price(&request.tools) },
            message_tokens: if conversation.is_empty() { Some(0) } else { price(&conversation) },
        });
        Ok(messages)
    }
}

#[derive(Clone, Serialize)]
pub struct Statistics {
    pub session_id: String,
    pub revision: u64,
    pub turns: usize,
    pub steps: usize,
    pub latest_step: usize,
    pub latest_run_id: Option<String>,
    pub active: bool,
    pub reported_tokens: u64,
    pub model_calls: u64,
    pub usage_complete: bool,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub model_time_ms: u64,
    pub tool_time_ms: u64,
    pub ttft_ms: u64,
    pub ttft_samples: u64,
    pub decode_ms: u64,
    pub decode_tokens: u64,
    pub context: Option<ContextReading>,
    pub cache_hit_percent: Option<f64>,
    pub tokens_per_second: Option<f64>,
}
pub fn statistics(doc: &sessions::Document) -> sessions::SessionResult<Statistics> {
    let mut steps = 0usize;
    let mut latest_step = 0;
    let mut tokens = 0u64;
    let mut calls = 0u64;
    let mut complete = true;
    let mut totals = RunStatistics::default();
    let mut context = None;
    for (index, turn) in doc.body.turns.iter().enumerate() {
        let checkpoint = doc
            .body
            .checkpoint
            .as_ref()
            .filter(|_| doc.body.checkpoint_turn == Some(index));
        let count = checkpoint.map_or(turn.steps, |c| turn.steps.max(c.step));
        steps = steps.saturating_add(count);
        latest_step = count;
        let usage = checkpoint.map(|c| &c.task_usage).unwrap_or(&turn.usage);
        let sample = checkpoint.map(|c| &c.statistics).unwrap_or(&turn.statistics);
        tokens = tokens.saturating_add(usage.reported_tokens);
        calls = calls.saturating_add(usage.model_calls);
        complete &= usage.usage_complete;
        totals.input_tokens = totals.input_tokens.saturating_add(sample.input_tokens);
        totals.output_tokens = totals.output_tokens.saturating_add(sample.output_tokens);
        totals.cache_read_tokens = totals.cache_read_tokens.and_then(|n| sample.cache_read_tokens.map(|v| n.saturating_add(v)));
        totals.cache_write_tokens = totals.cache_write_tokens.and_then(|n| sample.cache_write_tokens.map(|v| n.saturating_add(v)));
        totals.model_time_ms = totals.model_time_ms.saturating_add(sample.model_time_ms);
        totals.tool_time_ms = totals.tool_time_ms.saturating_add(sample.tool_time_ms);
        totals.ttft_ms = totals.ttft_ms.saturating_add(sample.ttft_ms);
        totals.ttft_samples = totals.ttft_samples.saturating_add(sample.ttft_samples);
        totals.decode_ms = totals.decode_ms.saturating_add(sample.decode_ms);
        totals.decode_tokens = totals.decode_tokens.saturating_add(sample.decode_tokens);
        if index + 1 == doc.body.turns.len() {
            context = turn.run_id.as_ref().and_then(|run_id| sample.context.as_ref().map(|value| ContextReading {
                run_id: run_id.clone(), tokens: value.tokens, capacity: value.capacity,
                provider_anchored: value.provider_anchored, observed_at: value.observed_at,
                system_tokens: value.system_tokens, tool_tokens: value.tool_tokens,
                message_tokens: value.message_tokens,
            }));
        }
    }
    let latest = doc.body.turns.last().and_then(|t| t.run_id.clone());
    let cache_hit_percent = totals.cache_read_tokens.zip(totals.cache_write_tokens)
        .filter(|(read, write)| read.saturating_add(*write) <= totals.input_tokens && totals.input_tokens > 0)
        .map(|(read, _)| read as f64 * 100.0 / totals.input_tokens as f64);
    let tokens_per_second = (totals.decode_ms > 0 && totals.decode_tokens > 0)
        .then_some(totals.decode_tokens as f64 * 1000.0 / totals.decode_ms.max(1) as f64);
    Ok(Statistics {
        session_id: doc.header.id.clone(),
        revision: doc.header.revision,
        turns: doc.body.turns.len(),
        steps,
        latest_step,
        context,
        latest_run_id: latest,
        active: doc.header.status == sessions::Status::Running,
        reported_tokens: tokens,
        model_calls: calls,
        usage_complete: complete,
        input_tokens: totals.input_tokens,
        output_tokens: totals.output_tokens,
        cache_read_tokens: totals.cache_read_tokens,
        cache_write_tokens: totals.cache_write_tokens,
        model_time_ms: totals.model_time_ms,
        tool_time_ms: totals.tool_time_ms,
        ttft_ms: totals.ttft_ms,
        ttft_samples: totals.ttft_samples,
        decode_ms: totals.decode_ms,
        decode_tokens: totals.decode_tokens,
        cache_hit_percent,
        tokens_per_second,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_session_has_no_invented_usage_or_context() {
        let temp = tempfile::tempdir().unwrap();
        let store = sessions::Store::open(&temp.path().join("state"), temp.path()).unwrap();
        let session = store.create("metrics").unwrap();
        let value = statistics(&store.get(&session.id).unwrap()).unwrap();
        assert_eq!(value.turns, 0);
        assert_eq!(value.steps, 0);
        assert_eq!(value.reported_tokens, 0);
        assert!(value.context.is_none());
        assert!(value.cache_hit_percent.is_none());
        assert!(value.tokens_per_second.is_none());
        let json = serde_json::to_string(&value).unwrap();
        assert!(!json.contains("transcript"));
        assert!(!json.contains("model_options"));
        let meter = ContextMeter::default();
        assert_eq!(
            meter.session(&store, &session.id).unwrap().revision,
            session.revision
        );
        assert_eq!(meter.session(&store, &session.id).unwrap().turns, 0);
    }
}
