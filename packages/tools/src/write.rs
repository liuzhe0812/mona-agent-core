use crate::{
    support::{error, mutate_file, resolve_path},
    ToolConfig,
};
use api::{async_trait, Tool, ToolConcurrency, ToolContext, ToolOutput, ToolSpec};
use serde::Deserialize;
use serde_json::{json, Value};

pub struct WriteTool {
    config: ToolConfig,
}

impl WriteTool {
    pub fn new(config: ToolConfig) -> Self {
        Self { config }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    path: String,
    content: String,
}

#[async_trait]
impl Tool for WriteTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write".into(),
            description: "Create or overwrite a UTF-8 text file. Parent directories are created automatically.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "minLength": 1, "description": "Path to the file to write (relative or absolute)"},
                    "content": {"type": "string", "description": "Complete file content"}
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }),
            concurrency: ToolConcurrency::Exclusive,
            side_effects: true,
        }
    }

    async fn execute(&self, ctx: ToolContext, arguments: Value) -> api::Result<ToolOutput> {
        ctx.run.task.check()?;
        let args: Arguments = serde_json::from_value(arguments)
            .map_err(|_| error(api::ErrorCode::Schema, "invalid write arguments"))?;
        let path = resolve_path(&self.config.cwd, &args.path)?;
        let path_text = path.display().to_string();
        let bytes = args.content.into_bytes();
        let byte_count = bytes.len();
        mutate_file(path.clone(), &ctx, move |_| Ok(bytes)).await?;
        ctx.run.task.check()?;

        let detail = json!({
            "path": path_text,
            "bytes": byte_count,
        });
        let mut output = ToolOutput::new(format!("Successfully wrote to {}", args.path));
        output.structured = Some(detail);
        Ok(output)
    }
}
