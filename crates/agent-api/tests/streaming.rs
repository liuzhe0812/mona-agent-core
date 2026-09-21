use agent_api::*;
fn event(seq: u64, event: RunEvent) -> EventEnvelope {
    EventEnvelope { protocol_version: STREAM_VERSION, run_id: "r".into(), seq, event }
}
#[test]
fn projection_requires_contiguous_sequence_and_correct_run() {
    let mut snapshot = RunSnapshot::new("r");
    assert!(!snapshot.apply(&event(2, RunEvent::RunStarted)));
    assert!(snapshot.apply(&event(1, RunEvent::RunStarted)));
    assert!(!snapshot.apply(&event(1, RunEvent::RunStarted)));
    let mut wrong = event(2, RunEvent::RunStarted); wrong.run_id = "other".into();
    assert!(!snapshot.apply(&wrong)); assert_eq!(snapshot.seq, 1);
}
#[test]
fn completed_message_is_authoritative_not_concatenated_twice() {
    let mut snapshot = RunSnapshot::new("r");
    let mut item = WorkItem::message(1);
    snapshot.apply(&event(1, RunEvent::ItemStarted { item: item.clone() }));
    snapshot.apply(&event(2, RunEvent::TextDelta { item_id: item.id.clone(), text: "draft".into() }));
    item.state = ItemState::Completed; item.content = ItemContent::AgentMessage { text: "final".into(), truncated: false };
    snapshot.apply(&event(3, RunEvent::ItemCompleted { item }));
    assert!(matches!(&snapshot.items[0].content, ItemContent::AgentMessage { text, .. } if text == "final"));
}
#[test]
fn ui_unicode_preview_is_bounded_and_flagged() {
    let mut snapshot = RunSnapshot::new("r"); let item = WorkItem::message(1);
    snapshot.apply(&event(1, RunEvent::ItemStarted { item: item.clone() }));
    snapshot.apply(&event(2, RunEvent::TextDelta { item_id: item.id, text: "中".repeat(UI_TEXT_BYTES) }));
    assert!(matches!(&snapshot.items[0].content, ItemContent::AgentMessage { text, truncated } if *truncated && text.len() <= UI_TEXT_BYTES));
}
#[test]
fn ui_item_retention_is_bounded_and_explicit() {
    let mut snapshot = RunSnapshot::new("r");
    for n in 1..=UI_RETAINED_ITEMS + 2 {
        let mut item = WorkItem::message(n); item.state = ItemState::Completed;
        assert!(snapshot.apply(&event(n as u64, RunEvent::ItemCompleted { item })));
    }
    assert_eq!(snapshot.items.len(), UI_RETAINED_ITEMS); assert_eq!(snapshot.pruned_items, 2);
}
#[test]
fn streaming_wire_roundtrip_preserves_item_identity() {
    let e = event(1, RunEvent::ItemStarted { item: WorkItem::tool(2, 3) });
    let json = serde_json::to_string(&e).unwrap();
    assert!(json.contains("item/started"));
    let restored: EventEnvelope = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.protocol_version, 2);
    assert!(matches!(restored.event, RunEvent::ItemStarted { item } if item.id == "step-2-tool-3"));
}
#[test]
fn limits_keep_all_in_flight_items_within_snapshot_capacity() {
    let mut limits = RunLimits::default(); limits.max_tools_per_step = UI_RETAINED_ITEMS;
    assert!(limits.validate().is_err());
    limits.max_tools_per_step = UI_RETAINED_ITEMS - 1;
    assert!(limits.validate().is_ok());
}
