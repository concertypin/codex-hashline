//! Create or overwrite a file.

use std::path::PathBuf;
use std::sync::Arc;
use codex_tools::tool_executor::{CoreToolRuntime, ToolCallResult, ToolOutput};

pub struct HashlineWriteHandler;

#[derive(serde::Deserialize)]
pub struct WriteArgs {
    /// File path (relative or absolute)
    pub path: String,
    /// Content to write
    pub content: String,
}

impl CoreToolRuntime for HashlineWriteHandler {
    fn call(&self, args: serde_json::Value, _context: Arc<dyn codex_tools::tool_executor::ToolCallContext>) -> ToolCallResult {
        Box::pin(async move {
            let args: WriteArgs = serde_json::from_value(args)
                .map_err(|e| anyhow::anyhow!("invalid write args: {e}"))?;

            let path = PathBuf::from(&args.path);

            // Ensure parent directory exists
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await
                    .map_err(|e| anyhow::anyhow!("cannot create dir {}: {e}", parent.display()))?;
            }

            tokio::fs::write(&path, &args.content).await
                .map_err(|e| anyhow::anyhow!("cannot write {}: {e}", args.path))?;

            let line_count = args.content.lines().count();
            Ok(ToolOutput::Custom(serde_json::json!({
                "status": "written",
                "path": args.path,
                "lines": line_count,
            })))
        })
    }

    fn tool_name(&self) -> String { "write_file".into() }
    fn tool_description(&self) -> String {
        "Create a new file or overwrite an existing file with the given content. \
         Use for full-file writes; for targeted edits, prefer `hashline_patch`."
            .into()
    }
    fn tool_input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path (relative or absolute)"
                },
                "content": {
                    "type": "string",
                    "description": "Full content to write"
                }
            },
            "required": ["path", "content"]
        })
    }
}
