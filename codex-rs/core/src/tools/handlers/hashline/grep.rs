use codex_tools::{JsonSchema, ResponsesApiTool, ToolName, ToolSpec};
use serde::Deserialize;
use std::collections::BTreeMap;

use crate::function_tool::FunctionCallError;
use crate::tools::context::{boxed_tool_output, ToolInvocation, ToolPayload, FunctionToolOutput};
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;

#[derive(Deserialize)]
struct GrepArgs {
    pub pattern: String,
    pub path: Option<String>,
    pub glob: Option<String>,
    pub context_lines: Option<usize>,
    pub max_matches: Option<usize>,
}

pub struct HashlineGrepHandler;

impl ToolExecutor<ToolInvocation> for HashlineGrepHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("hashline_grep")
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function(ResponsesApiTool {
            name: "hashline_grep".into(),
            description: "Search files using ripgrep. \
             Returns matches with hashline anchors (LINE:HASH|content). \
             Results can be used directly with read_file offset or hashline_patch anchors. \
             Requires `rg` on PATH."
                .into(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                BTreeMap::from([
                    ("pattern".to_string(), JsonSchema::string(Some("Regex pattern to search for".into()))),
                    ("path".to_string(), JsonSchema::string(Some("File or directory to search (default: current dir)".into()))),
                    ("glob".to_string(), JsonSchema::string(Some("Glob filter (e.g., '*.rs')".into()))),
                    ("context_lines".to_string(), JsonSchema::integer(Some("Lines of context before/after each match".into()))),
                    ("max_matches".to_string(), JsonSchema::integer(Some("Maximum number of matches (default: 100)".into()))),
                ]),
                Some(vec!["pattern".to_string()]),
                Some(false.into()),
            ),
            output_schema: None,
        })
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let ToolPayload::Function { arguments } = invocation.payload else {
                return Err(FunctionCallError::RespondToModel(
                    "hashline_grep requires function-call payload".into(),
                ));
            };
            let args: GrepArgs = parse_arguments(&arguments)?;

            let mut cmd = tokio::process::Command::new("rg");
            cmd.arg("--line-number")
                .arg("--no-heading")
                .arg("--color")
                .arg("never")
                .arg("-U") // --multiline
                .arg("--multiline-dotall")
                .arg("-E")
                .arg("auto");

            if let Some(ctx) = args.context_lines {
                cmd.arg("-C").arg(ctx.to_string());
            }
            if let Some(max) = args.max_matches {
                cmd.arg("-m").arg(max.to_string());
            }
            if let Some(glob) = &args.glob {
                cmd.arg("--glob").arg(glob);
            }

            cmd.arg(&args.pattern);
            if let Some(path) = &args.path {
                cmd.arg(path);
            }

            let output = cmd.output().await
                .map_err(|e| {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        FunctionCallError::RespondToModel(
                            "rg (ripgrep) not found on PATH. Install ripgrep to use this tool.".into()
                        )
                    } else {
                        FunctionCallError::RespondToModel(format!("rg failed: {e}"))
                    }
                })?;

            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut results: Vec<serde_json::Value> = Vec::new();

            for line in stdout.lines() {
                // rg output: filename:line_num:content
                if let Some((_file, line_num_str, content)) = line.split_once(':').and_then(|(f, rest)| {
                    rest.split_once(':').map(|(ln, c)| (f, ln, c))
                }) {
                    if let Ok(line_num) = line_num_str.parse::<usize>() {
                        let anchored = super::format_line(line_num, content);
                        results.push(serde_json::json!({
                            "file": line.split(':').next().unwrap_or("?"),
                            "line": line_num,
                            "anchor": anchored,
                        }));
                    }
                }
            }

            Ok(boxed_tool_output(FunctionToolOutput::from_text(
                serde_json::json!({
                    "matches": results,
                    "total": results.len(),
                    "pattern": args.pattern,
                }).to_string(),
                Some(true),
            )))
        })
    }
}

impl CoreToolRuntime for HashlineGrepHandler {}
