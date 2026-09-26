use crate::{invalid, Service};
use api::*;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
pub const TOOL_NAMES: [&str; 6] = [
    "spawn_agent",
    "send_message",
    "followup_agent",
    "wait_agent",
    "interrupt_agent",
    "list_agents",
];
pub(crate) struct AgentTool {
    pub service: Arc<Service>,
    pub name: &'static str,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    id: String,
    text: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Wait {
    ids: Vec<String>,
    #[serde(default = "wait_ms")]
    timeout_ms: u64,
}
fn wait_ms() -> u64 {
    10000
}
fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(|e| invalid(format!("invalid subagent arguments: {e}")))
}
fn as_value(value: impl serde::Serialize) -> Result<Value> {
    serde_json::to_value(value).map_err(|_| invalid("subagent result encoding failed"))
}
#[async_trait]
impl Tool for AgentTool {
    fn spec(&self) -> ToolSpec {
        let text = json!({"type":"string","minLength":1,"maxLength":16384});
        let id = json!({"type":"string","minLength":1,"maxLength":64});
        let(desc,properties,required)=match self.name {
            "spawn_agent"=>("Start a scoped child task asynchronously; returns its id. Independent context is default; fork receives a settled parent snapshot, never its in-flight tool batch. Collect results with wait_agent before finishing.",
                json!({"task":text,"role":{"type":"string","maxLength":64},"context":{"type":"string","enum":["independent","fork"]}}),json!(["task"])),
            "send_message"=>("Persist supplementary information for an owned child. It is available on the child's next model request; this does not start an idle child or claim the message was read.",json!({"id":id,"text":text}),json!(["id","text"])),
            "followup_agent"=>("Start new work on an idle owned child, reusing its saved history. Do not replay unknown side effects after failure; inspect evidence first. Shares the current parent task budget.",json!({"id":id,"text":text}),json!(["id","text"])),
            "wait_agent"=>("Wait for owned child results. A timeout only ends this wait, not the children. Results identify actual terminal status; failed/cancelled children are not completed work.",json!({"ids":{"type":"array","items":id,"minItems":1,"maxItems":16,"uniqueItems":true},"timeout_ms":{"type":"integer","minimum":0,"maximum":20000}}),json!(["ids"])),
            "interrupt_agent"=>("Stop an owned child and wait for its execution to settle. This does not cancel siblings, delete records, or roll back file changes.",json!({"id":id}),json!(["id"])),
            _=>("List owned child identities, status and per-activation usage, including saved children from earlier turns. Root can inspect descendants; children can inspect their own children.",json!({}),json!([])),
        };
        ToolSpec {
            name: self.name.into(),
            description: desc.into(),
            parameters: json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),
            concurrency: ToolConcurrency::ParallelSafe,
            side_effects: !matches!(self.name, "wait_agent" | "list_agents"),
        }
    }
    async fn execute(&self, ctx: ToolContext, args: Value) -> Result<ToolOutput> {
        let parent = self.service.parent(&ctx.run.run_id)?;
        let value = match self.name {
            "spawn_agent" => as_value(
                self.service
                    .spawn(&ctx.run.run_id, &ctx.call_id, decode(args)?)
                    .await?,
            )?,
            "followup_agent" => {
                let a: Text = decode(args)?;
                as_value(
                    self.service
                        .followup(&ctx.run.run_id, &ctx.call_id, &a.id, &a.text)
                        .await?,
                )?
            }
            "send_message" => {
                let a: Text = decode(args)?;
                self.service
                    .send_message(&ctx.run.run_id, &ctx.call_id, &a.id, &a.text)
                    .await?;
                json!({"id":a.id,"accepted":true,"starts_turn":false,"delivery":"next_model_request"})
            }
            "wait_agent" => {
                let a: Wait = decode(args)?;
                let mut result = self
                    .service
                    .wait(
                        &ctx.run.run_id,
                        &a.ids,
                        Duration::from_millis(a.timeout_ms),
                        &ctx.run.cancel,
                    )
                    .await?;
                for v in &mut result.agents {
                    if let Some(out) = &mut v.output {
                        if out.len() > 2048 {
                            *out = api::clip_utf8(out, 2048).into();
                            v.output_truncated = true;
                        }
                    }
                }
                as_value(result)?
            }
            "interrupt_agent" => {
                let a: Target = decode(args)?;
                let owned = self.service.owned_list(&parent).await?;
                let target = owned
                    .iter()
                    .find(|v| v.id == a.id)
                    .ok_or_else(|| invalid("child not owned by caller"))?;
                as_value(
                    self.service
                        .interrupt(&parent.root, &a.id, target.run_id.as_deref())
                        .await?,
                )?
            }
            _ => {
                let mut rows = self.service.owned_list(&parent).await?;
                for v in &mut rows {
                    v.output = None;
                }
                as_value(rows)?
            }
        };
        // Detail is a view only. Final tool output remains the authoritative observation.
        let _ = ctx.progress.set_detail(
            "subagent.status",
            json!({"operation":self.name,"root":parent.root}),
        );
        Ok(ToolOutput::new(
            serde_json::to_string(&value)
                .map_err(|_| invalid("subagent output encoding failed"))?,
        ))
    }
}
