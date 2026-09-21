use api::{Result, RunRequest, RunStatus};
use runtime::HostBuilder;
use demo::{call, display_run, text, Add, ScriptedModel};
use serde_json::json;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    let model = Arc::new(ScriptedModel::new(vec![call("add-1", "add", json!({"a":2,"b":3})), text("2 + 3 = 5")]));
    let mut host = HostBuilder::new().model(model).tool(Arc::new(Add)).build().await?;
    let report = display_run(&host.engine(), RunRequest::new("计算 2 + 3")).await?;
    assert_eq!(report.status, RunStatus::Completed);
    host.shutdown().await
}
