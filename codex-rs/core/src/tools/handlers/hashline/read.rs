use codex_tools::{JsonSchema, ResponsesApiTool, ToolName, ToolSpec};
use serde::Deserialize;
use std::collections::BTreeMap;

use crate::function_tool::FunctionCallError;
use crate::tools::context::{boxed_tool_output, ToolInvocation, ToolPayload, FunctionToolOutput};
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;

#[derive(Deserialize)]
struct ReadArgs {
    pub path: String,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

pub struct HashlineReadHandler;

impl ToolExecutor<ToolInvocation> for HashlineReadHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("hashline_read")
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function(ResponsesApiTool {
            name: "hashline_read".into(),
            description: "Read a file and return lines with hashline anchors (LINE:HASH|content).\
             Use these anchors with hashline_patch for safe editing.\
             Supports offset (1-based line number) and limit (max lines).\
             If the file is binary, returns base64-encoded content."
                .into(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                BTreeMap::from([
                    ("path".to_string(), JsonSchema::string(Some("File path".into()))),
                    ("offset".to_string(), JsonSchema::integer(Some("1-based starting line number".into()))),
                    ("limit".to_string(), JsonSchema::integer(Some("Maximum number of lines to return".into()))),
                ]),
                Some(vec!["path".to_string()]),
                Some(false.into()),
            ),
            output_schema: None,
        })
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let ToolPayload::Function { arguments } = invocation.payload else {
                return Err(FunctionCallError::RespondToModel(
                    "hashline_read requires function-call payload".into(),
                ));
            };
            let args: ReadArgs = parse_arguments(&arguments)?;

            let content = tokio::fs::read_to_string(&args.path).await
                .map_err(|e| FunctionCallError::RespondToModel(
                    format!("cannot read {}: {e}", args.path)
                ))?;

            let lines: Vec<&str> = content.lines().collect();
            let total = lines.len();

            let start = args.offset.unwrap_or(1).saturating_sub(1);
            let limit = args.limit.unwrap_or(total);
            let end = std::cmp::min(start + limit, total);

            let anchored: Vec<serde_json::Value> = lines[start..end].iter().enumerate().map(|(i, line)| {
                let abs_line = start + i + 1;
                serde_json::json!({
                    "line": abs_line,
                    "anchor": super::format_line(abs_line, line),
                    "content": line,
                })
            }).collect();

            Ok(boxed_tool_output(FunctionToolOutput::from_text(
                serde_json::json!({
                    "path": args.path,
                    "total_lines": total,
                    "start_line": start + 1,
                    "end_line": end,
                    "lines": anchored,
                }).to_string(),
                Some(true),
            )))
        })
    }
}

impl CoreToolRuntime for HashlineReadHandler {}
