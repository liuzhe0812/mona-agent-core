#[path = "../../runtime/tests/support/mod.rs"]
mod support;
use api::*;
use planner::*;
use runtime::HostBuilder;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use support::*;

fn update(revision: u64, status: &str) -> Value {
    json!({"revision":revision,"goal":"deliver","steps":[{"id":"work","text":"do the work","status":status}]})
}
#[tokio::test]
async fn same_agent_keeps_full_context_executes_once_and_marks_progress_without_child_runs() {
    let model = ScriptModel::new(vec![
        calls(&[("p1", PLAN_UPDATE, update(0, "in_progress"))]),
        call("business"),
        calls(&[("p2", PLAN_UPDATE, update(1, "completed"))]),
        answer("finished"),
    ]);
    let business = CountTool::default();
    let probe = business.probe.clone();
    let planner = Planner::default();
    let mut host = HostBuilder::new()
        .model(model.clone())
        .tool(Arc::new(business))
        .plugin(Arc::new(planner.plugin()))
        .build()
        .await
        .unwrap();
    let report = host
        .engine()
        .execute(RunRequest::new("keep original requirement 7919"))
        .await
        .unwrap();
    assert_eq!(report.status, RunStatus::Completed);
    assert_eq!(report.task_usage.model_calls, 4);
    assert_eq!(probe.count.load(std::sync::atomic::Ordering::SeqCst), 1);
    let state = recover_history(&report.transcript).unwrap().unwrap();
    assert!(state.is_complete());
    for request in model.requests.lock().unwrap().iter() {
        assert!(request
            .messages
            .iter()
            .any(|m| m.text().contains("original requirement 7919")));
    }
    for result in results(&report).iter().filter(|r| r.structured.is_some()) {
        assert_eq!(
            result.structured.as_ref().unwrap()["planner"]["run_id"],
            report.run_id
        );
    }
    assert!(!report
        .transcript
        .iter()
        .any(|m| m.text().contains("NORMAL MODE:")));
    assert!(planner.live_snapshot(&report.run_id).unwrap().is_none());
    host.shutdown().await.unwrap();
}
#[tokio::test]
async fn trivial_tasks_and_uninstalled_planner_need_no_planning_calls() {
    for installed in [false, true] {
        let model = ScriptModel::new(vec![answer("hello")]);
        let builder = HostBuilder::new().model(model.clone());
        let mut host = if installed {
            builder.plugin(Arc::new(PlannerPlugin::default()))
        } else {
            builder
        }
        .build()
        .await
        .unwrap();
        let report = host
            .engine()
            .execute(RunRequest::new("hello"))
            .await
            .unwrap();
        assert_eq!(report.task_usage.model_calls, 1);
        assert_eq!(report.output.as_deref(), Some("hello"));
        assert!(recover_history(&report.transcript).unwrap().is_none());
        host.shutdown().await.unwrap();
    }
}
#[tokio::test]
async fn plan_only_filters_effects_and_submission_denies_later_tools_in_the_same_batch() {
    let planner = Planner::new(PlannerConfig {
        initial_mode: PlanMode::PlanOnly,
        planning_tools: ["count".into(), "write".into()].into(),
        ..Default::default()
    })
    .unwrap();
    let read = CountTool::default();
    let read_probe = read.probe.clone();
    let write = CountTool {
        name: "write",
        effects: true,
        ..Default::default()
    };
    let write_probe = write.probe.clone();
    let model = ScriptModel::new(vec![
        calls(&[("p1", PLAN_UPDATE, update(0, "pending"))]),
        calls(&[
            (
                "submit",
                PLAN_SUBMIT,
                json!({"revision":1,"plan":"# Plan\nPerform work after host confirmation."}),
            ),
            ("late-read", "count", json!({"value":2})),
        ]),
        answer("waiting"),
    ]);
    let mut host = HostBuilder::new()
        .model(model.clone())
        .tool(Arc::new(read))
        .tool(Arc::new(write))
        .allow_side_effect_tool("write")
        .plugin(Arc::new(planner.plugin()))
        .build()
        .await
        .unwrap();
    let report = host
        .engine()
        .execute(RunRequest::new("plan this task"))
        .await
        .unwrap();
    assert_eq!(report.status, RunStatus::Completed);
    assert_eq!(results(&report).last().unwrap().status, ToolStatus::Denied);
    assert_eq!(
        read_probe.count.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert_eq!(
        write_probe.count.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    {
        let requests = model.requests.lock().unwrap();
        assert!(requests[0].tools.iter().any(|t| t.name == "count"));
        assert!(requests
            .iter()
            .all(|r| r.tools.iter().all(|t| t.name != "write")));
        assert!(requests[2].tools.iter().all(|t| t.name == PLAN_READ));
    }
    let snapshot = recover_history(&report.transcript).unwrap().unwrap();
    assert!(snapshot.proposal.is_some());
    assert_eq!(snapshot.mode, PlanMode::PlanOnly);
    host.shutdown().await.unwrap();
}
#[tokio::test]
async fn model_cannot_change_mode_and_schema_corrections_still_work() {
    let mut bad = update(0, "pending");
    bad["mode"] = json!("normal");
    let model = ScriptModel::new(vec![
        calls(&[("bad", PLAN_UPDATE, bad)]),
        calls(&[("good", PLAN_UPDATE, update(0, "pending"))]),
        answer("planned"),
    ]);
    let planner = Planner::new(PlannerConfig {
        initial_mode: PlanMode::PlanOnly,
        ..Default::default()
    })
    .unwrap();
    let mut host = HostBuilder::new()
        .model(model)
        .plugin(Arc::new(planner.plugin()))
        .build()
        .await
        .unwrap();
    let report = host
        .engine()
        .execute(RunRequest::new("plan"))
        .await
        .unwrap();
    assert_eq!(results(&report)[0].status, ToolStatus::Error);
    let state = recover_history(&report.transcript).unwrap().unwrap();
    assert_eq!(state.revision, 1);
    assert_eq!(state.mode, PlanMode::PlanOnly);
    host.shutdown().await.unwrap();
}
#[tokio::test]
async fn seed_is_authoritative_and_mode_cannot_change_mid_run() {
    let planner = Planner::default();
    let model = ScriptModel::new(vec![
        calls(&[("p", PLAN_UPDATE, update(0, "pending"))]),
        calls(&[(
            "s",
            PLAN_SUBMIT,
            json!({"revision":1,"plan":"# Plan\nDo work."}),
        )]),
        answer("waiting"),
    ]);
    let mut host = HostBuilder::new()
        .model(model)
        .plugin(Arc::new(planner.plugin()))
        .build()
        .await
        .unwrap();
    let mut request = RunRequest::new("plan");
    bind_state(&mut request, &PlanSnapshot::new(PlanMode::PlanOnly)).unwrap();
    let first = host.engine().execute(request).await.unwrap();
    host.shutdown().await.unwrap();
    let state = recover_history(&first.transcript).unwrap().unwrap();
    let resumed = state.resume_execution(state.revision).unwrap();
    let model = ScriptModel::new(vec![call("execute"), answer("done")]);
    let tool = CountTool::default();
    let probe = tool.probe.clone();
    let mut host = HostBuilder::new()
        .model(model.clone())
        .tool(Arc::new(tool))
        .plugin(Arc::new(planner.plugin()))
        .build()
        .await
        .unwrap();
    let mut request = RunRequest::new("execute approved version");
    request.messages.splice(0..0, first.transcript.clone());
    bind_state(&mut request, &resumed).unwrap();
    let report = host.engine().execute(request).await.unwrap();
    assert_eq!(report.status, RunStatus::Completed);
    assert_eq!(probe.count.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(model.requests.lock().unwrap()[0]
        .messages
        .iter()
        .any(|m| m.text().contains("NORMAL MODE:")));
    host.shutdown().await.unwrap();
}
#[tokio::test]
async fn result_budget_failure_does_not_mutate_or_publish_a_partial_plan() {
    let model = ScriptModel::new(vec![
        calls(&[("large", PLAN_UPDATE, update(0, "in_progress"))]),
        answer("smaller required"),
    ]);
    let mut host = HostBuilder::new()
        .model(model.clone())
        .plugin(Arc::new(PlannerPlugin::default()))
        .build()
        .await
        .unwrap();
    let mut request = RunRequest::new("task");
    request.limits.max_tool_result_bytes = 128;
    let report = host.engine().execute(request).await.unwrap();
    assert_eq!(results(&report)[0].status, ToolStatus::Error);
    assert!(recover_history(&report.transcript).unwrap().is_none());
    assert!(model.requests.lock().unwrap()[1]
        .messages
        .iter()
        .any(|m| m.text().contains("\"revision\":0")));
    host.shutdown().await.unwrap();
}
#[tokio::test]
async fn cancellation_preserves_progress_claims_and_cleans_only_its_run() {
    let model = ScriptModel::new(vec![
        calls(&[("p", PLAN_UPDATE, update(0, "in_progress"))]),
        call("waiting"),
    ]);
    let tool = CountTool {
        wait_for_cancel: true,
        ..Default::default()
    };
    let started = tool.started.clone();
    let planner = Planner::default();
    let mut host = HostBuilder::new()
        .model(model)
        .tool(Arc::new(tool))
        .plugin(Arc::new(planner.plugin()))
        .build()
        .await
        .unwrap();
    let handle = host.engine().start(RunRequest::new("task")).unwrap();
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .unwrap();
    assert_eq!(
        planner
            .live_snapshot(&handle.run_id)
            .unwrap()
            .unwrap()
            .revision,
        1
    );
    handle.cancel();
    let report = handle.wait().await.unwrap();
    assert_eq!(report.status, RunStatus::Cancelled);
    assert_eq!(
        recover_history(&report.transcript).unwrap().unwrap().steps[0].status,
        StepStatus::InProgress
    );
    assert!(planner.live_snapshot(&handle.run_id).unwrap().is_none());
    host.shutdown().await.unwrap();
}
#[tokio::test]
async fn independent_runs_do_not_share_plan_state_or_silently_evict_an_active_run() {
    let planner = Planner::new(PlannerConfig {
        max_active_runs: 1,
        ..Default::default()
    })
    .unwrap();
    let model = ScriptModel::new(vec![
        calls(&[("p", PLAN_UPDATE, update(0, "in_progress"))]),
        call("waiting"),
        answer("fresh"),
    ]);
    let tool = CountTool {
        wait_for_cancel: true,
        ..Default::default()
    };
    let started = tool.started.clone();
    let mut host = HostBuilder::new()
        .model(model.clone())
        .tool(Arc::new(tool))
        .plugin(Arc::new(planner.plugin()))
        .build()
        .await
        .unwrap();
    let first = host.engine().start(RunRequest::new("first")).unwrap();
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .unwrap();
    let second = host
        .engine()
        .execute(RunRequest::new("second"))
        .await
        .unwrap();
    assert_eq!(second.status, RunStatus::Limited);
    assert_eq!(
        planner
            .live_snapshot(&first.run_id)
            .unwrap()
            .unwrap()
            .revision,
        1
    );
    first.cancel();
    first.wait().await.unwrap();
    let third = host
        .engine()
        .execute(RunRequest::new("third"))
        .await
        .unwrap();
    assert_eq!(third.status, RunStatus::Completed);
    assert!(recover_history(&third.transcript).unwrap().is_none());
    host.shutdown().await.unwrap();
}
