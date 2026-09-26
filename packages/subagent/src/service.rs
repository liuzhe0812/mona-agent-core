use crate::{
    disk, execution::AgentExecutionLimiter, invalid, lock, state::*, storage, Binding, Config,
    Role, TOOL_NAMES,
};
use api::*;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::{watch, Mutex as AsyncMutex};

/// A host binds its existing runtime and trusted environment. No endpoints or credentials
/// are accepted from model arguments. The runtime must use this Service's Sessions Store.
pub trait Driver: Send + Sync {
    fn prepare(&self, parent: &RunContext, role: &Role) -> Result<Launch>;
}
pub struct Launch {
    pub runtime: Arc<dyn AgentRuntime>,
    pub model_options: ModelOptions,
    pub model: Option<crate::ModelRoute>,
    /// Only trusted environment/policy fields. Service replaces session and turn identities.
    pub metadata: BTreeMap<String, String>,
}
#[derive(Clone)]
pub(crate) struct Parent {
    pub ctx: RunContext,
    pub history: Arc<Vec<Message>>,
    pub driver: Arc<dyn Driver>,
    pub root: String,
    pub actor: String,
    pub depth: u32,
    pub root_run: String,
    pub stop: CancellationToken,
    pub limiter: Arc<AgentExecutionLimiter>,
}
impl Parent {
    fn check(&self) -> Result<()> {
        self.ctx.task.check()?;
        if self.stop.is_cancelled() {
            return Err(AgentError::new(
                ErrorCode::Cancelled,
                "parent run has ended",
            ));
        }
        Ok(())
    }
    fn allows(&self, profile: &Profile) -> bool {
        profile.root == self.root && (self.actor == self.root || profile.owner == self.actor)
    }
}
struct Active {
    parent_run: String,
    root: String,
    session: Arc<dyn RunSession>,
    task: TaskControl,
    done: watch::Receiver<Option<Result<()>>>,
}
pub struct Service {
    pub(crate) store: Arc<sessions::Store>,
    pub(crate) config: Config,
    pub(crate) parents: Mutex<BTreeMap<String, Parent>>,
    active: Mutex<BTreeMap<String, Arc<Active>>>,
    completion_errors: Mutex<BTreeMap<String, AgentError>>,
    admission: AsyncMutex<()>,
    changed: watch::Sender<u64>,
    closed: AtomicBool,
}
impl Service {
    pub fn new(store: Arc<sessions::Store>, config: Config) -> Result<Arc<Self>> {
        config.validate()?;
        Ok(Arc::new(Self {
            store,
            config,
            parents: Mutex::new(BTreeMap::new()),
            active: Mutex::new(BTreeMap::new()),
            completion_errors: Mutex::new(BTreeMap::new()),
            admission: AsyncMutex::new(()),
            changed: watch::channel(0).0,
            closed: AtomicBool::new(false),
        }))
    }
    pub fn config(&self) -> &Config {
        &self.config
    }
    pub fn binding(self: &Arc<Self>, driver: Arc<dyn Driver>) -> Binding {
        Binding::new(self.clone(), driver)
    }
    fn changed(&self) {
        self.changed.send_modify(|n| *n = n.wrapping_add(1));
    }
    pub(crate) fn parent(&self, run: &str) -> Result<Parent> {
        if self.closed.load(Ordering::Acquire) {
            return Err(AgentError::new(
                ErrorCode::Closed,
                "subagents are shutting down",
            ));
        }
        let p = lock(&self.parents)?
            .get(run)
            .cloned()
            .ok_or_else(|| invalid("parent context is unavailable"))?;
        p.check()?;
        Ok(p)
    }
    pub fn active_parent(&self, root: &str) -> Option<String> {
        lock(&self.parents)
            .ok()?
            .iter()
            .find(|(_, p)| p.actor == root && p.check().is_ok())
            .map(|(id, _)| id.clone())
    }
    pub(crate) async fn capture(
        &self,
        ctx: &RunContext,
        history: Vec<Message>,
        driver: Arc<dyn Driver>,
    ) -> Result<()> {
        let Some(actor) = ctx.metadata.get(sessions::SESSION_KEY).cloned() else {
            return Ok(());
        };
        let store = self.store.clone();
        let id = actor.clone();
        let doc = disk(move || store.get(&id).map_err(storage)).await?;
        let (root, depth) = if doc.header.metadata.contains_key(ROOT_KEY) {
            let p = Profile::read(&doc)?;
            (p.root, p.depth)
        } else {
            (actor.clone(), 0)
        };
        let root_run = ctx
            .metadata
            .get("subagent.root_run")
            .cloned()
            .unwrap_or_else(|| ctx.run_id.clone());
        let mut parents = lock(&self.parents)?;
        let (stop, limiter) = if let Some(old) = parents.get(&ctx.run_id) {
            (old.stop.clone(), old.limiter.clone())
        } else {
            if parents.len() >= self.config.max_active_parents {
                return Err(AgentError::new(
                    ErrorCode::Limit,
                    "subagent parent registry capacity reached",
                ));
            }
            let limiter = if depth == 0 {
                AgentExecutionLimiter::new(self.config.max_parallel)
            } else {
                parents
                    .get(&root_run)
                    .filter(|p| p.root == root)
                    .ok_or_else(|| invalid("root task no longer owns this child"))?
                    .limiter
                    .clone()
            };
            (CancellationToken::new(), limiter)
        };
        let mut context = ctx.clone();
        // ContextTransform's operation token is cancelled when that hook returns. This
        // extension owns a separate lifetime token, cancelled by its awaited finish hook.
        context.cancel = stop.clone();
        parents.insert(
            ctx.run_id.clone(),
            Parent {
                ctx: context,
                history: Arc::new(history),
                driver,
                root,
                actor,
                depth,
                root_run,
                stop,
                limiter,
            },
        );
        Ok(())
    }
    async fn document(&self, root: &str, id: &str) -> Result<sessions::Document> {
        let root = root.to_owned();
        let id = id.to_owned();
        let store = self.store.clone();
        disk(move || {
            store.header(&root).map_err(storage)?;
            let doc = store.get(&id).map_err(storage)?;
            let p = Profile::read(&doc)?;
            if p.root != root {
                return Err(invalid("child does not belong to this task"));
            }
            Ok(doc)
        })
        .await
    }
    async fn authorized(&self, parent: &Parent, id: &str) -> Result<sessions::Document> {
        let doc = self.document(&parent.root, id).await?;
        if !parent.allows(&Profile::read(&doc)?) {
            return Err(invalid(
                "only the owner or root agent can control this child",
            ));
        }
        Ok(doc)
    }
    pub async fn view(&self, root: &str, id: &str) -> Result<ChildView> {
        let doc = self.document(root, id).await?;
        let mut v = crate::state::view(&doc)?;
        if let Some(error) = lock(&self.completion_errors)?.get(id) {
            v.status = sessions::Status::Failed;
            v.error_code = Some(error.code);
            v.output = None;
        }
        Ok(v)
    }
    pub async fn list(&self, root: &str) -> Result<Vec<ChildView>> {
        let root = root.to_owned();
        let store = self.store.clone();
        let headers = disk(move || {
            store.header(&root).map_err(storage)?;
            child_headers(&store, &root)
        })
        .await?;
        let mut rows = Vec::with_capacity(headers.len());
        for h in headers {
            rows.push(
                self.view(h.metadata.get(ROOT_KEY).expect("filtered"), &h.id)
                    .await?,
            );
        }
        rows.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(rows)
    }
    pub(crate) async fn owned_list(&self, parent: &Parent) -> Result<Vec<ChildView>> {
        Ok(self
            .list(&parent.root)
            .await?
            .into_iter()
            .filter(|v| parent.actor == parent.root || v.parent == parent.actor)
            .collect())
    }
    pub async fn spawn(
        self: &Arc<Self>,
        run: &str,
        operation: &str,
        request: Spawn,
    ) -> Result<ChildView> {
        request.validate()?;
        let parent = self.parent(run)?;
        let this = self.clone();
        let operation = operation.to_owned();
        // Owned admission survives a caller losing its response. The parent's finish token
        // still prevents late dispatch; saved operation identity is never blindly replayed.
        tokio::spawn(async move { this.spawn_inner(parent, &operation, request).await })
            .await
            .map_err(|_| {
                invalid("child admission outcome unknown; list children before another action")
            })?
    }
    async fn spawn_inner(
        self: &Arc<Self>,
        parent: Parent,
        operation: &str,
        request: Spawn,
    ) -> Result<ChildView> {
        let _admission = self.admission.lock().await;
        parent.check()?;
        if parent.depth >= self.config.max_depth {
            return Err(AgentError::new(
                ErrorCode::Limit,
                "delegation depth reached",
            ));
        }
        let role = self
            .config
            .roles
            .get(&request.role)
            .cloned()
            .ok_or_else(|| invalid("unknown host role"))?;
        let key = digest(&[&parent.ctx.run_id, operation]);
        let id = format!("s-ag-{}", &key[..56]);
        let fingerprint = digest(&[
            &serde_json::to_string(&request).map_err(|_| invalid("task encoding failed"))?
        ]);
        let store = self.store.clone();
        let existing_id = id.clone();
        let existing = disk(move || match store.get(&existing_id) {
            Ok(d) => Ok(Some(d)),
            Err(e) if e.code == sessions::SessionErrorCode::NotFound => Ok(None),
            Err(e) => Err(storage(e)),
        })
        .await?;
        if let Some(doc) = existing {
            let p = Profile::read(&doc)?;
            if !parent.allows(&p) || p.fingerprint != fingerprint {
                return Err(invalid(
                    "duplicate delegation identity has different content",
                ));
            }
            return self.view(&parent.root, &id).await;
        }
        if self.list(&parent.root).await?.len() >= self.config.max_children {
            return Err(AgentError::new(
                ErrorCode::Limit,
                "child-session capacity reached",
            ));
        }
        let permit = parent.limiter.reserve()?;
        let tools = tool_ceiling(&parent, &role, None, self.config.max_depth);
        let launch = parent.driver.prepare(&parent.ctx, &role)?;
        let history = if request.context == ContextMode::Fork {
            parent
                .history
                .iter()
                .filter(|m| !matches!(m, Message::System { .. }))
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        let profile = Profile {
            version: 1,
            root: parent.root.clone(),
            owner: parent.actor.clone(),
            depth: parent.depth + 1,
            role_name: request.role,
            role: role.clone(),
            tools,
            fingerprint,
            context: request.context,
        };
        let messages = messages(&parent, &role, history.clone(), &request.task);
        launch
            .runtime
            .validate_history(&messages, &launch.model_options)?;
        let owner = parent.actor.clone();
        let root = parent.root.clone();
        let store = self.store.clone();
        let p = profile.clone();
        let header = disk(move || {
            let parent = store.header(&owner).map_err(storage)?;
            let mut metadata = BTreeMap::from([(ROOT_KEY.into(), root), (OWNER_KEY.into(), owner)]);
            if let Some(project) = parent.metadata.get("project.id") {
                metadata.insert("project.id".into(), project.clone());
            }
            let h = store
                .create_in(
                    &format!("ag-{}", &key[..56]),
                    std::path::Path::new(&parent.workspace),
                    metadata,
                )
                .map_err(storage)?;
            let state = BTreeMap::from([(
                PROFILE_KEY.into(),
                serde_json::to_value(p).map_err(|_| invalid("child profile encoding failed"))?,
            )]);
            store
                .initialize_history(&h.id, h.revision, history, state)
                .map_err(storage)
        })
        .await?;
        parent.check()?;
        self.activate(
            parent,
            header,
            profile,
            request.task,
            operation,
            launch,
            permit,
        )
        .await
    }
    pub async fn followup(
        self: &Arc<Self>,
        run: &str,
        operation: &str,
        id: &str,
        task: &str,
    ) -> Result<ChildView> {
        text(task)?;
        let parent = self.parent(run)?;
        let this = self.clone();
        let id = id.to_owned();
        let task = task.to_owned();
        let operation = operation.to_owned();
        tokio::spawn(async move {
            let _admission = this.admission.lock().await;
            parent.check()?;
            let doc = this.authorized(&parent, &id).await?;
            let profile = Profile::read(&doc)?;
            let turn_id = format!("t-{}", &digest(&[&parent.ctx.run_id, &operation])[..56]);
            if let Some(turn) = doc.body.turns.iter().find(|turn| turn.id == turn_id) {
                if turn.prompt != task {
                    return Err(invalid(
                        "duplicate follow-up identity has different content",
                    ));
                }
                return this.view(&parent.root, &id).await;
            }
            if lock(&this.active)?.contains_key(&id) {
                return Err(invalid(
                    "child is running; send a message or wait before a follow-up",
                ));
            }
            let permit = parent.limiter.reserve()?;
            let launch = parent.driver.prepare(&parent.ctx, &profile.role)?;
            this.activate(
                parent, doc.header, profile, task, &operation, launch, permit,
            )
            .await
        })
        .await
        .map_err(|_| {
            invalid("follow-up outcome unknown; inspect the child before another action")
        })?
    }
    // Keep the admitted identity, immutable profile, launch and owned permit explicit
    // at the single dispatch boundary rather than introducing another lifecycle object.
    #[allow(clippy::too_many_arguments)]
    async fn activate(
        self: &Arc<Self>,
        parent: Parent,
        header: sessions::Header,
        profile: Profile,
        task: String,
        operation: &str,
        launch: Launch,
        permit: crate::execution::LocalExecutionPermit,
    ) -> Result<ChildView> {
        parent.check()?;
        let key = digest(&[&parent.ctx.run_id, operation]);
        let turn_id = format!("t-{}", &key[..56]);
        let store = self.store.clone();
        let id = header.id.clone();
        let k = turn_id.clone();
        let input = task.clone();
        let runtime = launch.runtime.clone();
        let check_parent = parent.clone();
        let check_role = profile.role.clone();
        let options = launch.model_options.clone();
        let model_store = self.store.clone();
        let model_id = header.id.clone();
        let model = launch.model.clone();
        let prepared = disk(move || {
            store
                .prepare_checked(
                    &id,
                    header.revision,
                    &k,
                    &input,
                    check_parent.ctx.limits.max_initial_history_bytes,
                    |history| {
                        runtime
                            .validate_history(
                                &messages(&check_parent, &check_role, history.to_vec(), &input),
                                &options,
                            )
                            .map_err(|e| {
                                sessions::SessionError::new(
                                    sessions::SessionErrorCode::InvalidRequest,
                                    e.message,
                                )
                            })
                    },
                )
                .map_err(storage)
        })
        .await?;
        let history = match prepared {
            sessions::Prepared::Existing(_, _) => return self.view(&parent.root, &header.id).await,
            sessions::Prepared::New { history } => history,
        };
        if let Err(error) = disk(move || {
            model_store
                .update_auxiliary_state(&model_id, "aux.subagent.model", |_| {
                    serde_json::to_value(model).map_err(|_| {
                        sessions::SessionError::new(
                            sessions::SessionErrorCode::Internal,
                            "model identity encoding failed",
                        )
                    })
                })
                .map_err(storage)
        })
        .await
        {
            let store = self.store.clone();
            let id = header.id.clone();
            let key = turn_id.clone();
            disk(move || {
                store
                    .fail_start(&id, &key, "child model binding could not be saved")
                    .map_err(storage)
            })
            .await?;
            return Err(error);
        }
        let id = header.id.clone();
        if let Err(error) = parent.check() {
            let store = self.store.clone();
            let id = id.clone();
            let k = turn_id.clone();
            disk(move || {
                store
                    .fail_start(&id, &k, "parent ended before child dispatch")
                    .map_err(storage)
            })
            .await?;
            return Err(error);
        }
        let mut metadata = launch.metadata;
        metadata.insert(sessions::SESSION_KEY.into(), id.clone());
        metadata.insert(sessions::TURN_KEY.into(), turn_id.clone());
        metadata.insert("subagent.root_run".into(), parent.root_run.clone());
        let child_task = parent.ctx.task.branch();
        let request = RunRequest {
            messages: messages(&parent, &profile.role, history, &task),
            limits: parent.ctx.limits.clone(),
            task: child_task.clone(),
            metadata,
            enable_tools: parent.ctx.tools_enabled,
            allowed_tools: Some(tool_ceiling(
                &parent,
                &profile.role,
                Some(&profile),
                self.config.max_depth,
            )),
            model_options: launch.model_options,
        };
        let handle = match launch.runtime.start(request) {
            Ok(h) => h,
            Err(error) => {
                let store = self.store.clone();
                let sid = id.clone();
                let k = turn_id;
                let msg = error.message.clone();
                disk(move || store.fail_start(&sid, &k, &msg).map_err(storage)).await?;
                return Err(error);
            }
        };
        let session = handle.session();
        let (tx, done) = watch::channel(None);
        let active = Arc::new(Active {
            parent_run: parent.ctx.run_id.clone(),
            root: parent.root.clone(),
            session: session.clone(),
            task: child_task.clone(),
            done,
        });
        lock(&self.completion_errors)?.remove(&id);
        lock(&self.active)?.insert(id.clone(), active.clone());
        self.changed();
        let this = self.clone();
        let sid = id.clone();
        let stop = parent.stop.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let result = tokio::select! {
                result=session.wait()=>result.map(|_|()),
                _=stop.cancelled()=>{child_task.cancel();session.cancel();session.wait().await.map(|_|())}
            };
            if let Err(error) = &result {
                if let Ok(mut errors) = this.completion_errors.lock() {
                    errors.insert(sid.clone(), error.clone());
                }
            }
            // An observed completion must make both activation identity and capacity reusable.
            drop(_permit);
            if let Ok(mut runs) = this.active.lock() {
                if runs.get(&sid).is_some_and(|a| Arc::ptr_eq(a, &active)) {
                    runs.remove(&sid);
                }
            }
            tx.send_replace(Some(result));
            this.changed();
        });
        self.view(&parent.root, &id).await
    }
    pub async fn send_message(
        self: &Arc<Self>,
        run: &str,
        operation: &str,
        id: &str,
        message: &str,
    ) -> Result<()> {
        text(message)?;
        let parent = self.parent(run)?;
        self.authorized(&parent, id).await?;
        let mail = Mail {
            id: digest(&[run, operation]),
            from: parent.actor,
            text: message.into(),
        };
        let store = self.store.clone();
        let id = id.to_owned();
        disk(move || {
            store
                .update_auxiliary_state(&id, INBOX_KEY, |previous| {
                    let mut messages: Vec<Mail> = previous
                        .map(|v| serde_json::from_value(v.clone()))
                        .transpose()
                        .map_err(|_| {
                            sessions::SessionError::new(
                                sessions::SessionErrorCode::InvalidRequest,
                                "invalid child inbox",
                            )
                        })?
                        .unwrap_or_default();
                    if let Some(old) = messages.iter().find(|m| m.id == mail.id) {
                        if old.text != mail.text || old.from != mail.from {
                            return Err(sessions::SessionError::new(
                                sessions::SessionErrorCode::Conflict,
                                "same message identity has different content",
                            ));
                        }
                    } else {
                        messages.push(mail);
                    }
                    validate_mail(&messages).map_err(|e| {
                        sessions::SessionError::new(sessions::SessionErrorCode::Capacity, e.message)
                    })?;
                    serde_json::to_value(messages).map_err(|_| {
                        sessions::SessionError::new(
                            sessions::SessionErrorCode::Internal,
                            "message encoding failed",
                        )
                    })
                })
                .map_err(storage)
        })
        .await?;
        self.changed();
        Ok(())
    }
    /// Wait for terminal child results without changing the child's execution lifetime.
    pub async fn wait(
        self: &Arc<Self>,
        run: &str,
        ids: &[String],
        duration: Duration,
        cancel: &CancellationToken,
    ) -> Result<WaitResult> {
        if ids.is_empty() || ids.len() > 16 || duration > Duration::from_secs(20) {
            return Err(invalid(
                "wait requires 1..16 children and at most 20 seconds",
            ));
        }
        let parent = self.parent(run)?;
        for id in ids {
            self.authorized(&parent, id).await?;
        }
        let mut change = self.changed.subscribe();
        let deadline = (tokio::time::Instant::now() + duration).min(parent.ctx.task.deadline());
        loop {
            let mut views = vec![];
            for id in ids {
                views.push(self.view(&parent.root, id).await?);
            }
            let settling = lock(&self.active)?
                .iter()
                .any(|(id, a)| ids.contains(id) && a.done.borrow().is_none());
            if !settling && views.iter().all(|v| v.status != sessions::Status::Running) {
                return Ok(WaitResult {
                    timed_out: false,
                    agents: views,
                });
            }
            tokio::select! {
                biased;
                _=cancel.cancelled()=>return Err(AgentError::new(ErrorCode::Cancelled,"waiting caller cancelled; child lifetime unchanged")),
                _=parent.stop.cancelled()=>return Err(AgentError::new(ErrorCode::Cancelled,"parent stopped")),
                _=tokio::time::sleep_until(deadline)=>return Ok(WaitResult{timed_out:true,agents:views}),
                result=change.changed()=>{result.map_err(|_|invalid("child status channel closed"))?;}
            }
        }
    }
    pub async fn interrupt(
        &self,
        root: &str,
        id: &str,
        expected_run: Option<&str>,
    ) -> Result<ChildView> {
        self.document(root, id).await?;
        let active = lock(&self.active)?.get(id).cloned();
        if let Some(active) = active {
            if expected_run.is_some_and(|r| r != active.session.run_id()) {
                return Err(invalid("child activation changed; refresh before stopping"));
            }
            active.task.cancel();
            active.session.cancel();
            wait_done(active.done.clone()).await?;
        }
        self.view(root, id).await
    }
    pub(crate) async fn finish(&self, run: &str) -> Result<()> {
        let parent = lock(&self.parents)?.remove(run);
        if let Some(p) = parent {
            p.stop.cancel();
        }
        let children: Vec<_> = {
            let _admission = self.admission.lock().await;
            lock(&self.active)?
                .values()
                .filter(|a| a.parent_run == run)
                .cloned()
                .collect()
        };
        if children.is_empty() {
            return Ok(());
        }
        let unfinished = children
            .iter()
            .any(|a| a.session.snapshot().outcome.is_none());
        for a in &children {
            if a.session.snapshot().outcome.is_none() {
                a.task.cancel();
                a.session.cancel();
            }
        }
        settle_all(children).await?;
        if unfinished {
            Err(invalid("parent ended with unfinished delegated work; children were stopped, not treated as completed"))
        } else {
            Ok(())
        }
    }
    pub async fn shutdown(&self) -> Result<()> {
        self.closed.store(true, Ordering::Release);
        for p in lock(&self.parents)?.values() {
            p.stop.cancel();
        }
        let children: Vec<_> = {
            let _admission = self.admission.lock().await;
            lock(&self.active)?.values().cloned().collect()
        };
        for a in &children {
            a.task.cancel();
            a.session.cancel();
        }
        let settled = settle_all(children).await;
        lock(&self.parents)?.clear();
        settled
    }
    /// Called only after the root's deletion was acknowledged. Delete child records,
    /// not workspace files. Partial cleanup errors are explicit and the operation is retryable.
    pub async fn purge_deleted_root(&self, root: &str) -> Result<Vec<String>> {
        let _admission = self.admission.lock().await;
        if self.has_active_children(root) {
            return Err(invalid(
                "child executions must settle before deleting their records",
            ));
        }
        let root = root.to_owned();
        let store = self.store.clone();
        let removed = disk(move || {
            match store.header(&root) {
                Ok(_) => return Err(invalid("root still exists; refusing child record cleanup")),
                Err(e) if e.code == sessions::SessionErrorCode::NotFound => {}
                Err(e) => return Err(storage(e)),
            }
            let headers = child_headers(&store, &root)?;
            let mut removed = vec![];
            for h in headers {
                store.delete(&h.id, h.revision).map_err(storage)?;
                removed.push(h.id);
            }
            Ok(removed)
        })
        .await?;
        let mut errors = lock(&self.completion_errors)?;
        for id in &removed {
            errors.remove(id);
        }
        Ok(removed)
    }
    pub fn has_active_children(&self, root: &str) -> bool {
        lock(&self.active).map_or(true, |runs| runs.values().any(|a| a.root == root))
    }
}
fn child_headers(store: &sessions::Store, root: &str) -> Result<Vec<sessions::Header>> {
    let mut found = vec![];
    for archived in [false, true] {
        let mut offset = 0;
        loop {
            let page = store
                .list_matching(offset, 100, "", archived, |h| {
                    h.metadata.get(ROOT_KEY).is_some_and(|v| v == root)
                })
                .map_err(storage)?;
            found.extend(page.sessions);
            if found.len() > 128 {
                return Err(AgentError::new(
                    ErrorCode::Limit,
                    "saved child count exceeds supported capacity",
                ));
            }
            match page.next_offset {
                Some(n) => offset = n,
                None => break,
            }
        }
    }
    Ok(found)
}
async fn settle_all(children: Vec<Arc<Active>>) -> Result<()> {
    let mut failure = None;
    for child in children {
        if let Err(error) = wait_done(child.done.clone()).await {
            if failure.is_none() {
                failure = Some(error);
            }
        }
    }
    failure.map_or(Ok(()), Err)
}
async fn wait_done(mut rx: watch::Receiver<Option<Result<()>>>) -> Result<()> {
    loop {
        if let Some(result) = rx.borrow().clone() {
            return result;
        }
        rx.changed()
            .await
            .map_err(|_| invalid("child settlement lost"))?;
    }
}
fn digest(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for s in parts {
        h.update((s.len() as u64).to_le_bytes());
        h.update(s.as_bytes());
    }
    format!("{:x}", h.finalize())
}
fn messages(parent: &Parent, role: &Role, history: Vec<Message>, task: &str) -> Vec<Message> {
    let mut messages: Vec<_> = parent
        .history
        .iter()
        .take_while(|m| matches!(m, Message::System { .. }))
        .cloned()
        .collect();
    messages.push(Message::system(format!("Delegated agent. Work only on the assigned task; return evidence and unresolved items, not an unverified success claim. Do not change the parent plan or undo another agent's work. {}",role.instructions)));
    messages.extend(history);
    messages.push(Message::user(task));
    messages
}
fn tool_ceiling(
    parent: &Parent,
    role: &Role,
    profile: Option<&Profile>,
    max_depth: u32,
) -> BTreeSet<String> {
    let depth = profile.map_or(parent.depth + 1, |p| p.depth);
    parent
        .ctx
        .request_tools
        .iter()
        .map(|t| t.name.clone())
        .filter(|n| role.tools.as_ref().is_none_or(|s| s.contains(n)))
        .filter(|n| profile.is_none_or(|p| p.tools.contains(n)))
        .filter(|n| depth < max_depth || !TOOL_NAMES.contains(&n.as_str()))
        .collect()
}
