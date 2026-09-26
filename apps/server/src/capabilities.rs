//! Host-owned capability assembly policy and end-user preference storage.
use axum::{
    extract::{rejection::JsonRejection, DefaultBodyLimit, Path, Request, State},
    http::{header, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path as FilePath, PathBuf},
    sync::{Arc, Mutex},
};
use subtle::ConstantTimeEq;
use tower_http::cors::CorsLayer;

const CONFIG_VERSION: u32 = 1;
const MODEL_MANAGEMENT: &str = "model-management";
const SKILLS: &str = "skills";
pub const COMPACTION: &str = "compaction";
pub const SPILL: &str = "spill";
pub const INSTRUCTIONS: &str = "instructions";
pub const MEMORY: &str = "memory";
pub const MEMORY_UPDATE: &str = "memory_update";
pub const HISTORY_SEARCH: &str = "history-search";
pub const PLANNER: &str = "planner";
pub const SANDBOX: &str = "sandbox";
pub const SUBAGENT: &str = "subagent";
pub const MCP: &str = "mcp";
pub const GREP: &str = "grep";
pub const FIND: &str = "find";
pub const LS: &str = "ls";
const FIXED_TOOLS: [&str; 4] = ["read", "shell", "edit", "write"];

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct DeploymentFile {
    version: u32,
    capabilities: BTreeMap<String, CapabilityPolicy>,
}
impl Default for DeploymentFile {
    fn default() -> Self {
        let mut capabilities = BTreeMap::new();
        capabilities.insert(
            MODEL_MANAGEMENT.into(),
            CapabilityPolicy {
                enabled: cfg!(feature = "model-management"),
                user_configurable: false,
            },
        );
        capabilities.insert(
            SKILLS.into(),
            CapabilityPolicy {
                // The release binary includes Skills, but a fresh host must opt in
                // before discovery can scan project or user skill roots.
                enabled: false,
                user_configurable: true,
            },
        );
        capabilities.insert(
            COMPACTION.into(),
            CapabilityPolicy {
                enabled: cfg!(feature = "compaction"),
                user_configurable: true,
            },
        );
        capabilities.insert(
            SPILL.into(),
            CapabilityPolicy {
                enabled: cfg!(feature = "spill"),
                user_configurable: true,
            },
        );
        capabilities.insert(
            INSTRUCTIONS.into(),
            CapabilityPolicy {
                enabled: true,
                user_configurable: true,
            },
        );
        for (id, enabled) in [(MEMORY, cfg!(feature = "memory")), (HISTORY_SEARCH, cfg!(feature = "history-search")), (PLANNER, cfg!(feature = "planner")), (SANDBOX, cfg!(feature = "sandbox")), (SUBAGENT, cfg!(feature = "subagent")), (MCP, cfg!(feature = "mcp")), (MEMORY_UPDATE, false)] {
            capabilities.insert(id.into(), CapabilityPolicy { enabled, user_configurable: true });
        }
        for id in FIXED_TOOLS {
            capabilities.insert(
                id.into(),
                CapabilityPolicy {
                    enabled: true,
                    user_configurable: false,
                },
            );
        }
        for id in [GREP, FIND, LS] {
            capabilities.insert(
                id.into(),
                CapabilityPolicy {
                    enabled: false,
                    user_configurable: true,
                },
            );
        }
        Self {
            version: CONFIG_VERSION,
            capabilities,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CapabilityPolicy {
    enabled: bool,
    #[serde(default)]
    user_configurable: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct UserState {
    version: u32,
    revision: u64,
    enabled: BTreeMap<String, bool>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CapabilityView {
    revision: u64,
    components: Vec<CapabilityEntry>,
    tools: Vec<CapabilityEntry>,
}

#[derive(Clone, Debug, Serialize)]
struct CapabilityEntry {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    enabled: bool,
    restart_required: bool,
}

#[derive(Clone)]
pub struct CapabilityManager {
    inner: Arc<Inner>,
}
struct Inner {
    policies: BTreeMap<String, CapabilityPolicy>,
    active: BTreeMap<String, bool>,
    state_path: PathBuf,
    state: Mutex<UserState>,
}

impl CapabilityManager {
    fn open(
        mut deployment: DeploymentFile,
        state_path: PathBuf,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        if deployment.version != CONFIG_VERSION {
            return Err(format!("unsupported agent.toml version: {}", deployment.version).into());
        }
        // Accept an older deployment's fixed bash entry as shell. Explicit new
        // shell configuration takes precedence; optional choices remain intact.
        if let Some(legacy) = deployment.capabilities.remove("bash") {
            deployment
                .capabilities
                .entry("shell".into())
                .or_insert(legacy);
        }
        for id in deployment.capabilities.keys() {
            if !matches!(
                id.as_str(),
                MODEL_MANAGEMENT
                    | SKILLS
                    | COMPACTION
                    | SPILL
                    | INSTRUCTIONS
                    | MEMORY
                    | MEMORY_UPDATE
                    | HISTORY_SEARCH
                    | PLANNER
                    | SANDBOX
                    | SUBAGENT
                    | MCP
                    | "read"
                    | "shell"
                    | "edit"
                    | "write"
                    | GREP
                    | FIND
                    | LS
            ) {
                return Err(format!("unknown capability in agent.toml: {id}").into());
            }
        }
        for id in FIXED_TOOLS {
            if let Some(policy) = deployment.capabilities.get(id) {
                if !policy.enabled || policy.user_configurable {
                    return Err(format!("base tool {id} is fixed and must remain enabled").into());
                }
            }
        }
        let defaults = DeploymentFile::default();
        let mut policies = defaults.capabilities;
        policies.extend(deployment.capabilities);
        for (id, policy) in &policies {
            if policy.enabled && !compiled(id) {
                return Err(format!(
                    "capability {id} is enabled but was not compiled into this server"
                )
                .into());
            }
        }
        let state = read_state(&state_path)?;
        let active = policies
            .iter()
            .map(|(id, policy)| {
                let value = desired(id, policy, &state);
                (id.clone(), value && compiled(id))
            })
            .collect();
        Ok(Self {
            inner: Arc::new(Inner {
                policies,
                active,
                state_path,
                state: Mutex::new(state),
            }),
        })
    }

    #[allow(dead_code)]
    pub fn active(&self, id: &str) -> bool {
        self.inner.active.get(id).copied().unwrap_or(false)
    }

    /// Product-only inventory, including fixed and inactive capabilities. Never a permission token.
    pub fn assembly(&self) -> BTreeMap<String, serde_json::Value> {
        let state = self.inner.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self.inner.policies.iter().map(|(id, policy)| {
            let enabled = desired(id, policy, &state);
            let active = self.active(id);
            (id.clone(), serde_json::json!({"compiled":compiled(id),"enabled":enabled,"active":active,
                "configurable":policy.user_configurable,"restart_required":enabled != active}))
        }).collect()
    }
    pub fn view(&self) -> CapabilityView {
        let state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.view_with(&state)
    }

    fn view_with(&self, state: &UserState) -> CapabilityView {
        let mut components = Vec::new();
        let mut tools = Vec::new();
        for item in manageable_metadata() {
            let policy = &self.inner.policies[item.id];
            // The end-user pages contain settings, not an inventory. Fixed base
            // requirements, deployment-locked entries and unavailable builds stay hidden.
            if !policy.user_configurable || !compiled(item.id) {
                continue;
            }
            let enabled = desired(item.id, policy, state);
            let active = self.inner.active.get(item.id).copied().unwrap_or(false);
            let entry = CapabilityEntry {
                id: item.id,
                name: item.name,
                description: item.description,
                enabled,
                restart_required: active != enabled,
            };
            match item.kind {
                CapabilityKind::Component => components.push(entry),
                CapabilityKind::Tool => tools.push(entry),
            }
        }
        CapabilityView {
            revision: state.revision,
            components,
            tools,
        }
    }

    fn set_enabled(
        &self,
        revision: u64,
        id: &str,
        enabled: bool,
    ) -> Result<CapabilityView, ApiError> {
        let policy = self
            .inner
            .policies
            .get(id)
            .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "capability not found".into()))?;
        if !compiled(id) {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "capability is not available in this server".into(),
            ));
        }
        if !policy.user_configurable {
            return Err(ApiError(
                StatusCode::FORBIDDEN,
                "capability is controlled by the deployment".into(),
            ));
        }
        let mut state = self.inner.state.lock().map_err(|_| {
            ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                "capability state unavailable".into(),
            )
        })?;
        if state.revision != revision {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "capability_settings_conflict: refresh before saving".into(),
            ));
        }
        let mut next = state.clone();
        next.version = CONFIG_VERSION;
        next.revision = next.revision.checked_add(1).ok_or_else(|| {
            ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                "capability revision exhausted".into(),
            )
        })?;
        next.enabled.insert(id.into(), enabled);
        write_state(&self.inner.state_path, &next).map_err(|_| {
            ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                "capability settings could not be saved".into(),
            )
        })?;
        *state = next;
        Ok(self.view_with(&state))
    }
}

fn desired(id: &str, policy: &CapabilityPolicy, state: &UserState) -> bool {
    if policy.user_configurable {
        state.enabled.get(id).copied().unwrap_or(policy.enabled)
    } else {
        policy.enabled
    }
}

fn compiled(id: &str) -> bool {
    match id {
        MODEL_MANAGEMENT => cfg!(feature = "model-management"),
        SKILLS => cfg!(feature = "skills"),
        COMPACTION => cfg!(feature = "compaction"),
        SPILL => cfg!(feature = "spill"),
        MEMORY | MEMORY_UPDATE => cfg!(feature = "memory"),
        HISTORY_SEARCH => cfg!(feature = "history-search"),
        PLANNER => cfg!(feature = "planner"),
        SANDBOX => cfg!(feature = "sandbox"),
        SUBAGENT => cfg!(feature = "subagent"),
        MCP => cfg!(feature = "mcp"),
        "read" | "shell" | "edit" | "write" | GREP | FIND | LS | INSTRUCTIONS => true,
        _ => false,
    }
}

#[derive(Clone, Copy)]
enum CapabilityKind {
    Component,
    Tool,
}

struct Metadata {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    kind: CapabilityKind,
}

fn manageable_metadata() -> Vec<Metadata> {
    vec![
        Metadata { id: MCP, name: "MCP", description: "连接本地或远程 MCP 服务，按宿主配置使用外部工具和资源。", kind: CapabilityKind::Component },
        Metadata { id: SUBAGENT, name: "子 Agent", description: "有限并行委派，父子共用预算和权限边界；保留独立记录。", kind: CapabilityKind::Component },
        Metadata { id: SANDBOX, name: "本地沙箱", description: "限制 Agent Shell 与文件修改；模式由输入框选择，不限制读取和联网。", kind: CapabilityKind::Component },
        Metadata { id: PLANNER, name: "计划管理", description: "同一个 Agent 按需维护计划；显式计划模式只调研，普通任务不额外审批。", kind: CapabilityKind::Component },
        Metadata { id: MEMORY, name: "长期记忆", description: "按授权范围注入少量精选长期事实；关闭不删除已保存记忆。", kind: CapabilityKind::Component },
        Metadata { id: HISTORY_SEARCH, name: "历史会话检索", description: "按需搜索和读取已保存原文，不影响会话保存与恢复。", kind: CapabilityKind::Component },
        Metadata { id: MEMORY_UPDATE, name: "Agent 更新记忆", description: "授权 Agent 新增、纠正或删除长期记忆，不逐次确认；需要启用长期记忆组件。", kind: CapabilityKind::Tool },
        Metadata {
            id: INSTRUCTIONS,
            name: "项目规则",
            description:
                "加载工作空间及相关子目录的 AGENTS.md / CLAUDE.md，刷新规则变更；不扩大工具权限。",
            kind: CapabilityKind::Component,
        },
        Metadata {
            id: MODEL_MANAGEMENT,
            name: "模型管理",
            description: "提供供应商、凭据、模型目录和默认模型管理。",
            kind: CapabilityKind::Component,
        },
        Metadata {
            id: SKILLS,
            name: "Skills",
            description: "从宿主允许的目录发现并按需读取技能说明。",
            kind: CapabilityKind::Component,
        },
        Metadata {
            id: COMPACTION,
            name: "上下文压缩",
            description: "上下文接近限制时，使用当前模型压缩较早的已结算历史。",
            kind: CapabilityKind::Component,
        },
        Metadata {
            id: SPILL,
            name: "长结果归档",
            description: "将过长的纯文本工具结果保存在宿主私有目录，并提供受控分页读取。",
            kind: CapabilityKind::Component,
        },
        Metadata {
            id: GREP,
            name: "内容搜索",
            description: "递归搜索文件内容。",
            kind: CapabilityKind::Tool,
        },
        Metadata {
            id: FIND,
            name: "文件查找",
            description: "按名称模式递归查找文件。",
            kind: CapabilityKind::Tool,
        },
        Metadata {
            id: LS,
            name: "目录列表",
            description: "列出目录内容。",
            kind: CapabilityKind::Tool,
        },
    ]
}

pub fn from_environment(args: &[String]) -> Result<CapabilityManager, Box<dyn std::error::Error>> {
    let mut explicit_config = std::env::var_os("AGENT_CONFIG_PATH").map(PathBuf::from);
    let mut position = 1;
    while position < args.len() {
        match args[position].as_str() {
            "--demo" => position += 1,
            "--config" => {
                position += 1;
                let value = args.get(position).ok_or("--config requires a file path")?;
                explicit_config = Some(PathBuf::from(value));
                position += 1;
            }
            value if value.starts_with("--config=") => {
                explicit_config = Some(PathBuf::from(&value[9..]));
                position += 1;
            }
            value => return Err(format!("unknown server argument: {value}").into()),
        }
    }
    let default_path = std::env::current_dir()?.join("agent.toml");
    let config_path = explicit_config.or_else(|| default_path.exists().then_some(default_path));
    let mut deployment = match config_path {
        Some(path) => {
            let text = fs::read_to_string(&path)
                .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
            toml::from_str::<DeploymentFile>(&text)
                .map_err(|error| format!("invalid {}: {error}", path.display()))?
        }
        None => DeploymentFile::default(),
    };
    if std::env::var("AGENT_INSTRUCTIONS").as_deref() == Ok("0") {
        deployment.capabilities.insert(
            INSTRUCTIONS.into(),
            CapabilityPolicy {
                enabled: false,
                user_configurable: false,
            },
        );
    }
    if std::env::var("AGENT_MODEL_MANAGEMENT").as_deref() == Ok("0") {
        deployment.capabilities.insert(
            MODEL_MANAGEMENT.into(),
            CapabilityPolicy {
                enabled: false,
                user_configurable: false,
            },
        );
    }
    if std::env::var("AGENT_SKILLS").as_deref() == Ok("0") {
        deployment.capabilities.insert(
            SKILLS.into(),
            CapabilityPolicy {
                enabled: false,
                user_configurable: false,
            },
        );
    }
    if std::env::var("AGENT_COMPACTION").as_deref() == Ok("0") {
        deployment.capabilities.insert(
            COMPACTION.into(),
            CapabilityPolicy {
                enabled: false,
                user_configurable: false,
            },
        );
    }
    if std::env::var("AGENT_SPILL").as_deref() == Ok("0") {
        deployment.capabilities.insert(
            SPILL.into(),
            CapabilityPolicy {
                enabled: false,
                user_configurable: false,
            },
        );
    }
    if std::env::var("AGENT_MCP").as_deref() == Ok("0") {
        deployment.capabilities.insert(MCP.into(), CapabilityPolicy { enabled: false, user_configurable: false });
    }
    if std::env::var("AGENT_SUBAGENT").as_deref() == Ok("0") {
        deployment.capabilities.insert(SUBAGENT.into(), CapabilityPolicy { enabled: false, user_configurable: false });
    }
    if std::env::var("AGENT_SANDBOX").as_deref() == Ok("0") {
        if std::env::var_os("AGENT_SANDBOX_MODE").is_some() { return Err("AGENT_SANDBOX=0 conflicts with AGENT_SANDBOX_MODE".into()); }
        deployment.capabilities.insert(SANDBOX.into(), CapabilityPolicy { enabled: false, user_configurable: false });
    } else if std::env::var_os("AGENT_SANDBOX_MODE").is_some() {
        deployment.capabilities.insert(SANDBOX.into(), CapabilityPolicy { enabled: true, user_configurable: false });
    }
    let state_path = match std::env::var_os("AGENT_CAPABILITY_STATE_PATH") {
        Some(value) => PathBuf::from(value),
        None => state_root()?
            .join("mona-agent-core")
            .join("capabilities.json"),
    };
    CapabilityManager::open(deployment, state_path)
}

fn state_root() -> Result<PathBuf, Box<dyn std::error::Error>> {
    std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_STATE_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state")))
        .ok_or_else(|| "set AGENT_CAPABILITY_STATE_PATH for this host".into())
}

fn read_state(path: &FilePath) -> Result<UserState, Box<dyn std::error::Error>> {
    let text = match fs::read_to_string(path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(UserState::default())
        }
        Err(error) => return Err(error.into()),
    };
    let state: UserState = serde_json::from_str(&text)?;
    if state.version != 0 && state.version != CONFIG_VERSION {
        return Err(format!("unsupported capability state version: {}", state.version).into());
    }
    Ok(state)
}

pub(crate) fn write_state(path: &FilePath, state: &impl Serialize) -> Result<(), Box<dyn std::error::Error>> {
    let parent = path
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or_else(|| FilePath::new("."));
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(&serde_json::to_vec(state)?)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path)?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateCapability {
    revision: u64,
    enabled: bool,
}

struct Auth {
    token: String,
}

pub fn router(
    manager: CapabilityManager,
    token: String,
    origin: Option<String>,
) -> Result<Router, Box<dyn std::error::Error>> {
    if token.len() < 32 || token.len() > 512 || !token.bytes().all(|c| c.is_ascii_graphic()) {
        return Err("capability management requires a 32..512 character bearer token".into());
    }
    let mut cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::PUT])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);
    if let Some(origin) = origin {
        if origin == "*" || !crate::ui_origin_allowed(&origin) {
            return Err("capability management requires an explicit HTTP(S) origin".into());
        }
        cors = cors.allow_origin(origin.parse::<HeaderValue>()?);
    }
    Ok(Router::new()
        .route("/api/capabilities", get(get_capabilities))
        .route("/api/ui", get(ui_modules))
        .route("/api/capabilities/{id}", put(update_capability))
        .with_state(manager)
        .layer(DefaultBodyLimit::max(16 * 1024))
        .layer(middleware::from_fn_with_state(
            Arc::new(Auth { token }),
            authenticate,
        ))
        .layer(cors))
}

async fn ui_modules(State(manager): State<CapabilityManager>) -> Json<serde_json::Value> {
    let mut modules = vec!["capabilities", "workspace", "workbench", "side", "metrics"];
    if manager.active(MODEL_MANAGEMENT) { modules.push("models"); }
    // Management remains available when disabled; operations use actual active flags.
    if cfg!(feature = "memory") || cfg!(feature = "history-search") { modules.push("memory"); }
    if cfg!(feature = "planner") { modules.push("planner"); }
    if cfg!(feature = "sandbox") { modules.push("sandbox"); }
    if cfg!(feature = "subagent") { modules.push("subagent"); }
    if cfg!(feature = "mcp") { modules.push("mcp"); }
    Json(serde_json::json!({"version":1,"modules":modules,"capabilities":manager.assembly()}))
}

async fn authenticate(State(auth): State<Arc<Auth>>, request: Request, next: Next) -> Response {
    let bearer = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if !bearer.is_some_and(|v| bool::from(v.as_bytes().ct_eq(auth.token.as_bytes()))) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"message":"valid bearer authorization is required"})),
        )
            .into_response();
    }
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    response
}

#[derive(Debug)]
struct ApiError(StatusCode, String);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({"message":self.1}))).into_response()
    }
}

async fn get_capabilities(State(manager): State<CapabilityManager>) -> Json<CapabilityView> {
    Json(manager.view())
}

async fn update_capability(
    State(manager): State<CapabilityManager>,
    Path(id): Path<String>,
    input: Result<Json<UpdateCapability>, JsonRejection>,
) -> Result<Json<CapabilityView>, ApiError> {
    let Json(input) = input.map_err(|_| {
        ApiError(
            StatusCode::BAD_REQUEST,
            "invalid capability settings request".into(),
        )
    })?;
    tokio::task::spawn_blocking(move || manager.set_enabled(input.revision, &id, input.enabled))
        .await
        .map_err(|_| {
            ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                "capability settings operation failed".into(),
            )
        })?
        .map(Json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use serde_json::Value;
    use tower::ServiceExt;

    const TOKEN: &str = "test-capability-management-token-32-bytes";

    fn entry<'a>(view: &'a CapabilityView, id: &str) -> &'a CapabilityEntry {
        view.components
            .iter()
            .chain(view.tools.iter())
            .find(|item| item.id == id)
            .unwrap()
    }

    #[test]
    fn user_choice_is_persisted_and_only_becomes_active_after_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("capabilities.json");
        let manager = CapabilityManager::open(DeploymentFile::default(), path.clone()).unwrap();
        let target = if cfg!(feature = "skills") {
            SKILLS
        } else {
            GREP
        };
        assert!(!manager.active(target));
        assert_eq!(manager.active(COMPACTION), cfg!(feature = "compaction"));
        assert_eq!(manager.active(SPILL), cfg!(feature = "spill"));
        if !compiled(SKILLS) {
            assert_eq!(
                manager.set_enabled(0, SKILLS, true).unwrap_err().0,
                StatusCode::BAD_REQUEST
            );
            assert_eq!(manager.view().revision, 0);
        }
        let changed = manager.set_enabled(0, target, true).unwrap();
        let item = entry(&changed, target);
        assert!(item.enabled);
        assert!(item.restart_required);
        let reopened = CapabilityManager::open(DeploymentFile::default(), path.clone()).unwrap();
        assert!(reopened.active(target));
        assert!(!entry(&reopened.view(), target).restart_required);
        let changed = reopened.set_enabled(1, target, false).unwrap();
        assert_eq!(changed.revision, 2);
        let item = entry(&changed, target);
        assert!(!item.enabled);
        assert!(item.restart_required);
        let reopened = CapabilityManager::open(DeploymentFile::default(), path).unwrap();
        assert!(!reopened.active(target));
        assert!(!entry(&reopened.view(), target).restart_required);
    }

    #[test]
    fn project_instructions_can_be_disabled_and_restore_that_choice() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("capabilities.json");
        let manager = CapabilityManager::open(DeploymentFile::default(), path.clone()).unwrap();
        assert!(manager.active(INSTRUCTIONS));
        let changed = manager.set_enabled(0, INSTRUCTIONS, false).unwrap();
        let item = entry(&changed, INSTRUCTIONS);
        assert!(item.restart_required && !item.enabled);
        let reopened = CapabilityManager::open(DeploymentFile::default(), path).unwrap();
        assert!(!reopened.active(INSTRUCTIONS));
    }

    #[test]
    fn deployment_locked_capability_cannot_be_changed() {
        let directory = tempfile::tempdir().unwrap();
        let mut deployment = DeploymentFile::default();
        deployment.capabilities.remove("shell");
        deployment.capabilities.insert(
            "bash".into(),
            CapabilityPolicy {
                enabled: true,
                user_configurable: false,
            },
        );
        let manager =
            CapabilityManager::open(deployment, directory.path().join("capabilities.json"))
                .unwrap();
        assert!(manager.active("shell"));
        assert!(!manager.active("bash"));
        assert_eq!(
            manager.set_enabled(0, "shell", false).unwrap_err().0,
            StatusCode::FORBIDDEN
        );
        let error = manager.set_enabled(0, MODEL_MANAGEMENT, false).unwrap_err();
        assert_eq!(
            error.0,
            if compiled(MODEL_MANAGEMENT) {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::BAD_REQUEST
            }
        );
        assert_eq!(manager.view().revision, 0);
        let fixed = manager.set_enabled(0, "read", false).unwrap_err();
        assert_eq!(fixed.0, StatusCode::FORBIDDEN);
        assert!(!manager.active(GREP));
        let changed = manager.set_enabled(0, GREP, true).unwrap();
        let grep = entry(&changed, GREP);
        assert!(grep.enabled);
        assert!(grep.restart_required);
    }

    #[test]
    fn management_view_separates_components_and_tools_and_hides_fixed_entries() {
        let directory = tempfile::tempdir().unwrap();
        let manager = CapabilityManager::open(
            DeploymentFile::default(),
            directory.path().join("capabilities.json"),
        )
        .unwrap();
        let view = manager.view();
        assert!(view.components.iter().any(|item| item.id == INSTRUCTIONS));
        assert_eq!(
            view.tools.iter().map(|item| item.id).collect::<Vec<_>>(),
            if compiled(MEMORY_UPDATE) { vec![MEMORY_UPDATE, GREP, FIND, LS] } else { vec![GREP, FIND, LS] }
        );
        assert_eq!(manager.active(MEMORY), compiled(MEMORY));
        assert_eq!(manager.active(HISTORY_SEARCH), compiled(HISTORY_SEARCH));
        assert!(!manager.active(MEMORY_UPDATE));
        for hidden in [MODEL_MANAGEMENT, "read", "shell", "edit", "write"] {
            assert!(view
                .components
                .iter()
                .chain(view.tools.iter())
                .all(|item| item.id != hidden));
        }

        if compiled(MODEL_MANAGEMENT) {
            let mut deployment = DeploymentFile::default();
            deployment.capabilities.insert(
                MODEL_MANAGEMENT.into(),
                CapabilityPolicy {
                    enabled: true,
                    user_configurable: true,
                },
            );
            let unlocked = CapabilityManager::open(
                deployment,
                directory.path().join("unlocked-model-management.json"),
            )
            .unwrap();
            assert!(unlocked
                .view()
                .components
                .iter()
                .any(|item| item.id == MODEL_MANAGEMENT));
        }

        let mut deployment = DeploymentFile::default();
        deployment.capabilities.insert(
            INSTRUCTIONS.into(),
            CapabilityPolicy {
                enabled: true,
                user_configurable: false,
            },
        );
        let locked = CapabilityManager::open(
            deployment,
            directory.path().join("locked-capabilities.json"),
        )
        .unwrap();
        assert!(locked
            .view()
            .components
            .iter()
            .all(|item| item.id != INSTRUCTIONS));
    }

    #[tokio::test]
    async fn management_route_requires_auth_and_returns_restart_state() {
        let directory = tempfile::tempdir().unwrap();
        let manager = CapabilityManager::open(
            DeploymentFile::default(),
            directory.path().join("capabilities.json"),
        )
        .unwrap();
        let app = router(manager, TOKEN.into(), None).unwrap();

        let unauthorized = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/capabilities")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/capabilities/grep")
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"revision":0,"enabled":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(value.get("capabilities").is_none());
        let grep = value["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == GREP)
            .unwrap();
        assert_eq!(grep["enabled"], true);
        assert_eq!(grep["restart_required"], true);
        for hidden in [
            "active",
            "available",
            "user_configurable",
            "settings_section",
        ] {
            assert!(grep.get(hidden).is_none());
        }
        assert!(value["components"].is_array());
    }
}
