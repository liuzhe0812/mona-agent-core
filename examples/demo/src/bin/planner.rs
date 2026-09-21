use agent_api::{Result, RunStatus};
use agent_core::HostBuilder;
use agent_demo::{text, ScriptedModel};
use agent_planner::{PlanRequest, Planner, PlannerPlugin, PLANNER_SERVICE};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    let model = Arc::new(ScriptedModel::new(vec![
        text(r#"{"steps":["整理需求","输出验收清单"]}"#),
        text("需求：可取消、可观测、插件化。"), text("验收：正常执行、取消、工具超时、插件回滚。"),
    ]));
    let mut host = HostBuilder::new().model(model).plugin(Arc::new(PlannerPlugin::default())).build().await?;
    let planner = host.services()?.get::<Planner>(PLANNER_SERVICE)?;
    let report = planner.plan_and_execute(&host.engine(), PlanRequest::new("设计一个最小 Agent")).await?;
    assert_eq!(report.status, RunStatus::Completed);
    assert_eq!(report.step_runs.len(), 2);
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    host.shutdown().await
}
