use api::{ContextUsage, TaskControl, TaskLimits, Usage};
use std::time::Duration;

fn context(tokens: u64) -> ContextUsage {
    ContextUsage {
        tokens: Some(tokens),
        capacity: Some(1000),
        provider_anchored: false,
        observed_at: 1,
        system_tokens: None,
        tool_tokens: None,
        message_tokens: None,
    }
}
#[tokio::test]
async fn branches_share_atomic_quota_but_keep_local_observations() {
    let root = TaskControl::new(TaskLimits {
        max_model_calls: 7,
        ..Default::default()
    });
    root.record_context(context(10));
    let a = root.branch();
    let b = root.branch();
    a.record_context(context(100));
    a.anchor_context(120);
    assert_eq!(root.statistics().context.unwrap().tokens, Some(10));
    assert_eq!(a.statistics().context.unwrap().tokens, Some(120));
    assert!(b.statistics().context.is_none());
    let mut jobs = vec![];
    for i in 0..32 {
        let task = if i % 2 == 0 { a.clone() } else { b.clone() };
        jobs.push(tokio::spawn(
            async move { task.reserve_model_call().is_ok() },
        ));
    }
    let mut allowed = 0;
    for job in jobs {
        allowed += usize::from(job.await.unwrap());
    }
    assert_eq!(allowed, 7);
    assert_eq!(root.usage().model_calls, 7);
    assert_eq!(a.usage().model_calls + b.usage().model_calls, 7);
    assert!(root.check_model_call_available().is_err());
    assert!(a.check().is_ok());
    assert_eq!(a.deadline(), root.deadline());
}
#[tokio::test]
async fn usage_rolls_up_once_and_cancellation_is_directional() {
    let root = TaskControl::new(TaskLimits {
        max_reported_tokens: Some(20),
        ..Default::default()
    });
    let a = root.branch();
    let b = root.branch();
    let leaf = a.branch();
    leaf.record_usage(Some(Usage {
        input_tokens: 12,
        output_tokens: 8,
        ..Default::default()
    }));
    assert_eq!(root.usage().reported_tokens, 20);
    assert_eq!(a.usage().reported_tokens, 20);
    assert_eq!(b.usage().reported_tokens, 0);
    assert!(b.check().is_err());
    leaf.record_tool_timing(Duration::from_millis(9));
    assert_eq!(root.statistics().tool_time_ms, 9);
    assert_eq!(a.statistics().tool_time_ms, 9);
    a.cancel();
    assert!(leaf.cancellation().is_cancelled());
    assert!(!b.cancellation().is_cancelled());
    assert!(!root.cancellation().is_cancelled());
    root.cancel();
    assert!(b.cancellation().is_cancelled());
    leaf.mark_usage_incomplete();
    assert!(!root.usage().usage_complete);
}
