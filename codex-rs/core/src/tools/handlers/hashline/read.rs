//! Read a file with hashline-anchored line output.

use crate::tools::handlers::hashline;
use codex_tools::tool_executor::{CoreToolRuntime, ToolCallResult, ToolOutput};
use std::path::PathBuf;
use std::sync::Arc;

pub struct HashlineReadHandler;

#[derive(serde::Deserialize)]
pub struct ReadArgs {
    /// File path (relative or absolute)
    pub path: String,
    /// Start line (1-indexed, default 1)
    pub offset: Option<usize>,
    /// Max lines (default 2000)
    pub limit: Option<usize>,
}

#[derive(serde::Serialize)]
struct ReadResult {
    header: String,
    lines: Vec<String>,
    total_lines: usize,
}

impl CoreToolRuntime for HashlineReadHandler {
    fn call(&self, args: serde_json::Value, _context: Arc<dyn codex_tools::tool_executor::ToolCallContext>) -> ToolCallResult {
        Box::pin(async move {
            let args: ReadArgs = serde_json::from_value(args)
                .map_err(|e| anyhow::anyhow!("invalid read args: {e}"))?;

            let path = PathBuf::from(&args.path);
            if !path.exists() {
                anyhow::bail!("file not found: {}", args.path);
            }

            let content = tokio::fs::read_to_string(&path).await
                .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", args.path))?;

            let lines: Vec<&str> = content.lines().collect();
            let total = lines.len();

            let offset = args.offset.unwrap_or(1).max(1);
            let limit = args.limit.unwrap_or(2000);
            let end = (offset + limit).min(total + 1);

            let anchored: Vec<String> = (offset..end)
                .map(|i| hashline::format_line(i, lines[i - 1]))
                .collect();

            Ok(ToolOutput::Custom(serde_json::json!({
                "header": hashline::file_header(&path),
                "lines": anchored,
                "total_lines": total,
            })))
        })
    }

    fn tool_name(&self) -> String { "read_file".into() }
    fn tool_description(&self) -> String {
        "Read a file and display lines with content-hash anchors (hashline format). \
         Use the anchors (e.g., `42:ab`) when editing with `hashline_patch`."
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
                "offset": {
                    "type": "integer",
                    "description": "Start line (1-indexed, default 1)",
                    "minimum": 1
                },
                "limit": {
                    "type": "integer",
                    "description": "Max lines (default 2000)",
                    "minimum": 1,
                    "maximum": 50000
                }
            },
            "required": ["path"]
        })
    }
}
