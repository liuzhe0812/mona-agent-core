use api::{AgentExecutor, Result, RunRequest, RunStatus};
use demo::{call, text, Add, ScriptedModel};
use planner::{bind_state, recover_history, PlanMode, PlanSnapshot, Planner};
use runtime::HostBuilder;
use serde_json::json;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    // Explicitly scripted demo: no provider request, UI or approval service.
    let model = Arc::new(ScriptedModel::new(vec![
        call(
            "plan",
            "plan_update",
            json!({"revision":0,"goal":"计算 2 + 3", "steps":[{"id":"sum","text":"调用加法工具并核对结果","status":"pending"}]}),
        ),
        call(
            "submit",
            "plan_submit",
            json!({"revision":1,"plan":"# 计算方案\n由宿主发起执行后，计算 2 + 3 并核对结果。"}),
        ),
        text("方案已提交，尚未执行。"),
        call("calculate", "add", json!({"a":2,"b":3})),
        call(
            "completed",
            "plan_update",
            json!({"revision":3,"goal":"计算 2 + 3", "steps":[{"id":"sum","text":"调用加法工具并核对结果","status":"completed"}]}),
        ),
        text("2 + 3 = 5。"),
    ]));
    let planner = Planner::default();
    let mut host = HostBuilder::new()
        .model(model)
        .tool(Arc::new(Add))
        .plugin(Arc::new(planner.plugin()))
        .build()
        .await?;
    let mut request = RunRequest::new("先提出方案，不执行");
    bind_state(&mut request, &PlanSnapshot::new(PlanMode::PlanOnly))?;
    let planned = host.engine().execute(request).await?;
    assert_eq!(planned.status, RunStatus::Completed);
    let state = recover_history(&planned.transcript)?.expect("submitted plan");
    println!("{}", state.proposal.as_deref().unwrap());

    // A trusted host action, not a model tool or an implicit approval. The host decides when
    // to start this next Run and retains the original context. Normal mode needs no such phase.
    let approved = state.resume_execution(state.revision)?;
    let mut request = RunRequest::new("执行这份方案");
    request.messages.splice(0..0, planned.transcript.clone());
    bind_state(&mut request, &approved)?;
    let report = host.engine().execute(request).await?;
    assert_eq!(report.status, RunStatus::Completed);
    assert!(recover_history(&report.transcript)?.unwrap().is_complete());
    println!("{}", report.output.as_deref().unwrap_or(""));
    host.shutdown().await
}
