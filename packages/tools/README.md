# tools

Reusable filesystem and command tools. `core_tools` returns the current Web product's `read`, `shell`, `edit`, and `write` bundle; it does not define the mandatory tools of every Agent. `optional_tool` supplies `grep`, `find`, or `ls` when a trusted host enables them. All tools use the public `api::Tool` contract and the Runtime's existing validation, cancellation, permission, and result limits.

`ToolConfig` fixes the working directory, `ShellConfig`, output limits and host-owned `read` extensions. Relative paths resolve from that directory; absolute paths remain available for host-disclosed Skill files. Working-directory binding alone is not a sandbox; the optional native integration below must be explicitly installed.

The public shell types are `ShellTool`, `ShellConfig`, and `ShellKind`. `ShellConfig::discover()` is the normal host path; `ShellConfig::new` is available when a trusted host must choose an explicit executable and syntax.

- `read`: 1-based text-line paging, bounded UTF-8 output, decoded/resized PNG/JPEG/WebP/GIF/BMP images, and opaque output locators.
- `shell`: one-shot command execution using the configured backend syntax, with combined stdout/stderr, nonzero exit as a tool error, effective task/tool deadlines and process-tree cancellation. Windows uses a Job Object; Unix uses a process group. Dropping the execution future also terminates the group. The model-visible description names the backend syntax, so commands must use PowerShell, Bash, or POSIX `sh` syntax as configured. Descendants are cleaned up when the command ends, not retained as background sessions.
- `edit`: exact-first, limited normalized matching of unique, non-overlapping `oldText/newText` replacements, with a real bounded unified diff.
- `write`: atomic UTF-8 create or overwrite with parent creation. It shares the mutation lock with `edit`; the read/modify/commit operation is serialized per canonical destination, including across Runs. The blocking worker retains the lock until the write settles even if its awaiting future is dropped. Files are limited to 32 MiB; existing permissions are preserved.
- `grep/find/ls`: optional bounded implementations with no external `rg` or `fd` dependency. `grep` scans the complete file for its literal `query`; `find` matches file paths with globs. Discovery respects gitignore rules. Result-count and output truncation are explicit.

The formal Server authorizes the side-effect tools and always installs the four core tools. Future tools implement `api::Tool` and join the same registry; duplicate names fail during host construction.

Runtime validates every model-supplied argument object before entering a tool. Invalid calls return a bounded model-visible error with field paths and reasons; parameter values are not echoed, and the tool body is not invoked.

## Integration

The host supplies a model, an existing workspace directory and a discovered or explicitly configured shell. The optional `ToolConfig.output_archive` receives completed command captures; matching `read_extensions` resolve its locators. Both capabilities are supplied by the host. This example runs inside an async function returning `api::Result`:

```rust,ignore
let config = tools::ToolConfig::new(
    workspace_dir,
    tools::ShellConfig::discover()?,
);
let mut builder = runtime::HostBuilder::new().model(model);
for tool in tools::core_tools(&config) {
    builder = builder.tool(tool);
}
// Explicit authority from the trusted host.
for name in ["shell", "edit", "write"] {
    builder = builder.allow_side_effect_tool(name);
}
let mut host = builder.build().await?;
// Use host.engine() to start tasks, then shut down when the host is finished.
host.shutdown().await?;
```

Add optional tools with `tools::optional_tool(name, &config)` for `grep`, `find`, or `ls`; unknown names return `None`. The host also owns the Runtime dependency and task limits. The tools package itself depends on the public `api` contracts, not on Runtime internals or UI. See the [package index](../README.md) and [host capability configuration](../../docs/CAPABILITY-ASSEMBLY.zh-CN.md).

## Optional local sandbox

Enable the `tools/sandbox` Cargo feature and assign `ToolConfig.sandbox = Some(SandboxBinding::new(provider, resolver))`. The resolver receives trusted `RunContext`, not model arguments, and returns the immutable run mode, actual workspace root and optional Session ID. No sandbox dependency is linked without this feature; enabling the feature without a binding does not silently restrict or widen an existing host.

`ShellTool` prepares exact argv through [sandbox](../sandbox/README.md), retains the native lease through process-tree completion, and counts preparation time within the existing tool/task deadline. Captured output, exit status, streaming archive and cancellation keep their original owners. A required backend failure is never retried unconfined. Bounded backend/diagnostic metadata is included in the tool result and `sandbox.execution` UI detail; stderr classification is not retry authority.

`write/edit` share the same resolved policy in their existing atomic mutation path. The checked canonical target is the target actually modified, with repeat checks before creating directories and publishing the replacement; no second write/edit implementation is introduced. Read tools remain unrestricted by this file-effect policy. Host-owned output capture, checkpoints and Memory storage are not untrusted payload operations and do not inherit the file fence automatically.

Windows read-only PowerShell may use ConstrainedLanguage. Its wrapper avoids FullLanguage-only initialization in that mode; the trusted native launcher configures UTF-8 console encoding. Shell syntax, tool authorization and model permissions otherwise remain unchanged. The caller shuts down the sandbox provider after its tasks, and releases persistent-session temporary state on deletion.

## Command output

`OutputArchive` is a small injected retention interface: trigger/preview sizes and a streaming `store` operation returning an `ArtifactRef`. Tools have no indexed archive and no private locator scheme. The formal Server connects this interface to the same Spill backend used by result transforms; all retained command output uses `spill:` and the existing same-conversation read authorization.

Short output stays inline. Larger output is spooled to an anonymous temporary file while only a bounded preview remains in memory, then streamed to the archive at completion. The temporary handle has no public URI and disappears on drop. Raw and decoded output each have an 8 MiB per-command cap. The archive backend owns run/global quotas, retention, permissions and durable publication. A locator is returned only after the full capture commits; disk/quota/cancellation failures never publish a successful partial archive. Stored command output is normalized UTF-8: valid split characters are preserved per pipe, invalid bytes are replaced and marked in `utf8_replacements`.

Without an archive, or when the Run cannot use `read`, output remains bounded and explicitly states that omitted bytes were not retained. No dead locator is returned. Exit status and metadata are preserved; result limits include metadata and the artifact descriptor. Archive paging uses 1-based byte offsets while ordinary text-file paging uses lines. The package does not depend on Spill, Server or Runtime.

## Verification

Run `cargo test -p tools`. Shell tests require the configured backend and fail with setup guidance when it is missing; they do not silently return success. Set `SHELL_PATH` to override the executable for tests. This is a test-only override and does not replace deployment discovery.

## Host requirements

Shell discovery and missing-dependency feedback belong to the host. `ShellConfig::discover()` selects `pwsh.exe` first on Windows, then the system Windows PowerShell executable; on Unix it selects `bash` first, then `sh`. Windows does not depend on Git Bash. A deployment may set `AGENT_SHELL_PATH`; `AGENT_BASH_PATH` is retained only for old deployments, and an explicit `AGENT_SHELL_PATH` wins. This package accepts the resolved shell supplied by the host and reports startup errors through the tool result path.
