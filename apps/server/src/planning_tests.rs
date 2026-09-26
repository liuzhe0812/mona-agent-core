use super::*;
use planner::{PlanStep, PlanUpdate, StepStatus};

#[test]
fn disabled_unbound_run_cannot_resurrect_an_older_mode_from_tool_history() {
    use api::*;
    let old = PlanSnapshot::new(PlanMode::PlanOnly);
    let mut result = ToolResult::new("old-call", ToolStatus::Success, "past plan");
    result.structured = Some(serde_json::json!({"planner":{"run_id":"old-run","call_id":"old-call","state":old}}));
    let cp = RunCheckpoint {
        schema_version: CHECKPOINT_VERSION, run_id:"new-run".into(), revision:1, phase:CheckpointPhase::BeforeModel, step:1,
        transcript:vec![Message::user("old turn"),Message::Assistant{content:String::new(),tool_calls:vec![ToolCall::new("old-call",planner::PLAN_READ,serde_json::json!({}))],reasoning_content:None,provider_data:None},Message::Tool{result},Message::user("continue")],
        pending_tools:BTreeMap::new(),selected_tools:vec![],model_options:ModelOptions::default(),metadata:BTreeMap::new(),task_usage:TaskUsage{model_calls:0,reported_tokens:0,usage_complete:true},statistics:RunStatistics::default(),status:None,error:None,
    };
    assert_eq!(planner::recover_checkpoint(&cp).unwrap().unwrap().mode,PlanMode::PlanOnly);
    assert!(checkpoint_state(&cp).unwrap().is_empty());
}


#[test]
fn plan_mode_persists_before_any_model_call_and_only_exact_idle_revision_can_resume() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("sessions"), root.path()).unwrap();
    let header = store.create("product-plan").unwrap();
    change(&store,&header.id,PlanAction{revision:header.revision,plan_revision:0,action:Action::Plan},true).unwrap();
    let doc = store.get(&header.id).unwrap();
    let plan = state(&doc).unwrap(); assert_eq!(plan.mode, PlanMode::PlanOnly);
    assert!(doc.body.turns.is_empty()); assert!(doc.body.checkpoint.is_none());
    assert!(context(false)(&doc).is_err());
    let metadata = context(true)(&doc).unwrap(); assert!(metadata.contains_key(planner::PLAN_SEED_KEY));
    assert!(change(&store,&header.id,PlanAction{revision:header.revision,plan_revision:plan.revision,action:Action::Normal},true).is_err());
    let plan = plan.update(PlanUpdate{revision:plan.revision,goal:"deliver".into(),steps:vec![PlanStep{id:"s1".into(),text:"work".into(),status:StepStatus::Pending}],explanation:None,new_plan:false}).unwrap();
    let plan = plan.submit(plan.revision,"# Confirmed proposal\nOnly the selected work.".into()).unwrap();
    store.update_state(&header.id,doc.header.revision,STATE_KEY,|_|Ok(serde_json::to_value(&plan).unwrap())).unwrap();
    let doc=store.get(&header.id).unwrap();
    assert!(change(&store,&header.id,PlanAction{revision:doc.header.revision,plan_revision:plan.revision,action:Action::Normal},true).is_err());
    assert!(change(&store,&header.id,PlanAction{revision:doc.header.revision,plan_revision:plan.revision,action:Action::Resume},false).is_err());
    change(&store,&header.id,PlanAction{revision:doc.header.revision,plan_revision:plan.revision,action:Action::Resume},true).unwrap();
    let doc=store.get(&header.id).unwrap();let resumed=state(&doc).unwrap();
    assert_eq!(resumed.mode,PlanMode::Normal);assert_eq!(resumed.proposal,plan.proposal);
    assert!(doc.body.turns.is_empty()); // A click changes state, not an implicit Run.
    let before=doc.header.revision;
    let (_, metadata)=store.prepare_with_context(&header.id,before,"run","go",10000,|doc,_|context(true)(doc)).unwrap();
    assert_eq!(serde_json::from_str::<PlanSnapshot>(&metadata[planner::PLAN_SEED_KEY]).unwrap(),resumed);
    assert!(change(&store,&header.id,PlanAction{revision:store.header(&header.id).unwrap().revision,plan_revision:resumed.revision,action:Action::Plan},true).is_err());
}
