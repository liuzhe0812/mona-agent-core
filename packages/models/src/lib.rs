//! Optional model configuration and routing. No Core, Application or transport dependency.
//! The host retains the management handle; only the Model implementation enters the registry.
#![forbid(unsafe_code)]

mod storage;
pub use storage::{EncryptedFileStore, SettingsStore};

use api::*;
use providers::{ChatConfig, ChatModel};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|e| e.into_inner())
}
fn invalid(message: &str) -> AgentError {
    AgentError::new(ErrorCode::Configuration, message)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelEntry {
    pub id: String,
    pub enabled: bool,
    /// Trusted per-provider/model capacity. None means unknown, never an inferred global default.
    #[serde(default)]
    pub context_window_tokens: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub provider_id: String,
    pub model_id: String,
}

/// Safe for the settings UI; neither the credential nor a credential prefix is returned.
#[derive(Clone, Debug, Serialize)]
pub struct ProviderView {
    pub id: String,
    pub name: String,
    pub api_base: String,
    pub has_key: bool,
    pub builtin: bool,
    pub models: Vec<ModelEntry>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SettingsView {
    pub revision: u64,
    pub providers: Vec<ProviderView>,
    pub default: Option<Selection>,
}

/// Write-only credential. Omitting it retains the stored value. Clearing is explicit.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpsertProvider {
    pub revision: u64,
    pub id: String,
    pub name: String,
    pub api_base: String,
    pub api_key: Option<String>,
    #[serde(default)]
    pub clear_key: bool,
    pub models: Vec<ModelEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoverRequest {
    pub provider_id: Option<String>,
    pub api_base: String,
    pub api_key: Option<String>,
    #[serde(default)]
    pub clear_key: bool,
}

/// Not Debug: the encrypted document owns secrets.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Provider {
    id: String,
    name: String,
    api_base: String,
    api_key: Option<String>,
    models: Vec<ModelEntry>,
    #[serde(default)]
    extra_body: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u32,
    revision: u64,
    providers: Vec<Provider>,
    default: Option<Selection>,
}

struct BoundModel {
    adapter: Arc<dyn Model>,
    model: String,
}
#[derive(Default)]
struct Router {
    bindings: Mutex<BTreeMap<String, Arc<BoundModel>>>,
}

#[async_trait]
impl Model for Router {
    fn context_window_tokens(&self, options: &ModelOptions) -> Option<u64> {
        let bound = options
            .model
            .as_ref()
            .and_then(|key| lock(&self.bindings).get(key).cloned())?;
        let mut effective = options.clone();
        effective.model = Some(bound.model.clone());
        bound.adapter.context_window_tokens(&effective)
    }

    async fn stream(
        &self,
        mut request: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelStream> {
        let bound = {
            let bindings = lock(&self.bindings);
            request
                .options
                .model
                .as_ref()
                .and_then(|key| bindings.get(key))
                .cloned()
        }
        .ok_or_else(|| invalid("model management requires the host's ManagedRuntime"))?;
        request.options.model = Some(bound.model.clone());
        bound.adapter.stream(request, cancel).await
    }
}

struct Inner {
    document: Mutex<Document>,
    store: Arc<dyn SettingsStore>,
    router: Arc<Router>,
    sequence: std::sync::atomic::AtomicU64,
    allow_http_loopback: bool,
}

/// Keep this handle in the trusted host. It is deliberately not a plugin service or agent Tool.
#[derive(Clone)]
pub struct ModelManager {
    inner: Arc<Inner>,
}

impl ModelManager {
    /// Load existing settings; malformed or undecryptable data fails closed.
    pub fn open(store: Arc<dyn SettingsStore>, allow_http_loopback: bool) -> Result<Self> {
        let document = match store.load()? {
            Some(bytes) => serde_json::from_slice::<Document>(&bytes)
                .map_err(|_| invalid("invalid model settings document"))?,
            None => Document {
                version: 1,
                revision: 0,
                providers: vec![],
                default: None,
            },
        };
        let manager = Self {
            inner: Arc::new(Inner {
                document: Mutex::new(document),
                store,
                router: Arc::new(Router::default()),
                sequence: std::sync::atomic::AtomicU64::new(1),
                allow_http_loopback,
            }),
        };
        manager.validate(&lock(&manager.inner.document))?;
        Ok(manager)
    }

    pub fn view(&self) -> SettingsView {
        view(&lock(&self.inner.document))
    }

    /// Import a trusted initial configuration only for a never-written store.
    pub fn seed(
        &self,
        input: UpsertProvider,
        extra_body: serde_json::Map<String, serde_json::Value>,
    ) -> Result<()> {
        let mut document = lock(&self.inner.document);
        if document.revision != 0 {
            return Ok(());
        }
        let provider = Provider {
            id: input.id,
            name: input.name,
            api_base: normalize_base(&input.api_base, self.inner.allow_http_loopback)?,
            api_key: input.api_key,
            models: input.models,
            extra_body,
        };
        let default = provider
            .models
            .iter()
            .find(|m| m.enabled)
            .map(|m| Selection {
                provider_id: provider.id.clone(),
                model_id: m.id.clone(),
            });
        let next = Document {
            version: 1,
            revision: 1,
            providers: vec![provider],
            default,
        };
        self.validate(&next)?;
        self.persist(&next)?;
        *document = next;
        Ok(())
    }

    fn validate(&self, doc: &Document) -> Result<()> {
        if doc.version != 1 || doc.providers.len() > 32 {
            return Err(invalid(
                "unsupported settings version or provider limit exceeded",
            ));
        }
        let mut ids = BTreeSet::new();
        for p in &doc.providers {
            if p.id.is_empty()
                || p.id.len() > 80
                || !p
                    .id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
                || !ids.insert(&p.id)
                || p.name.trim().is_empty()
                || p.name.len() > 160
            {
                return Err(invalid("invalid or duplicate provider identifier/name"));
            }
            normalize_base(&p.api_base, self.inner.allow_http_loopback)?;
            validate_key(p.api_key.as_deref())?;
            let mut models = BTreeSet::new();
            if p.models.is_empty() || p.models.len() > 256 {
                return Err(invalid("a provider requires 1..256 models"));
            }
            for m in &p.models {
                if m.context_window_tokens.is_some_and(|window| window == 0 || window > 1_000_000_000) {
                    return Err(invalid("model context window must be 1..1000000000 tokens or null"));
                }
                if m.id.trim().is_empty()
                    || m.id.len() > 512
                    || m.id.chars().any(char::is_control)
                    || !models.insert(&m.id)
                {
                    return Err(invalid("invalid or duplicate model identifier"));
                }
            }
            // Validate adapter options before a durable configuration is accepted.
            self.adapter(p, &p.models[0].id)?;
        }
        if let Some(selected) = &doc.default {
            if !doc.providers.iter().any(|p| {
                p.id == selected.provider_id
                    && p.models
                        .iter()
                        .any(|m| m.id == selected.model_id && m.enabled)
            }) {
                return Err(invalid(
                    "select another default before removing or hiding the default model",
                ));
            }
        }
        Ok(())
    }

    fn adapter(&self, provider: &Provider, model: &str) -> Result<Arc<dyn Model>> {
        let mut config = ChatConfig::new(format!("{}/chat/completions", provider.api_base), model);
        config.api_key = provider.api_key.clone();
        config.allow_http_loopback = self.inner.allow_http_loopback;
        config.extra_body = provider.extra_body.clone();
        config.context_window_tokens = provider.models.iter().find(|entry| entry.id == model)
            .and_then(|entry| entry.context_window_tokens);
        // Signed/private replay data stays confined to this provider.
        config.protocol_namespace = format!("managed.{}", provider.id);
        Ok(Arc::new(ChatModel::new(config)?))
    }

    fn persist(&self, doc: &Document) -> Result<()> {
        let bytes = serde_json::to_vec(doc).map_err(|_| invalid("cannot encode model settings"))?;
        self.inner.store.save(&bytes)
    }

    fn update(
        &self,
        revision: u64,
        change: impl FnOnce(&mut Document) -> Result<()>,
    ) -> Result<SettingsView> {
        let mut current = lock(&self.inner.document);
        if current.revision != revision {
            return Err(invalid("settings_conflict: refresh settings before saving"));
        }
        let mut next = current.clone();
        change(&mut next)?;
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("settings revision exhausted"))?;
        self.validate(&next)?;
        // Do not publish an in-memory change if persistence fails.
        self.persist(&next)?;
        *current = next;
        Ok(view(&current))
    }

    pub fn upsert(&self, input: UpsertProvider) -> Result<SettingsView> {
        let base = normalize_base(&input.api_base, self.inner.allow_http_loopback)?;
        self.update(input.revision, |doc| {
            let previous = doc.providers.iter().find(|p| p.id == input.id);
            let key = merge_key(
                input.api_key,
                input.clear_key,
                previous.and_then(|p| p.api_key.clone()),
            )?;
            let provider = Provider {
                id: input.id,
                name: input.name.trim().into(),
                api_base: base,
                api_key: key,
                models: input.models,
                extra_body: previous.map(|p| p.extra_body.clone()).unwrap_or_default(),
            };
            if doc.default.is_none() {
                doc.default = provider
                    .models
                    .iter()
                    .find(|m| m.enabled)
                    .map(|m| Selection {
                        provider_id: provider.id.clone(),
                        model_id: m.id.clone(),
                    });
            }
            if let Some(index) = doc.providers.iter().position(|p| p.id == provider.id) {
                doc.providers[index] = provider;
            } else {
                doc.providers.push(provider);
            }
            Ok(())
        })
    }

    pub fn delete(&self, revision: u64, provider_id: &str) -> Result<SettingsView> {
        self.update(revision, |doc| {
            if !doc.providers.iter().any(|p| p.id == provider_id) {
                return Err(invalid("provider not found"));
            }
            doc.providers.retain(|p| p.id != provider_id);
            if doc
                .default
                .as_ref()
                .is_some_and(|s| s.provider_id == provider_id)
            {
                doc.default = doc.providers.iter().find_map(|provider| {
                    provider
                        .models
                        .iter()
                        .find(|model| model.enabled)
                        .map(|model| Selection {
                            provider_id: provider.id.clone(),
                            model_id: model.id.clone(),
                        })
                });
            }
            Ok(())
        })
    }

    pub fn set_default(&self, revision: u64, selection: Selection) -> Result<SettingsView> {
        self.update(revision, |doc| {
            let model = doc
                .providers
                .iter_mut()
                .find(|p| p.id == selection.provider_id)
                .and_then(|p| p.models.iter_mut().find(|m| m.id == selection.model_id))
                .ok_or_else(|| invalid("model not found"))?;
            model.enabled = true;
            doc.default = Some(selection);
            Ok(())
        })
    }

    pub fn set_visibility(
        &self,
        revision: u64,
        provider_id: &str,
        model_id: Option<&str>,
        enabled: bool,
    ) -> Result<SettingsView> {
        self.update(revision, |doc| {
            let provider = doc
                .providers
                .iter_mut()
                .find(|p| p.id == provider_id)
                .ok_or_else(|| invalid("provider not found"))?;
            if model_id.is_some_and(|id| !provider.models.iter().any(|m| m.id == id)) {
                return Err(invalid("model not found"));
            }
            for m in &mut provider.models {
                if model_id.is_none_or(|id| id == m.id) {
                    let is_default = doc
                        .default
                        .as_ref()
                        .is_some_and(|s| s.provider_id == provider_id && s.model_id == m.id);
                    m.enabled = enabled || is_default;
                }
            }
            Ok(())
        })
    }

    /// Discovery is read-only, bounded, authenticated and never follows redirects.
    pub async fn discover(&self, input: DiscoverRequest) -> Result<Vec<String>> {
        let base = normalize_base(&input.api_base, self.inner.allow_http_loopback)?;
        let previous = {
            let doc = lock(&self.inner.document);
            match input.provider_id.as_ref() {
                Some(id) => {
                    let p = doc
                        .providers
                        .iter()
                        .find(|p| &p.id == id)
                        .ok_or_else(|| invalid("provider not found"))?;
                    // A saved key must never be forwarded to a newly typed endpoint without re-entry.
                    if p.api_base != base
                        && input.api_key.as_ref().is_none_or(|s| s.trim().is_empty())
                        && !input.clear_key
                    {
                        return Err(invalid(
                            "enter the API key again when discovering from a different endpoint",
                        ));
                    }
                    p.api_key.clone()
                }
                None => None,
            }
        };
        let key = merge_key(input.api_key, input.clear_key, previous)?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|_| invalid("cannot initialize discovery client"))?;
        let mut request = client.get(format!("{base}/models"));
        if let Some(key) = key {
            request = request.bearer_auth(key);
        }
        let mut response = request
            .send()
            .await
            .map_err(|_| invalid("model discovery connection failed"))?;
        if !response.status().is_success() {
            return Err(invalid("model discovery rejected; check endpoint and credentials, or enter model IDs manually"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| invalid("model discovery response failed"))?
        {
            if bytes.len() + chunk.len() > 1024 * 1024 {
                return Err(invalid("model discovery response is too large"));
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|_| invalid("invalid model discovery response"))?;
        let data = value
            .get("data")
            .and_then(|v| v.as_array())
            .ok_or_else(|| invalid("model discovery requires an OpenAI-compatible data array"))?;
        let mut models = BTreeSet::new();
        for item in data {
            if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                if !id.trim().is_empty() && id.len() <= 512 && !id.chars().any(char::is_control) {
                    models.insert(id.to_owned());
                }
            }
            if models.len() >= 256 {
                break;
            }
        }
        Ok(models.into_iter().collect())
    }

    pub fn plugin(&self) -> Arc<ModelManagerPlugin> {
        Arc::new(ModelManagerPlugin {
            router: self.inner.router.clone(),
        })
    }

    pub fn runtime(&self, runtime: Arc<dyn AgentRuntime>) -> Arc<ManagedRuntime> {
        Arc::new(ManagedRuntime {
            runtime,
            manager: self.clone(),
        })
    }

    fn bind(&self) -> Result<Binding> {
        let doc = lock(&self.inner.document);
        let selection = doc
            .default
            .as_ref()
            .ok_or_else(|| invalid("configure a default model before starting a task"))?;
        let provider = doc
            .providers
            .iter()
            .find(|p| p.id == selection.provider_id)
            .ok_or_else(|| invalid("default provider missing"))?;
        let bound = Arc::new(BoundModel {
            adapter: self.adapter(provider, &selection.model_id)?,
            model: selection.model_id.clone(),
        });
        let number = self
            .inner
            .sequence
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let id = format!("managed:{}:{}:{number}", provider.id, doc.revision);
        lock(&self.inner.router.bindings).insert(id.clone(), bound);
        Ok(Binding {
            id,
            router: self.inner.router.clone(),
        })
    }
}

fn view(doc: &Document) -> SettingsView {
    SettingsView {
        revision: doc.revision,
        default: doc.default.clone(),
        providers: doc
            .providers
            .iter()
            .map(|p| ProviderView {
                id: p.id.clone(),
                name: p.name.clone(),
                api_base: p.api_base.clone(),
                has_key: p.api_key.is_some(),
                builtin: false,
                models: p.models.clone(),
            })
            .collect(),
    }
}

fn validate_key(key: Option<&str>) -> Result<()> {
    if key
        .is_some_and(|k| k.is_empty() || k.len() > 8192 || !k.bytes().all(|b| b.is_ascii_graphic()))
    {
        return Err(invalid(
            "API key must contain only visible ASCII characters",
        ));
    }
    Ok(())
}

fn merge_key(
    input: Option<String>,
    clear: bool,
    previous: Option<String>,
) -> Result<Option<String>> {
    let supplied = input.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    if clear && supplied.is_some() {
        return Err(invalid("cannot clear and replace an API key together"));
    }
    let key = if clear { None } else { supplied.or(previous) };
    validate_key(key.as_deref())?;
    Ok(key)
}

fn normalize_base(raw: &str, allow_http_loopback: bool) -> Result<String> {
    if raw.len() > 2048 {
        return Err(invalid("API base is too long"));
    }
    let mut url = Url::parse(raw.trim()).map_err(|_| invalid("invalid API base URL"))?;
    let local = url
        .host_str()
        .is_some_and(|s| matches!(s, "localhost" | "127.0.0.1" | "::1" | "[::1]"));
    if url.host_str().is_none()
        || (url.scheme() != "https" && !(url.scheme() == "http" && local && allow_http_loopback))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid("use HTTPS API base without credentials/query/fragment; loopback HTTP requires host opt-in"));
    }
    let path = url.path().trim_end_matches('/').to_owned();
    url.set_path(path.strip_suffix("/chat/completions").unwrap_or(&path));
    Ok(url.to_string().trim_end_matches('/').to_owned())
}

pub struct ModelManagerPlugin {
    router: Arc<Router>,
}
#[async_trait]
impl Plugin for ModelManagerPlugin {
    fn manifest(&self) -> PluginManifest {
        let mut manifest = PluginManifest::new("models");
        manifest.provides.push(MODEL_SERVICE.into());
        manifest
    }
    async fn install(&self, registrar: &mut dyn Registrar) -> Result<()> {
        registrar.publish(ServiceRegistration::new(
            MODEL_SERVICE,
            Arc::new(ModelProvider(self.router.clone())),
        ))
    }
}

struct Binding {
    id: String,
    router: Arc<Router>,
}
impl Drop for Binding {
    fn drop(&mut self) {
        lock(&self.router.bindings).remove(&self.id);
    }
}

/// Host-side decorator: selects once at admission, retaining the binding through completion.
/// It delegates the entire execution to the existing runtime, including its budgets and tools.
pub struct ManagedRuntime {
    runtime: Arc<dyn AgentRuntime>,
    manager: ModelManager,
}
impl AgentRuntime for ManagedRuntime {
    fn start(&self, mut request: RunRequest) -> Result<RunHandle> {
        if request.model_options.model.is_some() {
            return Err(invalid(
                "ManagedRuntime selects the configured default; per-run override is not enabled",
            ));
        }
        let executor = tokio::runtime::Handle::try_current()
            .map_err(|_| invalid("a Tokio runtime is required"))?;
        let binding = self.manager.bind()?;
        request.model_options.model = Some(binding.id.clone());
        let handle = self.runtime.start(request)?;
        let session = handle.session();
        executor.spawn(async move {
            let _ = session.wait().await;
            drop(binding);
        });
        Ok(handle)
    }
}
#[async_trait]
impl AgentExecutor for ManagedRuntime {
    async fn execute(&self, request: RunRequest) -> Result<Arc<RunReport>> {
        self.start(request)?.wait_owned().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[derive(Default)]
    struct Store {
        bytes: Mutex<Option<Vec<u8>>>,
        fail: AtomicBool,
    }
    impl SettingsStore for Store {
        fn load(&self) -> Result<Option<Vec<u8>>> {
            Ok(lock(&self.bytes).clone())
        }
        fn save(&self, bytes: &[u8]) -> Result<()> {
            if self.fail.load(Ordering::Relaxed) {
                return Err(invalid("storage failed"));
            }
            *lock(&self.bytes) = Some(bytes.to_vec());
            Ok(())
        }
    }
    fn input(revision: u64, id: &str) -> UpsertProvider {
        UpsertProvider {
            revision,
            id: id.into(),
            name: id.into(),
            api_base: "https://example.com/v1".into(),
            api_key: Some("test-key-not-a-real-secret".into()),
            clear_key: false,
            models: vec![
                ModelEntry {
                    id: "first".into(),
                    enabled: true,
                    context_window_tokens: None,
                },
                ModelEntry {
                    id: "second".into(),
                    enabled: true,
                    context_window_tokens: None,
                },
            ],
        }
    }

    #[test]
    fn model_windows_are_persisted_route_bound_and_frozen_for_active_runs() {
        let store = Arc::new(Store::default());
        let manager = ModelManager::open(store.clone(), false).unwrap();
        let legacy: ModelEntry = serde_json::from_str(r#"{"id":"legacy","enabled":true}"#).unwrap();
        assert_eq!(legacy.context_window_tokens, None);
        let mut first = input(0, "one"); first.models[0].context_window_tokens = Some(8192);
        first.models[1].context_window_tokens = Some(32768);
        manager.upsert(first).unwrap();
        let old = manager.bind().unwrap();
        let options = ModelOptions { model: Some(old.id.clone()), ..Default::default() };
        assert_eq!(manager.inner.router.context_window_tokens(&options), Some(8192));
        let mut edit = input(1, "one"); edit.models[0].context_window_tokens = Some(16384);
        manager.upsert(edit).unwrap();
        let new = manager.bind().unwrap();
        assert_eq!(manager.inner.router.context_window_tokens(&options), Some(8192));
        assert_eq!(manager.inner.router.context_window_tokens(&ModelOptions { model: Some(new.id.clone()), ..Default::default() }), Some(16384));
        assert_eq!(ModelManager::open(store.clone(), false).unwrap().view().providers[0].models[0].context_window_tokens, Some(16384));
        for window in [0, 1_000_000_001] {
            let mut edit = input(2, "one"); edit.models[0].context_window_tokens = Some(window);
            assert!(manager.upsert(edit).is_err()); assert_eq!(manager.view().revision, 2);
        }
        store.fail.store(true, Ordering::Relaxed);
        let mut edit = input(2, "one"); edit.models[0].context_window_tokens = Some(9999);
        assert!(manager.upsert(edit).is_err());
        assert_eq!(manager.view().providers[0].models[0].context_window_tokens, Some(16384));
    }

    #[test]
    fn persistence_failures_do_not_publish_and_revisions_prevent_lost_updates() {
        let store = Arc::new(Store::default());
        let manager = ModelManager::open(store.clone(), false).unwrap();
        manager.upsert(input(0, "one")).unwrap();
        assert!(manager
            .upsert(input(0, "two"))
            .unwrap_err()
            .message
            .starts_with("settings_conflict:"));
        store.fail.store(true, Ordering::Relaxed);
        assert!(manager
            .set_default(
                1,
                Selection {
                    provider_id: "one".into(),
                    model_id: "second".into()
                }
            )
            .is_err());
        assert_eq!(manager.view().revision, 1);
        assert_eq!(manager.view().default.unwrap().model_id, "first");
        let reopened = ModelManager::open(store, false).unwrap();
        assert_eq!(reopened.view().default.unwrap().model_id, "first");
    }

    #[test]
    fn credentials_are_write_only_and_blank_updates_preserve_them() {
        let manager = ModelManager::open(Arc::new(Store::default()), false).unwrap();
        manager.upsert(input(0, "one")).unwrap();
        let json = serde_json::to_string(&manager.view()).unwrap();
        assert!(!json.contains("test-key"));
        assert!(!json.contains("api_key"));
        let mut edit = input(1, "one");
        edit.api_key = Some(String::new());
        manager.upsert(edit).unwrap();
        assert_eq!(
            lock(&manager.inner.document).providers[0]
                .api_key
                .as_deref(),
            Some("test-key-not-a-real-secret")
        );
        let mut edit = input(2, "one");
        edit.api_key = None;
        edit.clear_key = true;
        manager.upsert(edit).unwrap();
        assert!(!manager.view().providers[0].has_key);
    }

    #[test]
    fn defaults_stay_enabled_and_deleting_the_default_clears_it_when_no_fallback_exists() {
        let manager = ModelManager::open(Arc::new(Store::default()), false).unwrap();
        manager.upsert(input(0, "one")).unwrap();
        let view = manager.set_visibility(1, "one", None, false).unwrap();
        assert!(view.providers[0].models[0].enabled);
        assert!(!view.providers[0].models[1].enabled);
        let view = manager
            .set_default(
                2,
                Selection {
                    provider_id: "one".into(),
                    model_id: "second".into(),
                },
            )
            .unwrap();
        assert!(view.providers[0].models[1].enabled);
        let mut edit = input(3, "one");
        edit.models.retain(|m| m.id != "second");
        assert!(manager.upsert(edit).is_err());
        assert_eq!(manager.view().revision, 3);
        let view = manager.delete(3, "one").unwrap();
        assert!(view.providers.is_empty() && view.default.is_none());
    }

    #[test]
    fn endpoints_and_model_catalogs_are_validated_before_saving() {
        let manager = ModelManager::open(Arc::new(Store::default()), false).unwrap();
        for base in [
            "http://127.0.0.1/v1",
            "https://user:pass@example.com/v1",
            "https://example.com/v1?key=secret",
            "file:///tmp/test",
        ] {
            let mut edit = input(0, "one");
            edit.api_base = base.into();
            assert!(manager.upsert(edit).is_err());
        }
        let mut edit = input(0, "one");
        edit.models.push(edit.models[0].clone());
        assert!(manager.upsert(edit).is_err());
        let mut edit = input(0, "one");
        edit.api_base = "https://example.com/v1/chat/completions/".into();
        assert_eq!(
            manager.upsert(edit).unwrap().providers[0].api_base,
            "https://example.com/v1"
        );
    }

    #[test]
    fn bound_models_outlive_configuration_changes_and_release_routes() {
        let manager = ModelManager::open(Arc::new(Store::default()), false).unwrap();
        manager.upsert(input(0, "one")).unwrap();
        let old = manager.bind().unwrap();
        manager
            .set_default(
                1,
                Selection {
                    provider_id: "one".into(),
                    model_id: "second".into(),
                },
            )
            .unwrap();
        let new = manager.bind().unwrap();
        assert_eq!(lock(&manager.inner.router.bindings)[&old.id].model, "first");
        assert_eq!(
            lock(&manager.inner.router.bindings)[&new.id].model,
            "second"
        );
        drop(old);
        drop(new);
        assert!(lock(&manager.inner.router.bindings).is_empty());
    }

    #[tokio::test]
    async fn plugin_exposes_only_model_and_requires_the_host_runtime_decorator() {
        let manager = ModelManager::open(Arc::new(Store::default()), false).unwrap();
        let mut host = runtime::HostBuilder::new()
            .plugin(manager.plugin())
            .build()
            .await
            .unwrap();
        let services = host.services().unwrap();
        assert!(!services.contains(MODEL_SERVICE));
        assert_eq!(services.keys().count(), 1); // only the API version; no management service
        let report = manager
            .runtime(Arc::new(host.engine()))
            .execute(RunRequest::new("hello"))
            .await;
        assert!(report.is_err()); // no default yet, no network or execution started
        assert!(lock(&manager.inner.router.bindings).is_empty());
        host.shutdown().await.unwrap();
    }
}
