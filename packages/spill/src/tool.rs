use crate::{error, validate_spill_id, SpillStore};
use api::{Tool, ToolConcurrency, ToolContext, ToolOutput, ToolSpec};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub struct SpillReadTool {
    store: Arc<dyn SpillStore>,
    max_page_bytes: usize,
}

impl SpillReadTool {
    pub fn new(store: Arc<dyn SpillStore>, max_page_bytes: usize) -> Self {
        Self {
            store,
            max_page_bytes,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    id: String,
    #[serde(default)]
    offset: usize,
    limit: usize,
}

#[async_trait]
impl Tool for SpillReadTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "spill_read".into(),
            description: "Read a bounded UTF-8 page from a previously archived tool result. Use the opaque id from its spill artifact, then continue with next_offset until eof.".into(),
            parameters: json!({
                "type":"object",
                "properties": {
                    "id":{"type":"string","minLength":4,"maxLength":96,"pattern":"^sp_[A-Za-z0-9_]+$"},
                    "offset":{"type":"integer","minimum":0},
                    "limit":{"type":"integer","minimum":1,"maximum": self.max_page_bytes}
                },
                "required":["id","limit"],
                "additionalProperties":false
            }),
            concurrency: ToolConcurrency::ParallelSafe,
            side_effects: false,
        }
    }

    async fn execute(&self, ctx: ToolContext, arguments: Value) -> api::Result<ToolOutput> {
        ctx.run.task.check()?;
        if ctx.run.cancel.is_cancelled() {
            return Err(error(api::ErrorCode::Cancelled, "spill read cancelled"));
        }
        let args: Arguments = serde_json::from_value(arguments)
            .map_err(|_| error(api::ErrorCode::Schema, "invalid spill_read arguments"))?;
        validate_spill_id(&args.id)?;
        if args.limit == 0 || args.limit > self.max_page_bytes {
            return Err(error(
                api::ErrorCode::Limit,
                "spill page exceeds its byte limit",
            ));
        }
        let result_limit = ctx.run.limits.max_tool_result_bytes;
        if result_limit == 0 {
            return Ok(ToolOutput::error(
                "spill page cannot fit within the run result limit",
            ));
        }

        // A page's structured metadata is part of the result budget too. Start
        // with a request that the Run can carry, then retry at the remaining
        // text budget when the metadata leaves less room than requested.
        let mut page_limit = args.limit.min(result_limit);
        loop {
            ctx.run.task.check()?;
            if ctx.run.cancel.is_cancelled() {
                return Err(error(api::ErrorCode::Cancelled, "spill read cancelled"));
            }
            let page = self
                .store
                .read_page(&ctx.run.run_id, &args.id, args.offset, page_limit)
                .await?;
            ctx.run.task.check()?;
            if ctx.run.cancel.is_cancelled() {
                return Err(error(api::ErrorCode::Cancelled, "spill read cancelled"));
            }
            let details = json!({
                "id": page.id,
                "offset": page.offset,
                "next_offset": page.next_offset,
                "total_bytes": page.total_bytes,
                "eof": page.eof,
            });
            let metadata_bytes = serde_json::to_vec(&details).map_err(|_| {
                error(
                    api::ErrorCode::Tool,
                    "spill page metadata cannot be serialized",
                )
            })?;
            let text_limit = result_limit.saturating_sub(metadata_bytes.len());
            if metadata_bytes.len() <= result_limit && page.text.len() <= text_limit {
                let mut output = ToolOutput::new(page.text);
                output.structured = Some(details);
                return Ok(output);
            }
            if text_limit == 0 {
                return Ok(ToolOutput::error(
                    "spill page metadata exceeds the run result limit",
                ));
            }

            // SpillStore implementations must honor the requested UTF-8 byte
            // limit. A non-decreasing retry would otherwise loop forever when
            // an invalid backend returns an oversized page.
            if text_limit >= page_limit {
                return Ok(ToolOutput::error("spill store returned an oversized page"));
            }
            page_limit = text_limit;
        }
    }
}
