use codex_tools::{JsonSchema, ResponsesApiTool, ToolName, ToolSpec};
use serde::Deserialize;
use std::collections::BTreeMap;

use crate::function_tool::FunctionCallError;
use crate::tools::context::{boxed_tool_output, ToolInvocation, ToolPayload, FunctionToolOutput};
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;

#[derive(Deserialize)]
#[serde(tag = "type")]
pub enum EditOp {
    #[serde(rename = "set_line")]
    SetLine {
        anchor: String,
        new_text: String,
    },
    #[serde(rename = "replace_lines")]
    ReplaceLines {
        start_anchor: String,
        end_anchor: String,
        new_text: String,
    },
    #[serde(rename = "insert_after")]
    InsertAfter {
        anchor: String,
        text: String,
    },
    #[serde(rename = "replace")]
    Replace {
        old_text: String,
        new_text: String,
    },
}

#[derive(Deserialize)]
struct PatchArgs {
    /// File path
    pub path: String,
    /// Edit operations (applied bottom-up automatically)
    pub edits: Vec<EditOp>,
}

/// Parse an anchor string like "42:ab" → (line_num, expected_hash).
fn parse_anchor(anchor: &str) -> Result<(usize, String), String> {
    let parts: Vec<&str> = anchor.split(':').collect();
    if parts.len() != 2 {
        return Err(format!("invalid anchor format: {anchor:?} (expected LINE:HEX)"));
    }
    let line_num: usize = parts[0].parse()
        .map_err(|_| format!("invalid line number in anchor: {anchor:?}"))?;
    Ok((line_num, parts[1].to_string()))
}

/// Apply edits bottom-up (reverse order) so earlier line numbers stay valid.
fn apply_edits(content: &str, edits: &[EditOp]) -> Result<String, String> {
    let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
    if content.ends_with('\n') {
        lines.push(String::new());
    }

    // Collect edit results with their positions
    struct EditResult {
        start_line: usize, // 0-indexed, used for sorting
        apply: Box<dyn FnOnce(&mut Vec<String>) -> Result<(), String>>,
    }

    let mut results: Vec<EditResult> = Vec::new();

    for edit in edits {
        match edit {
            EditOp::SetLine { anchor, new_text } => {
                let (num, expected) = parse_anchor(anchor)?;
                let idx = num.checked_sub(1).ok_or_else(|| "line 0 in anchor".to_string())?;
                if idx >= lines.len() {
                    return Err(format!("line {num} out of range (file has {} lines)", lines.len()));
                }
                let actual = super::line_anchor(&lines[idx]);
                if actual != expected {
                    return Err(format!(
                        "anchor mismatch at line {num}: expected {expected}, got {actual}. \
                         File changed since read? Current: {}",
                        super::format_line(num, &lines[idx])
                    ));
                }
                let nt = new_text.clone();
                results.push(EditResult {
                    start_line: idx,
                    apply: Box::new(move |l| {
                        l[idx] = nt;
                        Ok(())
                    }),
                });
            }
            EditOp::ReplaceLines { start_anchor, end_anchor, new_text } => {
                let (start_num, start_hex) = parse_anchor(start_anchor)?;
                let (end_num, end_hex) = parse_anchor(end_anchor)?;
                if start_num > end_num || start_num == 0 {
                    return Err(format!("invalid line range: {start_anchor}..{end_anchor}"));
                }
                let start_idx = start_num - 1;
                let end_idx = end_num.min(lines.len());
                // Verify start/end anchors
                if start_idx < lines.len() {
                    let actual = super::line_anchor(&lines[start_idx]);
                    if actual != start_hex {
                        return Err(format!("anchor mismatch at line {start_num}: expected {start_hex}, got {actual}"));
                    }
                }
                if end_idx > 0 && end_idx <= lines.len() && end_idx > start_idx {
                    let actual = super::line_anchor(&lines[end_idx - 1]);
                    if actual != end_hex {
                        return Err(format!("anchor mismatch at line {end_num}: expected {end_hex}, got {actual}"));
                    }
                }
                let si = start_idx;
                let ei = end_idx;
                let nt = new_text.clone();
                results.push(EditResult {
                    start_line: si,
                    apply: Box::new(move |l| {
                        let new_lines: Vec<String> = nt.lines().map(|s| s.to_string()).collect();
                        l.splice(si..ei, new_lines);
                        Ok(())
                    }),
                });
            }
            EditOp::InsertAfter { anchor, text } => {
                let (num, expected) = parse_anchor(anchor)?;
                let idx = num.checked_sub(1).ok_or_else(|| "line 0 in anchor".to_string())?;
                if idx >= lines.len() {
                    return Err(format!("line {num} out of range"));
                }
                let actual = super::line_anchor(&lines[idx]);
                if actual != expected {
                    return Err(format!("anchor mismatch at line {num}: expected {expected}, got {actual}"));
                }
                let insert_at = idx + 1;
                let txt = text.clone();
                results.push(EditResult {
                    start_line: idx,
                    apply: Box::new(move |l| {
                        let insert_lines: Vec<String> = txt.lines().map(|s| s.to_string()).collect();
                        for (i, line) in insert_lines.into_iter().enumerate() {
                            l.insert(insert_at + i, line);
                        }
                        Ok(())
                    }),
                });
            }
            EditOp::Replace { old_text, new_text } => {
                let ot = old_text.clone();
                let nt = new_text.clone();
                results.push(EditResult {
                    start_line: 0,
                    apply: Box::new(move |l| {
                        let joined = l.join("\n");
                        let replaced = joined.replace(&ot, &nt);
                        *l = replaced.lines().map(|s| s.to_string()).collect();
                        if joined.ends_with('\n') {
                            l.push(String::new());
                        }
                        Ok(())
                    }),
                });
            }
        }
    }

    // Sort by start_line (ascending), then apply in reverse order (bottom-up)
    results.sort_by_key(|r| r.start_line);

    // Apply bottom-up to preserve line numbers
    for result in results.into_iter().rev() {
        (result.apply)(&mut lines)?;
    }

    // Remove trailing empty if file didn't end with newline originally
    let mut result = lines.join("\n");
    if !content.ends_with('\n') {
        while result.ends_with('\n') {
            result.pop();
        }
    }

    Ok(result)
}

pub struct HashlinePatchHandler;

impl ToolExecutor<ToolInvocation> for HashlinePatchHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("hashline_patch")
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function(ResponsesApiTool {
            name: "hashline_patch".into(),
            description: "Edit a file using hash-verified line anchors. \
             Operations: set_line (replace single line), replace_lines (range), \
             insert_after (insert after line), replace (fuzzy substring). \
             Edits validated against content hashes; stale-anchor errors include current hashes. \
             Pass anchors exactly as returned by read_file (e.g. `42:ab`). \
             Use read_file first to get current anchors."
                .into(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                BTreeMap::from([
                    ("path".to_string(), JsonSchema::string(Some("File path".into()))),
                    ("edits".to_string(), {
                        JsonSchema::array(
                            JsonSchema::object(
                                BTreeMap::from([
                                    ("type".to_string(), JsonSchema::string(Some("Edit operation type: set_line, replace_lines, insert_after, replace".into()))),
                                    ("anchor".to_string(), JsonSchema::string(Some("LINE:HEX anchor for set_line/insert_after".into()))),
                                    ("start_anchor".to_string(), JsonSchema::string(Some("Start anchor for replace_lines".into()))),
                                    ("end_anchor".to_string(), JsonSchema::string(Some("End anchor for replace_lines".into()))),
                                    ("new_text".to_string(), JsonSchema::string(Some("New text for set_line/replace_lines/replace".into()))),
                                    ("old_text".to_string(), JsonSchema::string(Some("Old text for replace operation".into()))),
                                    ("text".to_string(), JsonSchema::string(Some("Text for insert_after".into()))),
                                ]),
                                Some(vec!["type".to_string()]),
                                Some(false.into()),
                            ),
                            Some("Edit operations (applied bottom-up)".into()),
                        )
                    }),
                ]),
                Some(vec!["path".to_string(), "edits".to_string()]),
                Some(false.into()),
            ),
            output_schema: None,
        })
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let ToolPayload::Function { arguments } = invocation.payload else {
                return Err(FunctionCallError::RespondToModel(
                    "hashline_patch requires function-call payload".into(),
                ));
            };
            let args: PatchArgs = parse_arguments(&arguments)?;

            let path = std::path::PathBuf::from(&args.path);
            let content = tokio::fs::read_to_string(&path).await
                .map_err(|e| FunctionCallError::RespondToModel(
                    format!("cannot read {}: {e}", args.path)
                ))?;

            let new_content = apply_edits(&content, &args.edits)
                .map_err(|e| FunctionCallError::RespondToModel(e))?;

            // Only write if content actually changed
            if new_content != content {
                tokio::fs::write(&path, &new_content).await
                    .map_err(|e| FunctionCallError::RespondToModel(
                        format!("cannot write {}: {e}", args.path)
                    ))?;
            }

            let new_lines: Vec<&str> = new_content.lines().collect();
            let changed = args.edits.len();

            Ok(boxed_tool_output(FunctionToolOutput::from_text(
                serde_json::json!({
                    "status": "patched",
                    "path": args.path,
                    "edits_applied": changed,
                    "total_lines": new_lines.len(),
                }).to_string(),
                Some(true),
            )))
        })
    }
}

impl CoreToolRuntime for HashlinePatchHandler {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_line() {
        let content = "line1\nline2\nline3";
        let anchor = format!("2:{}", super::line_anchor("line2"));
        let result = apply_edits(content, &[EditOp::SetLine {
            anchor,
            new_text: "modified".into(),
        }]).unwrap();
        assert_eq!(result, "line1\nmodified\nline3");
    }

    #[test]
    fn test_replace_lines() {
        let content = "a\nb\nc\nd";
        let sa = format!("1:{}", super::line_anchor("a"));
        let ea = format!("2:{}", super::line_anchor("b"));
        let result = apply_edits(content, &[EditOp::ReplaceLines {
            start_anchor: sa,
            end_anchor: ea,
            new_text: "x\ny".into(),
        }]).unwrap();
        assert_eq!(result, "x\ny\nc\nd");
    }

    #[test]
    fn test_insert_after() {
        let content = "a\nb\nc";
        let anchor = format!("2:{}", super::line_anchor("b"));
        let result = apply_edits(content, &[EditOp::InsertAfter {
            anchor,
            text: "inserted".into(),
        }]).unwrap();
        assert_eq!(result, "a\nb\ninserted\nc");
    }

    #[test]
    fn test_replace_fuzzy() {
        let content = "hello world\nfoo bar";
        let result = apply_edits(content, &[EditOp::Replace {
            old_text: "world".into(),
            new_text: "universe".into(),
        }]).unwrap();
        assert_eq!(result, "hello universe\nfoo bar");
    }

    #[test]
    fn test_bottom_up_ordering() {
        let content = "a\nb\nc";
        let a1 = format!("1:{}", super::line_anchor("a"));
        let a3 = format!("3:{}", super::line_anchor("c"));
        let result = apply_edits(content, &[
            EditOp::InsertAfter { anchor: a1.clone(), text: "insert".into() },
            EditOp::SetLine { anchor: a3, new_text: "modified".into() },
        ]).unwrap();
        // Line 3 edit applied first (bottom-up), so line numbers stable
        assert_eq!(result, "a\ninsert\nb\nmodified");
    }

    #[test]
    fn test_anchor_mismatch_error() {
        let content = "line1\nline2";
        let result = apply_edits(content, &[EditOp::SetLine {
            anchor: "2:ff".into(),
            new_text: "x".into(),
        }]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("anchor mismatch"));
    }
}
