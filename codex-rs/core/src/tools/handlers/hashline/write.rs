use codex_tools::{JsonSchema, ResponsesApiTool, ToolName, ToolSpec};
use serde::Deserialize;
use std::collections::BTreeMap;

use crate::function_tool::FunctionCallError;
use crate::tools::context::{boxed_tool_output, ToolInvocation, ToolPayload, FunctionToolOutput};
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;

#[derive(Deserialize)]
struct WriteArgs {
    pub path: String,
    pub content: String,
}

pub struct HashlineWriteHandler;

impl ToolExecutor<ToolInvocation> for HashlineWriteHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("hashline_write")
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function(ResponsesApiTool {
            name: "hashline_write".into(),
            description: "Write a file (creates parent dirs if needed).\
             Requires a read-only dependency: if you already read the file, pass its path to this tool.\
             Warning: overwrites existing files."
                .into(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                BTreeMap::from([
                    ("path".to_string(), JsonSchema::string(Some("File path".into()))),
                    ("content".to_string(), JsonSchema::string(Some("File content".into()))),
                ]),
                Some(vec!["path".to_string(), "content".to_string()]),
                Some(false.into()),
            ),
            output_schema: None,
        })
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let ToolPayload::Function { arguments } = invocation.payload else {
                return Err(FunctionCallError::RespondToModel(
                    "hashline_write requires function-call payload".into(),
                ));
            };
            let args: WriteArgs = parse_arguments(&arguments)?;

            let path = std::path::PathBuf::from(&args.path);
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await
                    .map_err(|e| FunctionCallError::RespondToModel(
                        format!("cannot create dirs for {}: {e}", args.path)
                    ))?;
            }

            tokio::fs::write(&path, &args.content).await
                .map_err(|e| FunctionCallError::RespondToModel(
                    format!("cannot write {}: {e}", args.path)
                ))?;

            Ok(boxed_tool_output(FunctionToolOutput::from_text(
                serde_json::json!({
                    "status": "written",
                    "path": args.path,
                    "size": args.content.len(),
                }).to_string(),
                Some(true),
            )))
        })
    }
}

impl CoreToolRuntime for HashlineWriteHandler {}
