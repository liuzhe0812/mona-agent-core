//! Optional session-to-application adapter. Reuses the application's existing run registry.
//! Session storage/recovery is implemented in the `sessions` extension, not here.
use crate::{
    AgentApplication, ApplicationError, ApplicationErrorCode as Code, ApplicationResult,
    StartRequest,
};
use api::ErrorCode;
use serde::{Deserialize, Serialize};
use sessions::{
    Header, Prepared, SessionError, SessionErrorCode, SessionResult, Store, SESSION_KEY, TURN_KEY,
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
};
use tokio::sync::Mutex;

impl From<SessionError> for ApplicationError {
    fn from(error: SessionError) -> Self {
        let code = match error.code {
            SessionErrorCode::InvalidRequest => Code::InvalidRequest,
            SessionErrorCode::NotFound => Code::NotFound,
            SessionErrorCode::Conflict => Code::Conflict,
            SessionErrorCode::Closed => Code::Closed,
            SessionErrorCode::Capacity => Code::Capacity,
            SessionErrorCode::Internal => Code::Internal,
        };
        Self::new(code, error.message)
    }
}
async fn disk<T: Send + 'static>(
    action: impl FnOnce() -> SessionResult<T> + Send + 'static,
) -> ApplicationResult<T> {
    tokio::task::spawn_blocking(action)
        .await
        .map_err(|_| ApplicationError::new(Code::Internal, "session storage worker failed"))?
        .map_err(Into::into)
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnRequest {
    pub request_id: String,
    pub revision: u64,
    pub prompt: String,
}
#[derive(Serialize)]
pub struct TurnResponse {
    pub session: Header,
    pub turn_id: String,
    pub run_id: Option<String>,
    pub reused: bool,
    pub live: bool,
}

/// Trusted, pure host projection. Called under exact session admission, never from browser metadata.
pub type SessionContext = Arc<dyn Fn(&sessions::Document) -> SessionResult<BTreeMap<String, String>> + Send + Sync>;

#[derive(Clone)]
pub struct SessionApplication {
    store: Arc<Store>,
    app: AgentApplication,
    // Only admission and retained-run cleanup are serialized, never model/tool execution.
    starts: Arc<Mutex<VecDeque<String>>>,
    context: Option<SessionContext>,
}
impl SessionApplication {
    pub fn new(store: Arc<Store>, app: AgentApplication) -> Self {
        Self {
            store,
            app,
            starts: Arc::new(Mutex::new(VecDeque::new())),
            context: None,
        }
    }
    pub fn with_context(mut self, context: SessionContext) -> Self {
        self.context = Some(context);
        self
    }
    /// Accepted admission completes even when a transport caller disconnects.
    /// A saved duplicate never starts another Run, including after process restart.
    pub async fn start_turn(
        &self,
        id: String,
        request: TurnRequest,
    ) -> ApplicationResult<TurnResponse> {
        let this = self.clone();
        tokio::spawn(async move { this.start(id, request).await })
            .await
            .map_err(|_| {
                ApplicationError::new(
                    Code::Internal,
                    "session start outcome unknown; inspect saved history before submitting again",
                )
            })?
    }
    async fn start(&self, id: String, request: TurnRequest) -> ApplicationResult<TurnResponse> {
        let mut retained = self.starts.lock().await;
        retained.retain(|run| match self.app.get_result(run) {
            Ok(result)
                if result.outcome.as_ref().is_some_and(|o| {
                    o.error
                        .as_ref()
                        .is_none_or(|e| e.code != ErrorCode::Checkpoint)
                }) =>
            {
                let _ = self.app.forget(run);
                false
            }
            Err(e) if e.code == Code::NotFound => false,
            _ => true,
        });
        let limit = self.app.session_admission_bytes()?;
        let store = self.store.clone();
        let sid = id.clone();
        let req = request.clone();
        let app = self.app.clone();
        let context = self.context.clone();
        let (prepared, mut metadata) = disk(move || store.prepare_with_context(
            &sid, req.revision, &req.request_id, &req.prompt, limit,
            |doc, history| {
                app.validate_session_history(history).map_err(|failure| {
                    let failure = ApplicationError::from(failure);
                    SessionError::new(SessionErrorCode::InvalidRequest, failure.message)
                })?;
                context.as_ref().map_or_else(|| Ok(BTreeMap::new()), |project| project(doc))
            },
        )).await?;
        if let Prepared::Existing(turn, session) = prepared {
            let live = turn
                .run_id
                .as_ref()
                .is_some_and(|run| self.app.get_snapshot(run).is_ok());
            return Ok(TurnResponse {
                session,
                turn_id: turn.id,
                run_id: turn.run_id,
                reused: true,
                live,
            });
        }
        let Prepared::New { history } = prepared else {
            unreachable!()
        };
        metadata.insert(SESSION_KEY.into(), id.clone());
        metadata.insert(TURN_KEY.into(), request.request_id.clone());
        let request_key = format!(
            "session-{:x}",
            Sha256::digest(format!("{}:{}", id, request.request_id).as_bytes())
        );
        let response = match self.app.start_task_with_history(
            StartRequest {
                request_id: request_key,
                prompt: request.prompt,
            },
            history,
            metadata,
        ) {
            Ok(response) => response,
            Err(failure) => {
                let store = self.store.clone();
                let sid = id.clone();
                let key = request.request_id.clone();
                let message = failure.message.clone();
                disk(move || store.fail_start(&sid, &key, &message)).await?;
                return Err(failure);
            }
        };
        let run_id = response.run_id.clone();
        let store = self.store.clone();
        let key = request.request_id.clone();
        let session = match disk(move || store.bind(&id, &key, &run_id)).await {
            Ok(header) => header,
            Err(failure) => {
                let _ = self.app.cancel_task(&response.run_id);
                return Err(failure);
            }
        };
        retained.push_back(response.run_id.clone());
        Ok(TurnResponse {
            session,
            turn_id: request.request_id,
            run_id: Some(response.run_id),
            reused: false,
            live: true,
        })
    }
}
