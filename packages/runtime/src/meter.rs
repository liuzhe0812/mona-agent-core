//! A single primary-request usage anchor. No request bodies are retained in metadata mode.
use api::*;
use sha2::{Digest, Sha256};

fn digest<T: serde::Serialize>(value: &T) -> Option<[u8; 32]> {
    Some(Sha256::digest(serde_json::to_vec(value).ok()?).into())
}
fn envelope(request: &ModelRequest) -> Option<[u8; 32]> {
    digest(&(&request.tools, &request.options, request.max_output_tokens))
}
#[derive(Default)]
pub(crate) struct InputMeter {
    sample: Option<Sample>,
}
pub(crate) struct Sample {
    envelope: [u8; 32],
    messages: Vec<[u8; 32]>,
    tokens: u64,
}
impl InputMeter {
    pub fn prepare(request: &ModelRequest) -> Option<Sample> {
        Some(Sample {
            envelope: envelope(request)?,
            messages: request
                .messages
                .iter()
                .map(digest)
                .collect::<Option<Vec<_>>>()?,
            tokens: 0,
        })
    }
    pub fn record(&mut self, prepared: Option<Sample>, usage: Option<Usage>) {
        self.sample =
            prepared
                .zip(usage.filter(|u| u.input_tokens > 0))
                .map(|(mut sample, usage)| {
                    sample.tokens = usage.input_tokens;
                    sample
                });
    }
    pub fn estimate(&self, request: &ModelRequest) -> Option<InputTokenEstimate> {
        let sample = self.sample.as_ref()?;
        if sample.envelope != envelope(request)? || request.messages.len() < sample.messages.len() {
            return None;
        }
        for (message, fingerprint) in request.messages.iter().zip(&sample.messages) {
            if digest(message)? != *fingerprint {
                return None;
            }
        }
        let tail = &request.messages[sample.messages.len()..];
        // Intentionally approximate and conservatively byte-based, including JSON escaping.
        let extra = if tail.is_empty() {
            0
        } else {
            (serde_json::to_vec(tail).ok()?.len() as u64).saturating_add(2) / 3
        };
        Some(InputTokenEstimate {
            tokens: sample.tokens.saturating_add(extra),
            provider_anchored: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    };
    struct UsageModel(AtomicU64);
    #[async_trait]
    impl Model for UsageModel {
        async fn stream(&self, _: ModelRequest, _: CancellationToken) -> Result<ModelStream> {
            let tokens = self.0.fetch_add(500_000, Ordering::SeqCst);
            Ok(Box::pin(futures_util::stream::iter(vec![
                Ok(ModelEvent::Text("ok".into())),
                Ok(ModelEvent::Usage(Usage {
                    input_tokens: tokens,
                    output_tokens: 3,
                })),
                Ok(ModelEvent::Finish(FinishReason::Stop)),
                Ok(ModelEvent::End),
            ])))
        }
    }
    #[tokio::test]
    async fn auxiliary_usage_is_billed_but_cannot_replace_the_primary_input_anchor() {
        let gateway = crate::model::Gateway::new(
            Arc::new(UsageModel(AtomicU64::new(31))),
            TaskControl::default(),
            CancellationToken::new(),
            RunLimits {
                audit_mode: AuditMode::Metadata,
                ..Default::default()
            },
            ModelOptions::default(),
        );
        let request = ModelRequest {
            messages: vec![Message::user("primary input")],
            tools: vec![],
            max_output_tokens: 32,
            options: ModelOptions::default(),
        };
        gateway
            .complete_primary(request.clone(), None)
            .await
            .unwrap();
        let mut auxiliary = request.clone();
        auxiliary.messages = vec![Message::user("auxiliary summary")];
        gateway.complete(auxiliary, None).await.unwrap();
        assert_eq!(gateway.estimate_input_tokens(&request).unwrap().tokens, 31);
        let audits = gateway.audits();
        assert_eq!(audits.len(), 2);
        assert!(audits.iter().all(|audit| audit.request.is_none()));
        assert!(audits[1].usage.unwrap().input_tokens > 500_000);
    }

    #[test]
    fn anchor_is_prefix_and_exact_envelope_scoped_and_body_free() {
        let original = ModelRequest {
            messages: vec![Message::user("中文 and JSON")],
            tools: vec![],
            max_output_tokens: 64,
            options: ModelOptions::default(),
        };
        let mut meter = InputMeter::default();
        meter.record(
            InputMeter::prepare(&original),
            Some(Usage {
                input_tokens: 37,
                output_tokens: 2,
            }),
        );
        assert_eq!(meter.estimate(&original).unwrap().tokens, 37);
        let mut next = original.clone();
        next.messages.push(Message::user("more"));
        assert!(meter.estimate(&next).unwrap().tokens > 37);
        next.messages[0] = Message::user("changed rules or compacted prefix");
        assert!(meter.estimate(&next).is_none());
        next = original.clone();
        next.options.model = Some("another-model".into());
        assert!(meter.estimate(&next).is_none());
        next = original.clone();
        next.max_output_tokens += 1;
        assert!(meter.estimate(&next).is_none());
        meter.record(InputMeter::prepare(&original), None);
        assert!(meter.estimate(&original).is_none());
    }
}
