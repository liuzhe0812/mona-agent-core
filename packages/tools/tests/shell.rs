use api::{
    CancellationToken, ModelCaller, ModelOptions, RunContext, RunLimits, Services, TaskControl,
    Tool, ToolContext, ToolProgress,
};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};
use tools::{ShellConfig, ShellKind, ShellTool, ToolConfig};

struct NoProgress;
impl ToolProgress for NoProgress {
    fn report(&self, _: &str) {}
}

struct NoModel;
#[api::async_trait]
impl ModelCaller for NoModel {
    async fn complete(
        &self,
        _: api::ModelRequest,
        _: Option<Arc<dyn api::ModelSink>>,
    ) -> api::Result<api::ModelReply> {
        Err(api::AgentError::new(api::ErrorCode::Unsupported, "unused"))
    }
}

fn context() -> ToolContext {
    ToolContext {
        run: RunContext {
            run_id: "shell-test".into(),
            task: TaskControl::default(),
            cancel: CancellationToken::new(),
            model: Arc::new(NoModel),
            services: Services::default(),
            metadata: Arc::new(BTreeMap::new()),
            model_options: ModelOptions::default(),
            limits: RunLimits::default(),
            request_overhead_bytes: 0,
            request_tools: Arc::new(Vec::new()),
            context_sources: Arc::new(Vec::new()),
            model_context_window_tokens: None,
            tools_enabled: true,
            allowed_tools: None,
        },
        call_id: "call".into(),
        progress: Arc::new(NoProgress),
    }
}

fn native_shell() -> ShellConfig {
    if let Some(path) = std::env::var_os("SHELL_PATH").map(PathBuf::from) {
        assert!(path.is_file(), "SHELL_PATH must name an actual executable");
        return path.into();
    }
    ShellConfig::discover().expect("a native shell must be available; tests never silently skip")
}

fn command(shell: &ShellConfig, posix: &str, powershell: &str) -> String {
    if shell.kind == ShellKind::PowerShell {
        powershell
    } else {
        posix
    }
    .to_owned()
}

#[tokio::test]
async fn executes_successful_command_and_reports_exit_code() {
    let shell = native_shell();
    let script = command(
        &shell,
        "printf 'hello中文'",
        "[Console]::Write('hello中文')",
    );
    let dir = tempfile::tempdir().unwrap();
    let tool = ShellTool::new(ToolConfig::new(dir.path(), shell.clone()));
    assert_eq!(tool.spec().name, "shell");
    assert!(tool.spec().description.contains(shell.syntax()));
    let output = tool
        .execute(context(), serde_json::json!({"command":script}))
        .await
        .unwrap();
    assert!(!output.is_error);
    assert!(
        output.content.preview().ends_with("hello中文"),
        "unexpected shell output: {:?}",
        output.content.text()
    );
    let structured = output.structured.as_ref().unwrap();
    assert_eq!(structured["exit_code"], 0);
    assert_eq!(structured["truncated"], false);
}

#[tokio::test]
async fn nonzero_command_is_a_tool_error_with_exit_code() {
    let shell = native_shell();
    let script = command(
        &shell,
        "printf 'failed'; sh -c 'exit 7'",
        "[Console]::Write('failed'); & $env:ComSpec /d /c 'exit 7'",
    );
    let dir = tempfile::tempdir().unwrap();
    let tool = ShellTool::new(ToolConfig::new(dir.path(), shell));
    let output = tool
        .execute(context(), serde_json::json!({"command":script}))
        .await
        .unwrap();
    assert!(output.is_error);
    assert!(output.content.preview().contains("failed"));
    assert_eq!(output.structured.as_ref().unwrap()["exit_code"], 7);
}

#[tokio::test]
async fn timeout_terminates_the_child_and_returns_deadline_error() {
    let shell = native_shell();
    let script = command(&shell, "sleep 5", "Start-Sleep -Seconds 5");
    let dir = tempfile::tempdir().unwrap();
    let tool = ShellTool::new(ToolConfig::new(dir.path(), shell));
    let started = std::time::Instant::now();
    let error = tool
        .execute(
            context(),
            serde_json::json!({"command":script, "timeout":0.05}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, api::ErrorCode::Deadline);
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn no_archive_returns_an_explicit_bounded_preview_without_a_dead_locator() {
    let shell = native_shell();
    let script = command(&shell,
        "printf '%0120000d' 0; printf 'END_OUTPUT'",
        "[Console]::Write(('0' * 120000) + 'END_OUTPUT')");
    let dir = tempfile::tempdir().unwrap();
    let mut config = ToolConfig::new(dir.path(), shell);
    config.max_command_bytes = 1024;
    let output = ShellTool::new(config).execute(context(), serde_json::json!({"command":script})).await.unwrap();
    assert!(!output.is_error);
    assert_eq!(output.structured.as_ref().unwrap()["truncated"], true);
    assert_eq!(output.structured.as_ref().unwrap()["exit_code"], 0);
    assert_eq!(output.structured.as_ref().unwrap()["output_bytes"], 120010);
    assert!(output.artifact.is_none());
    assert!(output.content.text().contains("not retained"));
    assert!(output.content.text().ends_with("END_OUTPUT"));
    assert!(output.content.byte_len() < 1400);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn a_small_result_budget_never_silently_truncates_complete_inline_output() {
    let dir = tempfile::tempdir().unwrap(); let shell = native_shell();
    let script = command(&shell, "printf '%0120d' 0", "[Console]::Write('0' * 120)");
    let tool = ShellTool::new(ToolConfig::new(dir.path(), shell));
    let mut ctx = context(); ctx.run.limits.max_tool_result_bytes = 240;
    let result = tool.execute(ctx.clone(), serde_json::json!({"command":script})).await.unwrap();
    assert_eq!(result.structured.as_ref().unwrap()["exit_code"], 0);
    assert_eq!(result.structured.as_ref().unwrap()["output_bytes"], 120);
    assert!(api::ToolResult::from_output(&ctx.call_id,result.clone()).payload_bytes() <= 240);
    if result.content.byte_len() < 120 || result.structured.as_ref().unwrap()["truncated"] == true {
        assert_eq!(result.structured.as_ref().unwrap()["truncated"], true);
        assert!(result.content.text().contains("not retained"));
    } else { assert_eq!(result.content.text(), "0".repeat(120)); }
}

struct FailingArchive(std::sync::atomic::AtomicUsize);
#[api::async_trait]
impl tools::OutputArchive for FailingArchive {
    fn trigger_bytes(&self) -> usize { 16 }
    fn preview_bytes(&self) -> usize { 8 }
    async fn store(&self, _: &ToolContext, _: &mut (dyn tokio::io::AsyncRead + Unpin + Send), _: usize) -> api::Result<api::ArtifactRef> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err(api::AgentError::new(api::ErrorCode::Tool, "controlled storage failure"))
    }
}
#[tokio::test]
async fn disabled_reader_prevents_upload_and_archive_failure_does_not_claim_command_was_unexecuted() {
    let dir = tempfile::tempdir().unwrap(); let shell = native_shell();
    let script = command(&shell,"printf '%02000d' 0","[Console]::Write('0' * 2000)");
    let archive = Arc::new(FailingArchive(std::sync::atomic::AtomicUsize::new(0)));
    let mut config = ToolConfig::new(dir.path(), shell); config.max_command_bytes = 256;
    config.output_archive = Some(archive.clone()); let tool = ShellTool::new(config);
    let mut ctx = context();
    ctx.run.allowed_tools = Some(Arc::new(["shell".to_owned()].into_iter().collect()));
    let output = tool.execute(ctx, serde_json::json!({"command":script})).await.unwrap();
    assert!(output.artifact.is_none()); assert!(output.content.text().contains("not retained"));
    assert_eq!(archive.0.load(std::sync::atomic::Ordering::SeqCst),0);
    let error = tool.execute(context(), serde_json::json!({"command":script})).await.unwrap_err();
    assert!(error.message.contains("command completed")); assert!(error.message.contains("controlled storage failure"));
    assert_eq!(archive.0.load(std::sync::atomic::Ordering::SeqCst),1);
}

#[tokio::test]
async fn cancellation_and_dropped_future_both_stop_descendant_processes() {
    let shell = native_shell();
    use base64::Engine;
    let child_script = "[IO.File]::WriteAllText('started','ready'); Start-Sleep -Seconds 1; [IO.File]::WriteAllText('leaked','leaked')";
    let child_bytes: Vec<u8> = child_script
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let encoded = base64::engine::general_purpose::STANDARD.encode(child_bytes);
    let powershell = format!("$child = Start-Process -FilePath (Get-Process -Id $PID).Path -ArgumentList '-NoProfile','-NonInteractive','-EncodedCommand','{encoded}' -WorkingDirectory (Get-Location).Path -WindowStyle Hidden -PassThru; $child.WaitForExit()");
    for drop_future in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let tool = ShellTool::new(ToolConfig::new(dir.path(), shell.clone()));
        let script = command(
            &shell,
            "(printf ready > started; sleep 1; printf leaked > leaked) & wait",
            &powershell,
        );
        let ctx = context();
        let cancel = ctx.run.cancel.clone();
        let task = tokio::spawn(async move {
            tool.execute(ctx, serde_json::json!({"command":script}))
                .await
        });
        tokio::time::timeout(Duration::from_secs(4), async {
            while !dir.path().join("started").exists() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("descendant must actually start before testing cancellation");
        if drop_future {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            cancel.cancel();
            let result = tokio::time::timeout(Duration::from_secs(3), task)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(result.unwrap_err().code, api::ErrorCode::Cancelled);
        }
        tokio::time::sleep(Duration::from_millis(1200)).await;
        assert!(
            !dir.path().join("leaked").exists(),
            "descendant wrote a file after its tool was stopped"
        );
    }
}
