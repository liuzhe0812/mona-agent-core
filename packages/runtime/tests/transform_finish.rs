mod support;
use api::*;
use runtime::HostBuilder;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use support::*;

struct Cleanup {
    label: &'static str,
    calls: Arc<Mutex<Vec<String>>>,
    fail: bool,
}
#[async_trait]
impl ContextTransform for Cleanup {
    async fn transform(&self, _: &RunContext, messages: Vec<Message>) -> Result<Vec<Message>> {
        Ok(messages)
    }
    async fn finish(&self, id: &str) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("{}:{id}", self.label));
        if self.fail {
            Err(AgentError::new(ErrorCode::Plugin, "cleanup failed"))
        } else {
            Ok(())
        }
    }
}
#[async_trait]
impl ResultTransform for Cleanup {
    async fn transform(
        &self,
        _: &RunContext,
        _: &ToolCall,
        result: ToolResult,
    ) -> Result<ToolResult> {
        Ok(result)
    }
    async fn finish(&self, id: &str) -> Result<()> {
        ContextTransform::finish(self, id).await
    }
}

#[tokio::test(flavor = "current_thread")]
async fn finish_is_awaited_once_even_when_no_observer_gets_cpu_time() {
    let calls = Arc::new(Mutex::new(vec![]));
    let mut host = HostBuilder::new()
        .model(ScriptModel::new((0..10).map(|_| answer("done")).collect()))
        .context_transform(Arc::new(Cleanup {
            label: "context",
            calls: calls.clone(),
            fail: false,
        }))
        .result_transform(Arc::new(Cleanup {
            label: "result",
            calls: calls.clone(),
            fail: false,
        }))
        .build()
        .await
        .unwrap();
    for n in 0..10 {
        let report = host.engine().execute(RunRequest::new("go")).await.unwrap();
        assert_eq!(report.status, RunStatus::Completed);
        assert_eq!(calls.lock().unwrap().len(), (n + 1) * 2);
        assert_eq!(
            calls
                .lock()
                .unwrap()
                .iter()
                .filter(|entry| entry.ends_with(&report.run_id))
                .count(),
            2
        );
    }
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancellation_still_finishes_both_transform_families() {
    let calls = Arc::new(Mutex::new(vec![]));
    let tool = Arc::new(CountTool {
        wait_for_cancel: true,
        ..Default::default()
    });
    let mut host = HostBuilder::new()
        .model(ScriptModel::new(vec![call("a")]))
        .tool(tool.clone())
        .context_transform(Arc::new(Cleanup {
            label: "context",
            calls: calls.clone(),
            fail: false,
        }))
        .result_transform(Arc::new(Cleanup {
            label: "result",
            calls: calls.clone(),
            fail: false,
        }))
        .build()
        .await
        .unwrap();
    let run = host.engine().start(RunRequest::new("go")).unwrap();
    tool.started.notified().await;
    run.cancel();
    let report = run.wait().await.unwrap();
    assert_eq!(report.status, RunStatus::Cancelled);
    assert_eq!(calls.lock().unwrap().len(), 2);
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn cleanup_failure_is_reported_and_remaining_cleanup_still_runs() {
    let calls = Arc::new(Mutex::new(vec![]));
    let mut host = HostBuilder::new()
        .model(ScriptModel::new(vec![answer("done")]))
        .context_transform(Arc::new(Cleanup {
            label: "context",
            calls: calls.clone(),
            fail: false,
        }))
        .result_transform(Arc::new(Cleanup {
            label: "result",
            calls: calls.clone(),
            fail: true,
        }))
        .build()
        .await
        .unwrap();
    let report = host.engine().execute(RunRequest::new("go")).await.unwrap();
    assert_eq!(report.status, RunStatus::Failed);
    assert_eq!(report.error.as_ref().unwrap().code, ErrorCode::Plugin);
    assert_eq!(calls.lock().unwrap().len(), 2);
    host.shutdown().await.unwrap();
}

struct Stuck;
#[async_trait]
impl ResultTransform for Stuck {
    async fn transform(
        &self,
        _: &RunContext,
        _: &ToolCall,
        result: ToolResult,
    ) -> Result<ToolResult> {
        Ok(result)
    }
    async fn finish(&self, _: &str) -> Result<()> {
        std::future::pending().await
    }
}
#[tokio::test]
async fn cleanup_timeout_does_not_leave_the_run_without_a_report() {
    let mut host = HostBuilder::new()
        .model(ScriptModel::new(vec![answer("done")]))
        .result_transform(Arc::new(Stuck))
        .build()
        .await
        .unwrap();
    let mut request = RunRequest::new("go");
    request.limits.hook_timeout = Duration::from_millis(10);
    let report = tokio::time::timeout(Duration::from_secs(1), host.engine().execute(request))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(report.status, RunStatus::Failed);
    assert_eq!(report.error.as_ref().unwrap().code, ErrorCode::Deadline);
    host.shutdown().await.unwrap();
}
