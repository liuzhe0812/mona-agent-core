# Optional model management

This crate provides provider configuration, a model catalog, visibility preferences,
default selection, bounded protocol-aware model discovery, and a model router.
It depends on the public Agent API and Providers, not Runtime, Application, HTTP Bridge or Tauri.
Supported protocols are `chat_completions`, `responses`, and `messages`; selection is explicit, not inferred from a model/brand name.

## Host assembly

1. Construct `ModelManager::open(store, allow_http_loopback)` in the trusted host.
2. Supply an initial provider through `seed` for a new store, or configure via `upsert`.
3. Install `manager.plugin()` on `HostBuilder`, without also calling `.model(...)`.
4. Wrap the built engine with `manager.runtime(Arc::new(host.engine()))` and inject
   that `AgentRuntime` into the existing `AgentApplication`.
5. Retain the manager in the host to implement authenticated management routes/commands.

The plugin registers only `agent.model`. It does not register a management Tool or
publish the administrative handle to runtime services. Unmanaged hosts continue to
use `HostBuilder.model(...)` as before; no public task request or stream fields change.

The runtime decorator captures the default provider, model, endpoint, and credential
once per Run. All rounds and auxiliary calls that inherit the run model use that
snapshot. Editing or deleting a provider does not switch an already-running task.
Bindings remain alive until the underlying session finishes, even if the caller
drops its handle. Core still owns execution, budgets, cancellation and tool policy.
`execute()` uses the shared `RunHandle::wait_owned()` contract: dropping its pending
future cancels that Run, without cancelling a shared task or another Run. `start()`
and ordinary `wait()` remain passive; a detached observer does not cancel execution.
The trusted request audit contains an internal `managed:<provider>:<revision>:<sequence>`
selector; the router replaces it with the actual model ID at the adapter boundary.
Per-run model overrides and crash recovery are not implemented by this module.

## History preflight and model switching

`ManagedRuntime::validate_history` checks the current default adapter locally. It is
not a model reservation: `start` binds the actual selected model and checks again
before the engine starts or any context transform calls a summarizer. Router and
session wrappers forward the same hook. Plain histories can switch; incompatible
private replay fails with `ModelHistoryIncompatible`, without a model call or silent
conversion. The application presents a new-session instruction; it never creates one
or falls back to another model automatically. The current session extension runs this
check before saving a new turn. A configuration race after that preflight can still
produce a saved failed input, but cannot send incompatible history to the new model.

## Protocol and model facts

Upsert/discovery require `protocol`. Each provider uses an API base plus the selected
protocol resource path; a full matching endpoint is normalized once, while a mismatched
protocol suffix is rejected. Custom HTTPS bases are supported; loopback HTTP requires host opt-in.
The same `providers::create_model` factory powers managed and fixed-model hosts.

Model entries include optional `capabilities` (tools, images, sampling fields, stop and
maximum output). Unknown facts remain unknown; discovery returns only IDs and never replaces
configured facts. Native `generation` accepts the whitelisted reasoning/thinking options
specified in [Providers](../providers/README.md); omitting it preserves the current profile,
an empty object explicitly clears it. Arbitrary Chat deployment options are not exposed to the UI.
Protocol, capabilities, window and generation profile are captured with the Run binding.

Only settings document version 2 is read. Missing/unknown protocol and obsolete formats are
rejected, not migrated or silently reset. Changing a saved endpoint/protocol requires re-entering
its API key or explicitly clearing it; a stored credential is never implicitly sent to a new route.
Discovery uses Bearer for Chat/Responses and x-api-key plus anthropic-version for Messages,
requests at most 256 IDs, and does not claim all catalog pages have been retrieved.

## Model context capacity

`ModelEntry.context_window_tokens: Option<u64>` is a trusted per-provider/per-model
capacity (1..1,000,000,000). `None` remains unknown. Model discovery
returns IDs, not guessed capacities. The formal UI supports setting/clearing the value
and preserves it when refreshing IDs or toggling visibility.

The captured adapter forwards that model's configured capacity to the existing Model
contract. Changes do not affect already-bound Runs. The host may import
`AGENT_MODEL_CONTEXT_TOKENS` only when seeding a new environment-backed provider;
existing saved values are not overwritten. Runtime/compaction still own output reserve,
byte limits and approximate pressure decisions; this crate does not implement a meter
or retry loop. See [context management](../../docs/CONTEXT-MANAGEMENT.zh-CN.md).

## Persistence

`SettingsStore` is an injectable, synchronous store. The supplied `EncryptedFileStore`
uses AES-256-GCM with a fresh random nonce and a versioned file. A separate high-entropy
key supplied by the host is required; SHA-256 converts it into the encryption key and
is not password hardening. Use a generated secret, not a human password. The store
uses a same-directory temporary file, sync and atomic replacement. A failed write
does not publish a new in-memory configuration. Corrupt data or a wrong key fails
closed rather than resetting the configuration. Protect/back up the key separately;
this implementation does not manage key rotation or OS keychains.

Each write supplies the last observed revision. Stale writes fail instead of overwriting
newer data. The store is intended for one manager process; shared files across server
processes require a transactional external store. Hosts should run writes on a blocking
worker, as the server example does.

Public views contain only `has_key`, never stored credentials. Omitted/blank API keys
retain an existing value; `clear_key` explicitly removes it. Stored credentials are
not reused across endpoint/protocol changes without re-entry or explicit clearing.
Discovery never follows redirects, times out after 15 seconds, caps responses at
1 MiB and returns at most 256 model IDs. HTTPS endpoints are allowed; loopback HTTP
requires trusted host opt-in. Production hosts own network egress restrictions,
user authorization and tenant isolation.

## Reference Web host

`server` enables its optional `model-management` Cargo feature by
default. `--no-default-features` omits this plugin and its management routes entirely.
`AGENT_MODEL_MANAGEMENT=0` also selects the original fixed-model assembly at runtime.
The existing `--demo` mode remains explicitly offline and does not install management.

Management endpoints live in `apps/server/src/model_settings.rs`, separately
from the reusable HTTP task Bridge. They require the host bearer token, explicit CORS
origins, bounded JSON bodies and return `Cache-Control: no-store`. In this reference
single-authority host, that token grants both task and configuration access. A public
multi-user product must apply its own admin authorization and per-domain manager.

- `GET /api/model-settings`: redacted snapshot.
- `POST /api/model-settings/providers`: create/edit a provider and model list.
- `POST /api/model-settings/default`: select the default for new Runs.
- `POST /api/model-settings/visibility`: update one/all display switches; keeps the default visible.
- `POST /api/model-settings/delete`: delete a non-default provider.
- `POST /api/model-settings/discover`: read models from a draft or saved connection.

An empty first startup is valid. When both `AGENT_MODEL_ENDPOINT` and
`AGENT_MODEL_NAME` exist, the host imports them with optional `AGENT_MODEL_KEY` and
`AGENT_MODEL_EXTRA_JSON` and `AGENT_MODEL_PROTOCOL` (new host default: chat_completions); subsequent starts use the saved document. Production hosts
set `AGENT_MODEL_STORE_KEY` to a stable random secret of at least 16 characters.

`AGENT_MODEL_SETTINGS_PATH` overrides the encrypted file path. Otherwise it lives at
`LOCALAPPDATA/mona-agent-core/model-settings.enc` on Windows, or under
`XDG_STATE_HOME` / `$HOME/.local/state` on Unix. There are no credentials in the repo
or browser storage. `AGENT_ALLOW_HTTP_LOOPBACK=1` enables local model servers.

The development launcher creates and reuses a local `model-store.key` when no key is
injected, while model catalogs and provider credentials remain behind the host
management API. The Web UI separately discovers that API. Tauri hosts can implement
the same operations using the manager without depending on HTTP; no new native commands
are installed by this crate.
