use crate::{config::name, invalid, storage, Role};
use api::{Message, Result, TaskUsage};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
pub const ROOT_KEY: &str = "subagent.root";
pub const OWNER_KEY: &str = "subagent.owner";
pub(crate) const PROFILE_KEY: &str = "subagent";
pub(crate) const INBOX_KEY: &str = "aux.subagent.inbox";
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContextMode {
    #[default]
    Independent,
    Fork,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spawn {
    pub task: String,
    #[serde(default = "default_role")]
    pub role: String,
    #[serde(default)]
    pub context: ContextMode,
}
fn default_role() -> String {
    "default".into()
}
impl Spawn {
    pub(crate) fn validate(&self) -> Result<()> {
        text(&self.task)?;
        if !name(&self.role) {
            return Err(invalid("invalid role name"));
        }
        Ok(())
    }
}
pub(crate) fn text(value: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 16 * 1024 {
        return Err(invalid("task/message must contain 1..16384 bytes"));
    }
    Ok(())
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Profile {
    pub version: u32,
    pub root: String,
    pub owner: String,
    pub depth: u32,
    pub role_name: String,
    pub role: Role,
    pub tools: BTreeSet<String>,
    pub fingerprint: String,
    pub context: ContextMode,
}
impl Profile {
    pub fn read(doc: &sessions::Document) -> Result<Self> {
        let value = doc.body.state.get(PROFILE_KEY).ok_or_else(|| {
            invalid("child initialization is incomplete; not automatically replayed")
        })?;
        let p: Self = serde_json::from_value(value.clone())
            .map_err(|_| invalid("invalid saved subagent profile"))?;
        p.role.validate()?;
        if p.version != 1
            || !(1..=4).contains(&p.depth)
            || !name(&p.role_name)
            || p.tools.len() > 128
            || p.tools.iter().any(|n| !name(n))
            || doc.header.metadata.get(ROOT_KEY) != Some(&p.root)
            || doc.header.metadata.get(OWNER_KEY) != Some(&p.owner)
            || p.fingerprint.len() != 64
        {
            return Err(invalid("saved subagent identity does not match its owner"));
        }
        sessions::validate_id(&p.root).map_err(storage)?;
        sessions::validate_id(&p.owner).map_err(storage)?;
        Ok(p)
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Mail {
    pub id: String,
    pub from: String,
    pub text: String,
}
pub(crate) fn inbox(doc: &sessions::Document) -> Result<Vec<Mail>> {
    let messages: Vec<Mail> = doc
        .body
        .state
        .get(INBOX_KEY)
        .map(|v| serde_json::from_value(v.clone()))
        .transpose()
        .map_err(|_| invalid("invalid saved child inbox"))?
        .unwrap_or_default();
    validate_mail(&messages)?;
    Ok(messages)
}
pub(crate) fn validate_mail(messages: &[Mail]) -> Result<()> {
    if messages.len() > 32
        || serde_json::to_vec(messages)
            .map_err(|_| invalid("inbox encoding failed"))?
            .len()
            > 32 * 1024
    {
        return Err(invalid("child inbox capacity reached"));
    }
    let mut ids = BTreeSet::new();
    for m in messages {
        text(&m.text)?;
        if !name(&m.from) || m.id.len() != 64 || !ids.insert(&m.id) {
            return Err(invalid("invalid child message identity"));
        }
    }
    Ok(())
}
#[derive(Clone, Debug, Serialize)]
pub struct ChildView {
    pub id: String,
    pub root: String,
    pub parent: String,
    pub role: String,
    pub depth: u32,
    pub title: String,
    pub status: sessions::Status,
    pub revision: u64,
    pub turns: usize,
    pub run_id: Option<String>,
    pub steps: usize,
    pub usage: TaskUsage,
    pub output: Option<String>,
    pub output_truncated: bool,
    pub error_code: Option<api::ErrorCode>,
    pub model: Option<crate::ModelRoute>,
}
pub(crate) fn view(doc: &sessions::Document) -> Result<ChildView> {
    let p = Profile::read(doc)?;
    let turn = doc.body.turns.last();
    let output = if doc.header.status == sessions::Status::Completed {
        doc.body
            .checkpoint
            .as_ref()
            .and_then(|cp| {
                cp.transcript.iter().rev().find_map(|m| match m {
                    Message::Assistant {
                        content,
                        tool_calls,
                        ..
                    } if tool_calls.is_empty() => Some(content),
                    _ => None,
                })
            })
            .map(|s| (api::clip_utf8(s, 8192).to_owned(), s.len() > 8192))
    } else {
        None
    };
    Ok(ChildView {
        id: doc.header.id.clone(),
        root: p.root,
        parent: p.owner,
        role: p.role_name,
        depth: p.depth,
        title: doc.header.title.clone(),
        status: doc.header.status,
        revision: doc.header.revision,
        turns: doc.header.turn_count,
        run_id: turn.and_then(|t| t.run_id.clone()),
        steps: turn.map_or(0, |t| t.steps),
        usage: turn.map(|t| t.usage.clone()).unwrap_or(TaskUsage {
            model_calls: 0,
            reported_tokens: 0,
            usage_complete: true,
        }),
        output: output.as_ref().map(|o| o.0.clone()),
        output_truncated: output.is_some_and(|o| o.1),
        error_code: turn.and_then(|t| t.error.as_ref().map(|e| e.code)),
        model: doc
            .body
            .state
            .get("aux.subagent.model")
            .map(|v| serde_json::from_value(v.clone()))
            .transpose()
            .map_err(|_| invalid("invalid saved model identity"))?
            .flatten(),
    })
}
#[derive(Serialize)]
pub struct WaitResult {
    pub timed_out: bool,
    pub agents: Vec<ChildView>,
}
