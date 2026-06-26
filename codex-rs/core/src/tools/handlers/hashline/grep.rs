//! Search files with ripgrep and return hashline-anchored results.

use crate::tools::handlers::hashline;
use codex_tools::tool_executor::{CoreToolRuntime, ToolCallResult};
use codex_tools::JsonToolOutput;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::process::Command;

pub struct HashlineGrepHandler;

#[derive(serde::Deserialize)]
struct GrepArgs {
    /// Regex pattern
    pub pattern: String,
    /// File or directory path
    pub path: Option<String>,
    /// Glob filter (e.g., *.js, **/*.ts)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub glob: Option<String>,
    /// Case-insensitive search
    #[serde(default)]
    pub i: bool,
    /// Context lines before match
    #[serde(default)]
    pub pre: usize,
    /// Context lines after match
    #[serde(default)]
    pub post: usize,
    /// Max matches (default 100)
    pub limit: Option<usize>,
}

impl CoreToolRuntime for HashlineGrepHandler {
    fn call(&self, args: serde_json::Value, _context: Arc<dyn codex_tools::tool_executor::ToolCallContext>) -> ToolCallResult {
        Box::pin(async move {
            let args: GrepArgs = serde_json::from_value(args)
                .map_err(|e| anyhow::anyhow!("invalid grep args: {e}"))?;

            // Build rg command
            let mut cmd = Command::new("rg");
            cmd.arg("--line-number")
                .arg("--no-heading")
                .arg("--color")
                .arg("never");

            if let Some(glob) = &args.glob {
                cmd.arg("--glob").arg(glob);
            }
            if args.i {
                cmd.arg("-i");
            }
            if args.pre > 0 {
                cmd.arg("-B").arg(args.pre.to_string());
            }
            if args.post > 0 {
                cmd.arg("-A").arg(args.post.to_string());
            }

            let limit = args.limit.unwrap_or(100);
            cmd.arg("-m").arg(limit.to_string());

            cmd.arg(&args.pattern);

            if let Some(path) = &args.path {
                cmd.arg(path);
            }

            let output = cmd.output().await
                .map_err(|e| {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        anyhow::anyhow!("rg (ripgrep) not found on PATH. Install ripgrep to use this tool.")
                    } else {
                        anyhow::anyhow!("rg failed: {e}")
                    }
                })?;

            if !output.status.success() && !output.stdout.is_empty() {
                // rg exits 1 when no matches — that's OK
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut results: Vec<serde_json::Value> = Vec::new();

            for line in stdout.lines() {
                // rg output: filename:line_num:content
                if let Some((rest, content)) = line.split_once(':').and_then(|(f, rest)| {
                    rest.split_once(':').map(|(ln, c)| (f, ln, c))
                }) {
                    if let Ok(line_num) = rest.parse::<usize>() {
                        let anchored = hashline::format_line(line_num, content);
                        results.push(serde_json::json!({
                            "file": line.split(':').next().unwrap_or("?"),
                            "line": line_num,
                            "anchor": anchored,
                        }));
                    }
                }
            }

            Ok(Box::new(JsonToolOutput::new(serde_json::json!({
                "matches": results,
                "total": results.len(),
                "pattern": args.pattern,
            }))))
        })
    }

    fn tool_name(&self) -> String { "grep".into() }
    fn tool_description(&self) -> String {
        "Search files using ripgrep. \
         Returns matches with hashline anchors (LINE:HASH|content). \
         Results can be used directly with read_file offset or hashline_patch anchors. \
         Requires `rg` on PATH."
            .into()
    }
    fn tool_input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regex pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "File or directory to search (default: current dir)"
                },
                "glob": {
                    "type": "string",
                    "description": "Glob filter (e.g., *.js, **/*.ts, src/**)"
                },
                "i": {
                    "type": "boolean",
                    "description": "Case-insensitive search (default false)"
                },
                "pre": {
                    "type": "integer",
                    "description": "Context lines before match",
                    "minimum": 0,
                    "maximum": 20,
                    "default": 0
                },
                "post": {
                    "type": "integer",
                    "description": "Context lines after match",
                    "minimum": 0,
                    "maximum": 20,
                    "default": 0
                },
                "limit": {
                    "type": "integer",
                    "description": "Max matches (default 100)",
                    "minimum": 1,
                    "maximum": 5000,
                    "default": 100
                }
            },
            "required": ["pattern"]
        })
    }
}
