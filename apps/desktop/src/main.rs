use serde::{Deserialize, Serialize};
#[cfg(windows)]
use std::process::Command;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    net::{SocketAddr, TcpListener},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread::JoinHandle,
    time::Duration,
};
use tauri::{State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;
use tokio::sync::oneshot;

type AppResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct DesktopHost {
    endpoint: String,
    addr: SocketAddr,
    token: String,
    failure: Arc<Mutex<Option<String>>>,
    stop: Mutex<Option<oneshot::Sender<()>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl DesktopHost {
    fn start(
        listener: TcpListener,
        token: String,
        model_store_key: String,
    ) -> AppResult<Arc<Self>> {
        let addr = listener.local_addr()?;
        listener.set_nonblocking(true)?;
        let (stop, stopped) = oneshot::channel();
        let failure = Arc::new(Mutex::new(None));
        let report = failure.clone();
        let service_token = token.clone();
        let thread = std::thread::spawn(move || {
            let result = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|error| error.to_string())
                .and_then(|runtime| {
                    runtime
                        .block_on(server::serve_with_listener(
                            listener,
                            service_token,
                            model_store_key,
                            async {
                                let _ = stopped.await;
                            },
                        ))
                        .map_err(|error| error.to_string())
                });
            if let Err(error) = result {
                *report
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error);
            }
        });
        Ok(Arc::new(Self {
            endpoint: format!("http://{addr}"),
            addr,
            token,
            failure,
            stop: Mutex::new(Some(stop)),
            thread: Mutex::new(Some(thread)),
        }))
    }

    fn shutdown(&self) {
        if let Some(stop) = self
            .stop
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = stop.send(());
        }
        if let Some(thread) = self
            .thread
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = thread.join();
        }
    }
}

#[derive(Serialize)]
struct DesktopConfig {
    endpoint: String,
    token: String,
    open_with: Vec<&'static str>,
}

#[derive(Deserialize)]
struct DesktopWorkspace {
    root: PathBuf,
    available: bool,
    message: Option<String>,
}

fn trusted(window: &WebviewWindow) -> Result<(), String> {
    let url = window.url().map_err(|_| "无法确认桌面窗口来源。")?;
    let local = (url.scheme() == "http" && url.host_str() == Some("tauri.localhost"))
        || (url.scheme() == "tauri" && url.host_str() == Some("localhost"));
    if window.label() == "main" && local {
        Ok(())
    } else {
        Err("此窗口没有桌面操作权限。".into())
    }
}

#[tauri::command]
async fn desktop_config(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopHost>>,
) -> Result<DesktopConfig, String> {
    trusted(&window)?;
    for _ in 0..150 {
        if let Some(error) = state
            .failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            return Err(format!("桌面服务启动失败：{error}"));
        }
        if tokio::net::TcpStream::connect(state.addr).await.is_ok() {
            return Ok(DesktopConfig {
                endpoint: state.endpoint.clone(),
                token: state.token.clone(),
                open_with: if cfg!(windows) {
                    vec!["explorer", "terminal"]
                } else {
                    Vec::new()
                },
            });
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err("桌面服务未在 15 秒内就绪。".into())
}

#[cfg(windows)]
fn shell_path(path: &Path) -> PathBuf {
    let value = path.to_string_lossy();
    if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = value.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path.to_path_buf()
    }
}

#[cfg(windows)]
fn open_system_application(application: &str, root: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    let root = fs::canonicalize(root).map_err(|_| "工作区目录不存在或不可访问。")?;
    if !root.is_dir() {
        return Err("当前工作区不是文件夹。".into());
    }
    let root = shell_path(&root);
    let windows = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && path.is_dir())
        .ok_or("无法定位 Windows 系统目录。")?;
    let mut command = match application {
        "explorer" => {
            let mut command = Command::new(windows.join("explorer.exe"));
            command.arg(&root);
            command
        }
        "terminal" => {
            let mut command = Command::new(windows.join("System32").join("cmd.exe"));
            command
                .arg("/K")
                .current_dir(&root)
                .creation_flags(CREATE_NEW_CONSOLE);
            command
        }
        _ => return Err("不支持的桌面打开方式。".into()),
    };
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("无法打开系统应用：{error}"))
}

#[cfg(not(windows))]
fn open_system_application(_application: &str, _root: &Path) -> Result<(), String> {
    Err("当前桌面宿主没有系统打开方式。".into())
}

#[tauri::command]
async fn desktop_open_workspace(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopHost>>,
    session_id: String,
    application: String,
) -> Result<(), String> {
    trusted(&window)?;
    if session_id.is_empty()
        || session_id.len() > 64
        || !session_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("会话 ID 无效。".into());
    }
    if !["explorer", "terminal"].contains(&application.as_str()) {
        return Err("不支持的桌面打开方式。".into());
    }
    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/sessions/{session_id}/workspace",
            state.endpoint
        ))
        .bearer_auth(&state.token)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|_| "无法读取当前工作区。")?;
    if !response.status().is_success() {
        return Err("当前会话的工作区不可用。".into());
    }
    let workspace: DesktopWorkspace = response.json().await.map_err(|_| "工作区响应无效。")?;
    if !workspace.available {
        return Err(workspace
            .message
            .unwrap_or_else(|| "当前工作区不可用。".into()));
    }
    tokio::task::spawn_blocking(move || open_system_application(&application, &workspace.root))
        .await
        .map_err(|_| "启动系统应用失败。")?
}

#[tauri::command]
async fn desktop_pick_project(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopHost>>,
    request_id: String,
    revision: u64,
) -> Result<Option<serde_json::Value>, String> {
    trusted(&window)?;
    let chosen = tokio::task::spawn_blocking(move || window.dialog().file().blocking_pick_folder())
        .await
        .map_err(|_| "文件夹选择器未返回结果。")?;
    let Some(chosen) = chosen else {
        return Ok(None);
    };
    let path = chosen.into_path().map_err(|_| "请选择本机文件夹。")?;
    let name = path
        .file_name()
        .and_then(|part| part.to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| path.display().to_string());
    let response = reqwest::Client::new()
        .post(format!("{}/api/projects", state.endpoint))
        .bearer_auth(&state.token)
        .json(&serde_json::json!({ "request_id": request_id, "revision": revision, "name": name, "path": path }))
        .timeout(Duration::from_secs(20))
        .send().await.map_err(|_| "项目登记请求失败，请刷新项目列表确认。")?;
    if !response.status().is_success() {
        let body: serde_json::Value = response.json().await.unwrap_or_default();
        return Err(body
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("项目登记失败，请刷新项目列表确认。")
            .to_owned());
    }
    response
        .json()
        .await
        .map(Some)
        .map_err(|_| "项目登记响应无效，请刷新项目列表确认。".into())
}

fn desktop_state_dir() -> AppResult<PathBuf> {
    if let Some(path) = std::env::var_os("MONA_DESKTOP_STATE_DIR").filter(|value| !value.is_empty())
    {
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err("MONA_DESKTOP_STATE_DIR 必须是绝对路径。".into());
        }
        return Ok(path);
    }
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA").ok_or("缺少 LOCALAPPDATA。")?;
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME")
        .map(|value| PathBuf::from(value).join("Library/Application Support"))
        .ok_or("缺少 HOME。")?
        .into_os_string();
    #[cfg(all(unix, not(target_os = "macos")))]
    let base = std::env::var_os("XDG_STATE_HOME")
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|value| PathBuf::from(value).join(".local/state").into_os_string())
        })
        .ok_or("缺少 HOME。")?;
    Ok(PathBuf::from(base).join("Mona"))
}

fn model_store_key(root: &Path) -> AppResult<String> {
    let path = root.join("model-store.key");
    let read = || -> AppResult<String> {
        let key = fs::read_to_string(&path)?.trim().to_owned();
        if key.len() < 32 || !key.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err("模型设置密钥文件无效。".into());
        }
        Ok(key)
    };
    match read() {
        Ok(key) => Ok(key),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound) =>
        {
            let key = format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            );
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    file.write_all(key.as_bytes())?;
                    file.sync_all()?;
                    Ok(key)
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => read(),
                Err(error) => Err(error.into()),
            }
        }
        Err(error) => Err(error),
    }
}

fn configure_server(root: &Path, addr: SocketAddr) -> AppResult<String> {
    fs::create_dir_all(root)?;
    let key = model_store_key(root)?;
    std::env::set_var("MONA_STATE_DIR", root);
    std::env::set_var("AGENT_SERVER_ADDR", addr.to_string());
    std::env::set_var("AGENT_MODEL_SETTINGS_PATH", root.join("model-settings.enc"));
    std::env::set_var("AGENT_SESSIONS_DIR", root.join("sessions"));
    std::env::set_var("AGENT_PROJECTS_PATH", root.join("projects.json"));
    std::env::set_var(
        "AGENT_WORKSPACE_SETTINGS_PATH",
        root.join("workspace-settings.json"),
    );
    std::env::set_var(
        "AGENT_CAPABILITY_STATE_PATH",
        root.join("capabilities.json"),
    );
    std::env::set_var("AGENT_SPILL_DIR", root.join("spill"));
    std::env::set_var("AGENT_MEMORY_DIR", root.join("memory"));
    #[cfg(windows)]
    std::env::set_var("AGENT_UI_ORIGIN", "http://tauri.localhost");
    #[cfg(not(windows))]
    std::env::set_var("AGENT_UI_ORIGIN", "tauri://localhost");
    Ok(key)
}

fn run() -> AppResult<()> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let model_store_key = configure_server(&desktop_state_dir()?, listener.local_addr()?)?;
    let host = DesktopHost::start(listener, token, model_store_key)?;
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(host.clone())
        .invoke_handler(tauri::generate_handler![
            desktop_config,
            desktop_pick_project,
            desktop_open_workspace
        ])
        .run(tauri::generate_context!());
    host.shutdown();
    result?;
    Ok(())
}

fn main() {
    if let Some(code) = server::dispatch_helper() {
        std::process::exit(code);
    }
    if let Err(error) = run() {
        eprintln!("Mona Desktop 启动失败：{error}");
        std::process::exit(1);
    }
}
