use api::*;
use std::sync::Arc;

#[test]
fn utf8_clipping_never_breaks_a_codepoint() {
    let text = "A中🙂B";
    for limit in 0..=text.len() + 2 {
        let clipped = clip_utf8(text, limit);
        assert!(clipped.len() <= limit);
        assert!(text.starts_with(clipped));
    }
}
#[test]
fn service_lookup_checks_type_and_duplicate_names() {
    let mut services = Services::default();
    services.insert(ServiceRegistration::new("n", Arc::new(7u32))).unwrap();
    assert_eq!(*services.get::<u32>("n").unwrap(), 7);
    assert!(services.get::<String>("n").is_err());
    assert!(services.insert(ServiceRegistration::new("n", Arc::new(8u32))).is_err());
    assert!(!services.without("n").contains("n"));
    assert!(services.contains("n"));
}
#[test]
fn message_json_roundtrip_preserves_tool_identity_and_unknown_status() {
    let message = Message::Tool { result: ToolResult::new("id", ToolStatus::Unknown, "started, not settled") };
    let encoded = serde_json::to_string(&message).unwrap();
    assert_eq!(serde_json::from_str::<Message>(&encoded).unwrap(), message);
}
#[test]
fn zero_limits_are_rejected() {
    let limits = RunLimits { max_parallel_tools: 0, ..Default::default() };
    assert!(limits.validate().is_err());
}
#[test]
fn cancellation_child_does_not_cancel_other_task_children() {
    let task = TaskControl::default();
    let first = task.cancellation().child_token();
    let second = task.cancellation().child_token();
    first.cancel();
    assert!(!second.is_cancelled());
    assert!(!task.cancellation().is_cancelled());
    task.cancel();
    assert!(second.is_cancelled());
}
