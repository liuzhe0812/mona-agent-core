use crate::{bounded, error, public_name, Config, Server, TransportConfig};
use api::{AgentError, CancellationToken, ErrorCode, Result, ToolProgress};
use rmcp::{
    model::{
        ClientCapabilities, ClientConfig, ClientRequest, Implementation, ProgressNotificationParam,
        ServerResult,
    },
    service::{NotificationContext, Peer, PeerRequestOptions, RunningService},
    ClientHandler, RoleClient, ServiceExt,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::RwLock;
use tokio::time::Instant;
const MAX_TOOLS: usize = 64;
#[derive(Clone, Serialize)]
pub struct ToolView {
    pub name: String,
    pub remote_name: String,
    pub description: String,
    pub read_only: bool,
}
#[derive(Clone, Serialize)]
pub struct ServerView {
    pub id: String,
    pub enabled: bool,
    pub connected: bool,
    pub needs_restart: bool,
    pub error: Option<String>,
    pub tools: Vec<ToolView>,
    pub resources: bool,
    pub has_instructions: bool,
}
#[derive(Clone)]
pub(crate) struct Definition {
    pub view: ToolView,
    pub input: Value,
    pub output: Option<Value>,
}
#[derive(Clone, Default)]
pub(crate) struct Catalog {
    pub tools: Vec<Definition>,
    pub instructions: String,
    pub resources: bool,
    pub signature: String,
}
#[derive(Clone, Default)]
struct Handler {
    changed: Arc<AtomicBool>,
    progress: Arc<Mutex<BTreeMap<String, Arc<dyn ToolProgress>>>>,
}
impl ClientHandler for Handler {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new("mona-mcp", env!("CARGO_PKG_VERSION")),
        )
    }
    async fn on_tool_list_changed(&self, _context: NotificationContext<RoleClient>) {
        self.changed.store(true, Ordering::Release);
    }
    async fn on_progress(
        &self,
        params: ProgressNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        let key = serde_json::to_string(&params.progress_token).unwrap_or_default();
        if let Some(progress) = self.progress.lock().ok().and_then(|p| p.get(&key).cloned()) {
            if let Some(message) = params.message {
                progress.report(api::clip_utf8(&message, 512));
            }
        }
    }
}
pub(crate) struct Connection {
    pub id: String,
    pub config: Server,
    live: RwLock<Option<RunningService<RoleClient, Handler>>>,
    catalog: Mutex<Catalog>,
    handler: Handler,
    last_error: Mutex<Option<String>>,
    frozen: AtomicBool,
    stop: CancellationToken,
}
impl Connection {
    fn new(id: String, config: Server) -> Arc<Self> {
        Arc::new(Self {
            id,
            config,
            live: RwLock::new(None),
            catalog: Mutex::new(Catalog::default()),
            handler: Handler::default(),
            last_error: Mutex::new(None),
            frozen: AtomicBool::new(false),
            stop: CancellationToken::new(),
        })
    }
    pub fn catalog(&self) -> Catalog {
        self.catalog.lock().expect("MCP catalog lock").clone()
    }
    pub fn view(&self) -> ServerView {
        let catalog = self.catalog();
        let connected = self.live.try_read().is_ok_and(|s| {
            s.as_ref()
                .is_some_and(|s| !s.is_closed() && !s.peer().is_transport_closed())
        });
        ServerView {
            id: self.id.clone(),
            enabled: self.config.enabled,
            connected,
            needs_restart: self.handler.changed.load(Ordering::Acquire),
            error: self.last_error.lock().expect("MCP status lock").clone(),
            tools: catalog.tools.into_iter().map(|t| t.view).collect(),
            resources: catalog.resources,
            has_instructions: !catalog.instructions.is_empty(),
        }
    }
    pub(crate) fn available(&self) -> bool {
        let v = self.view();
        v.connected && !v.needs_restart && !self.stop.is_cancelled()
    }
    async fn connect(&self) -> Result<()> {
        if self.stop.is_cancelled() {
            return Err(error("MCP service has stopped"));
        }
        if !self.config.enabled {
            return Ok(());
        }
        let mut guard = self
            .live
            .try_write()
            .map_err(|_| error("MCP server has active calls; wait before reconnecting"))?;
        if let Some(mut old) = guard.take() {
            old.close()
                .await
                .map_err(|_| error("MCP previous connection could not be closed"))?;
        }
        let cancel = self.stop.child_token();
        let startup_lifetime = cancel.clone().drop_guard();
        let deadline = Instant::now() + Duration::from_millis(self.config.timeout_ms.min(30_000));
        let handler = self.handler.clone();
        handler.changed.store(false, Ordering::Release);
        let connecting = async {
            let service = match &self.config.transport {
                TransportConfig::Stdio {
                    command,
                    args,
                    env,
                    cwd,
                } => {
                    let mut command=rmcp::transport::which_command(command).map_err(|_|error("MCP executable was not found; install it or provide an absolute executable path"))?;
                    command.args(args).env_clear();
                    command
                        .envs(std::env::vars_os().filter(|(key, _)| {
                            let key = key.to_string_lossy().to_ascii_uppercase();
                            !["KEY", "PASSWORD", "SECRET", "TOKEN"]
                                .iter()
                                .any(|s| key.contains(s))
                                && !key.starts_with("AGENT_")
                                && !key.starts_with("MONA_")
                                && !key.starts_with("DSH_")
                        }))
                        .envs(env);
                    command.current_dir(
                        cwd.as_deref()
                            .map(std::path::Path::new)
                            .unwrap_or_else(|| std::path::Path::new(".")),
                    );
                    command.kill_on_drop(true);
                    #[cfg(windows)]
                    command.creation_flags(0x08000000);
                    let mut wrapped = process_wrap::tokio::CommandWrap::from(command);
                    #[cfg(windows)]
                    wrapped.wrap(process_wrap::tokio::JobObject);
                    #[cfg(unix)]
                    wrapped.wrap(process_wrap::tokio::ProcessGroup::leader());
                    let (transport, _) = rmcp::transport::TokioChildProcess::builder(wrapped)
                        .stderr(std::process::Stdio::null())
                        .spawn()
                        .map_err(|_| {
                            error("MCP process could not start; verify command, arguments and cwd")
                        })?;
                    handler
                        .serve_with_ct(transport, cancel.clone())
                        .await
                        .map_err(|_| error("MCP stdio handshake failed"))?
                }
                TransportConfig::StreamableHttp { url, headers, .. } => {
                    let headers = headers
                        .iter()
                        .map(|(k, v)| {
                            Ok((
                                http::HeaderName::try_from(k.as_str())
                                    .map_err(|_| error("invalid header name"))?,
                                http::HeaderValue::try_from(v.as_str())
                                    .map_err(|_| error("invalid header value"))?,
                            ))
                        })
                        .collect::<Result<_>>()?;
                    let config=rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri(url.as_str())
                        .custom_headers(headers).max_sse_event_size(4*1024*1024).reinit_on_expired_session(false).max_concurrent_requests(8);
                    let client = reqwest::Client::builder()
                        .redirect(reqwest::redirect::Policy::none())
                        .connect_timeout(Duration::from_secs(10))
                        .build()
                        .map_err(|_| error("MCP HTTP client initialization failed"))?;
                    let transport =
                        rmcp::transport::StreamableHttpClientTransport::with_client(client, config);
                    handler
                        .serve_with_ct(transport, cancel.clone())
                        .await
                        .map_err(|_| {
                            error(
                                "MCP HTTP handshake failed; verify endpoint, headers and protocol",
                            )
                        })?
                }
            };
            Ok::<_, AgentError>(service)
        };
        let mut service = tokio::select! {biased;_=self.stop.cancelled()=>{cancel.cancel();return Err(error("MCP connection cancelled"));},_=tokio::time::sleep_until(deadline)=>{cancel.cancel();return Err(error("MCP connection timed out"));},result=connecting=>result?};
        let discovered = self.discover(service.peer(), deadline).await;
        let catalog = match discovered {
            Ok(c) => c,
            Err(e) => {
                let _ = service.close().await;
                return Err(e);
            }
        };
        if self.frozen.load(Ordering::Acquire) && catalog.signature != self.catalog().signature {
            self.handler.changed.store(true, Ordering::Release);
            let _ = service.close().await;
            return Err(error(
                "MCP catalog changed; restart the host to assemble the new tool schemas",
            ));
        }
        *self.catalog.lock().expect("MCP catalog lock") = catalog;
        *guard = Some(service);
        *self.last_error.lock().expect("MCP status lock") = None;
        startup_lifetime.disarm();
        Ok(())
    }
    async fn discover(&self, peer: &Peer<RoleClient>, deadline: Instant) -> Result<Catalog> {
        let info = peer
            .peer_info()
            .ok_or_else(|| error("MCP handshake returned no server identity"))?;
        let instructions = info.instructions.as_deref().unwrap_or("").trim().to_owned();
        if instructions.len() > 16 * 1024 {
            return Err(error("MCP server instructions exceed 16 KiB"));
        }
        let mut tools = Vec::new();
        let mut names = BTreeSet::new();
        let mut cursor = None;
        let mut cursors = BTreeSet::new();
        if info.capabilities.tools.is_some() {
            for page in 0..32 {
                let params = cursor
                    .as_ref()
                    .map(|c| json!({"cursor":c}))
                    .unwrap_or(json!({}));
                let result = self
                    .request_on(peer, "tools/list", params, &self.stop, deadline, None)
                    .await?;
                let list = result
                    .get("tools")
                    .and_then(Value::as_array)
                    .ok_or_else(|| error("invalid MCP tools/list response"))?;
                for tool in list {
                    let raw = tool
                        .get("name")
                        .and_then(Value::as_str)
                        .filter(|s| {
                            !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control)
                        })
                        .ok_or_else(|| error("invalid remote tool name"))?;
                    if !names.insert(raw.to_owned()) {
                        return Err(error("MCP returned duplicate tool names"));
                    }
                    if names.len() > 1024 {
                        return Err(error(
                            "MCP discovery exceeds 1024 tools; configure a smaller server catalog",
                        ));
                    }
                    if !self.config.permits(raw) {
                        continue;
                    }
                    let input = normalize_schema(
                        tool.get("inputSchema")
                            .cloned()
                            .ok_or_else(|| error("MCP tool has no input schema"))?,
                    )?;
                    let output = tool
                        .get("outputSchema")
                        .cloned()
                        .map(normalize_schema)
                        .transpose()?;
                    validate_schema(&input)?;
                    if let Some(s) = &output {
                        validate_schema(s)?;
                    }
                    let description = tool
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or(raw)
                        .to_owned();
                    if description.len() > 16 * 1024 {
                        return Err(error("MCP tool description exceeds 16 KiB"));
                    }
                    if tool
                        .pointer("/execution/taskSupport")
                        .and_then(Value::as_str)
                        == Some("required")
                    {
                        return Err(error("MCP task-based execution is not supported"));
                    }
                    tools.push(Definition {
                        view: ToolView {
                            name: public_name(&self.id, raw),
                            remote_name: raw.into(),
                            description,
                            read_only: self.config.read_only_tools.contains(raw),
                        },
                        input,
                        output,
                    });
                    if tools.len() > MAX_TOOLS {
                        return Err(error("MCP enabled tool limit is 64; narrow allow_tools"));
                    }
                }
                cursor = result
                    .get("nextCursor")
                    .and_then(Value::as_str)
                    .map(String::from);
                match &cursor {
                    None => break,
                    Some(c) => {
                        if c.len() > 4096 || !cursors.insert(c.clone()) || page == 31 {
                            return Err(error("invalid or excessive MCP pagination"));
                        }
                    }
                }
            }
        }
        tools.sort_by(|a, b| a.view.name.cmp(&b.view.name));
        let mut public = BTreeSet::new();
        for t in &tools {
            if !public.insert(&t.view.name) {
                return Err(error("MCP public tool identity collision"));
            }
        }
        let resources = info.capabilities.resources.is_some();
        let signature=serde_json::to_string(&json!({"tools":tools.iter().map(|t|json!({"view":t.view,"input":t.input,"output":t.output})).collect::<Vec<_>>(),"instructions":instructions,"resources":resources})).map_err(|_|error("MCP catalog encoding failed"))?;
        if signature.len() > 512 * 1024 {
            return Err(error("MCP catalog exceeds 512 KiB"));
        }
        Ok(Catalog {
            tools,
            instructions,
            resources,
            signature,
        })
    }
    pub(crate) async fn request(
        &self,
        method: &str,
        params: Value,
        cancel: &CancellationToken,
        deadline: Instant,
        progress: Option<Arc<dyn ToolProgress>>,
    ) -> Result<Value> {
        if self.handler.changed.load(Ordering::Acquire) {
            return Err(error(
                "MCP catalog changed; restart the host before using the changed tools",
            ));
        }
        let live = self.live.read().await;
        let service=live.as_ref().filter(|s|!s.is_closed()&&!s.peer().is_transport_closed()).ok_or_else(||error("MCP server is disconnected; reconnect explicitly, do not replay unknown side effects"))?;
        let deadline = deadline.min(Instant::now() + Duration::from_millis(self.config.timeout_ms));
        self.request_on(service.peer(), method, params, cancel, deadline, progress)
            .await
    }
    async fn request_on(
        &self,
        peer: &Peer<RoleClient>,
        method: &str,
        params: Value,
        cancel: &CancellationToken,
        deadline: Instant,
        progress: Option<Arc<dyn ToolProgress>>,
    ) -> Result<Value> {
        bounded(&params, 128 * 1024, "arguments")?;
        if cancel.is_cancelled() || self.stop.is_cancelled() {
            return Err(AgentError::new(
                ErrorCode::Cancelled,
                "MCP request cancelled",
            ));
        }
        if Instant::now() >= deadline {
            return Err(AgentError::new(
                ErrorCode::Deadline,
                "MCP request deadline reached",
            ));
        }
        let request: ClientRequest =
            serde_json::from_value(json!({"method":method,"params":params}))
                .map_err(|_| error("unsupported MCP request"))?;
        let handle = tokio::select! {biased;_=cancel.cancelled()=>return Err(AgentError::new(ErrorCode::Cancelled,"MCP request cancelled")),_=self.stop.cancelled()=>return Err(error("MCP service stopped")),_=tokio::time::sleep_until(deadline)=>return Err(AgentError::new(ErrorCode::Deadline,"MCP request deadline reached")),r=peer.send_cancellable_request(request,PeerRequestOptions::with_timeout(deadline.saturating_duration_since(Instant::now())))=>r.map_err(|_|error("MCP request could not be sent; outcome may be unknown, not retried"))?};
        let id = handle.id.clone();
        let key = serde_json::to_string(&handle.progress_token).unwrap_or_default();
        let mut abandoned = CancelOnDrop {
            peer: peer.clone(),
            id: Some(id.clone()),
        };
        if let Some(progress) = progress {
            self.handler
                .progress
                .lock()
                .expect("MCP progress lock")
                .insert(key.clone(), progress);
        }
        let cleanup = ProgressGuard {
            handler: self.handler.clone(),
            key,
        };
        let result = tokio::select! {biased;_=cancel.cancelled()=>Err(AgentError::new(ErrorCode::Cancelled,"MCP call cancelled; remote effects are not rolled back")),_=self.stop.cancelled()=>Err(error("MCP service stopped")),_=tokio::time::sleep_until(deadline)=>Err(AgentError::new(ErrorCode::Deadline,"MCP call timed out; not replayed")),r=handle.await_response()=>r.map_err(|_|error("MCP remote request failed; not retried"))};
        drop(cleanup);
        if result.is_err() {
            if let Ok(notification) = serde_json::from_value(
                json!({"method":"notifications/cancelled","params":{"requestId":id,"reason":"caller stopped waiting"}}),
            ) {
                let _ = tokio::time::timeout(
                    Duration::from_millis(250),
                    peer.send_notification(notification),
                )
                .await;
            }
        }
        abandoned.id = None;
        let result: ServerResult = result?;
        let value =
            serde_json::to_value(result).map_err(|_| error("MCP response encoding failed"))?;
        bounded(&value, 4 * 1024 * 1024, "response")?;
        Ok(value)
    }
}
struct CancelOnDrop {
    peer: Peer<RoleClient>,
    id: Option<rmcp::model::RequestId>,
}
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            let peer = self.peer.clone();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
            if let Ok(notification)=serde_json::from_value(json!({"method":"notifications/cancelled","params":{"requestId":id,"reason":"caller dropped the request"}})){let _=tokio::time::timeout(Duration::from_millis(250),peer.send_notification(notification)).await;}
        });
            }
        }
    }
}
struct ProgressGuard {
    handler: Handler,
    key: String,
}
impl Drop for ProgressGuard {
    fn drop(&mut self) {
        if let Ok(mut p) = self.handler.progress.lock() {
            p.remove(&self.key);
        }
    }
}
fn normalize_schema(mut schema: Value) -> Result<Value> {
    let object = schema
        .as_object_mut()
        .ok_or_else(|| error("MCP schema must be an object"))?;
    object
        .entry("$schema")
        .or_insert(json!("https://json-schema.org/draft/2020-12/schema"));
    match object
        .get("$schema")
        .and_then(Value::as_str)
        .map(|s| s.trim_end_matches('#'))
    {
        Some(
            "https://json-schema.org/draft/2020-12/schema"
            | "https://json-schema.org/draft/2019-09/schema"
            | "http://json-schema.org/draft-07/schema"
            | "https://json-schema.org/draft-07/schema",
        ) => {}
        _ => return Err(error("unsupported MCP JSON Schema dialect")),
    }
    Ok(schema)
}
pub(crate) fn validate_schema(schema: &Value) -> Result<()> {
    bounded(schema, 64 * 1024, "schema")?;
    fn refs(v: &Value) -> Result<()> {
        match v {
            Value::Object(m) => {
                if ["$ref", "$dynamicRef", "$recursiveRef"].iter().any(|key| {
                    m.get(*key)
                        .is_some_and(|r| r.as_str().is_none_or(|s| !s.starts_with('#')))
                }) {
                    return Err(error("MCP schema external references are not allowed"));
                }
                for v in m.values() {
                    refs(v)?;
                }
            }
            Value::Array(a) => {
                for v in a {
                    refs(v)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    refs(schema)?;
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        return Err(error("MCP tool schemas must be objects"));
    }
    jsonschema::JSONSchema::options()
        .compile(schema)
        .map_err(|_| error("MCP tool schema is invalid"))?;
    Ok(())
}
pub struct Service {
    pub(crate) connections: BTreeMap<String, Arc<Connection>>,
}
impl Service {
    pub async fn connect(config: Config, cancel: CancellationToken) -> Result<Arc<Self>> {
        config.validate()?;
        let mut connections = BTreeMap::new();
        let mut total = 0;
        for (id, server) in config.servers {
            if cancel.is_cancelled() {
                break;
            }
            let entry = Connection::new(id.clone(), server);
            let result = tokio::select! {_=cancel.cancelled()=>Err(AgentError::new(ErrorCode::Cancelled,"MCP startup cancelled")),r=entry.connect()=>r};
            if let Err(e) = result {
                *entry.last_error.lock().expect("MCP status lock") = Some(e.message);
            }
            total += entry.catalog().tools.len();
            entry.frozen.store(true, Ordering::Release);
            connections.insert(id, entry);
        }
        let service = Arc::new(Self { connections });
        if cancel.is_cancelled() {
            service.shutdown().await?;
            return Err(AgentError::new(
                ErrorCode::Cancelled,
                "MCP startup cancelled",
            ));
        }
        if total > MAX_TOOLS {
            service.shutdown().await?;
            return Err(error(
                "MCP aggregate enabled tool limit is 64; narrow allow_tools",
            ));
        }
        Ok(service)
    }
    pub fn views(&self) -> Vec<ServerView> {
        self.connections.values().map(|c| c.view()).collect()
    }
    pub async fn reconnect(&self, id: &str) -> Result<ServerView> {
        let c = self
            .connections
            .get(id)
            .ok_or_else(|| error("unknown MCP server"))?;
        let result = c.connect().await;
        if let Err(e) = &result {
            *c.last_error.lock().expect("MCP status lock") = Some(e.message.clone());
        }
        result?;
        Ok(c.view())
    }
    pub async fn shutdown(&self) -> Result<()> {
        for c in self.connections.values() {
            c.stop.cancel();
        }
        let mut failed = false;
        for c in self.connections.values() {
            if let Some(mut service) = c.live.write().await.take() {
                failed |= service.close().await.is_err();
            }
        }
        if failed {
            Err(error("MCP transport cleanup failed"))
        } else {
            Ok(())
        }
    }
}
