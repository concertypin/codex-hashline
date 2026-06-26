//! Edit files using hash-verified line anchors (hashline patch).

use crate::tools::handlers::hashline;
use codex_tools::tool_executor::{CoreToolRuntime, ToolCallResult};
use codex_tools::JsonToolOutput;
use std::path::PathBuf;
use std::sync::Arc;

pub struct HashlinePatchHandler;

#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
enum EditOp {
    SetLine {
        anchor: String,
        new_text: String,
    },
    ReplaceLines {
        start_anchor: String,
        end_anchor: String,
        new_text: String,
    },
    InsertAfter {
        anchor: String,
        text: String,
    },
    Replace {
        old_text: String,
        new_text: String,
    },
}

#[derive(serde::Deserialize)]
struct PatchArgs {
    /// File path
    pub path: String,
    /// Edit operations (applied bottom-up automatically)
    pub edits: Vec<EditOp>,
}

/// Parse an anchor string like "42:ab" → (line_num, expected_hash).
fn parse_anchor(anchor: &str) -> anyhow::Result<(usize, String)> {
    let parts: Vec<&str> = anchor.split(':').collect();
    if parts.len() != 2 {
        anyhow::bail!("invalid anchor format: {anchor:?} (expected LINE:HEX)");
    }
    let line_num: usize = parts[0].parse()
        .map_err(|_| anyhow::anyhow!("invalid line number in anchor: {anchor:?}"))?;
    Ok((line_num, parts[1].to_string()))
}

/// Apply edits bottom-up (reverse order) so earlier line numbers stay valid.
fn apply_edits(content: &str, edits: &[EditOp]) -> anyhow::Result<String> {
    let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
    if content.ends_with('\n') {
        lines.push(String::new());
    }

    // Collect edit results with their positions
    struct EditResult {
        start_line: usize, // 0-indexed, used for sorting
        apply: Box<dyn FnOnce(&mut Vec<String>) -> anyhow::Result<()>>,
    }

    let mut results: Vec<EditResult> = Vec::new();

    for edit in edits {
        match edit {
            EditOp::SetLine { anchor, new_text } => {
                let (num, expected) = parse_anchor(anchor)?;
                let idx = num.checked_sub(1).ok_or_else(|| anyhow::anyhow!("line 0 in anchor"))?;
                if idx >= lines.len() {
                    anyhow::bail!("line {num} out of range (file has {} lines)", lines.len());
                }
                let actual = hashline::line_anchor(&lines[idx]);
                if actual != expected {
                    anyhow::bail!(
                        "anchor mismatch at line {num}: expected {expected}, got {actual}. \
                         File changed since read? Current: {}",
                        hashline::format_line(num, &lines[idx])
                    );
                }
                results.push(EditResult {
                    start_line: idx,
                    apply: Box::new(move |l| {
                        l[idx] = new_text.clone();
                        Ok(())
                    }),
                });
            }
            EditOp::ReplaceLines { start_anchor, end_anchor, new_text } => {
                let (start_num, start_hex) = parse_anchor(start_anchor)?;
                let (end_num, end_hex) = parse_anchor(end_anchor)?;
                if start_num > end_num || start_num == 0 {
                    anyhow::bail!("invalid line range: {start_anchor}..{end_anchor}");
                }
                let start_idx = start_num - 1;
                let end_idx = end_num.min(lines.len());
                // Verify start/end anchors
                if start_idx < lines.len() {
                    let actual = hashline::line_anchor(&lines[start_idx]);
                    if actual != start_hex {
                        anyhow::bail!("anchor mismatch at line {start_num}: expected {start_hex}, got {actual}");
                    }
                }
                if end_idx > 0 && end_idx <= lines.len() && end_idx > start_idx {
                    let actual = hashline::line_anchor(&lines[end_idx - 1]);
                    if actual != end_hex {
                        anyhow::bail!("anchor mismatch at line {end_num}: expected {end_hex}, got {actual}");
                    }
                }
                results.push(EditResult {
                    start_line: start_idx,
                    apply: Box::new(move |l| {
                        let new_lines: Vec<String> = new_text.lines().map(|s| s.to_string()).collect();
                        l.splice(start_idx..end_idx, new_lines);
                        Ok(())
                    }),
                });
            }
            EditOp::InsertAfter { anchor, text } => {
                let (num, expected) = parse_anchor(anchor)?;
                let idx = num.checked_sub(1).ok_or_else(|| anyhow::anyhow!("line 0 in anchor"))?;
                if idx >= lines.len() {
                    anyhow::bail!("line {num} out of range");
                }
                let actual = hashline::line_anchor(&lines[idx]);
                if actual != expected {
                    anyhow::bail!("anchor mismatch at line {num}: expected {expected}, got {actual}");
                }
                results.push(EditResult {
                    start_line: idx,
                    apply: Box::new(move |l| {
                        let insert_lines: Vec<String> = text.lines().map(|s| s.to_string()).collect();
                        let insert_at = idx + 1;
                        for (i, line) in insert_lines.into_iter().enumerate() {
                            l.insert(insert_at + i, line);
                        }
                        Ok(())
                    }),
                });
            }
            EditOp::Replace { old_text, new_text } => {
                // Fuzzy fallback: find first occurrence
                if let Some(pos) = content.find(old_text) {
                    let prefix = &content[..pos];
                    let prefix_lines = prefix.matches('\n').count();
                    results.push(EditResult {
                        start_line: prefix_lines,
                        apply: Box::new({
                            let old = old_text.clone();
                            let new = new_text.clone();
                            move |l| {
                                let joined = l.join("\n");
                                if let Some(p) = joined.find(&old) {
                                    let before: String = joined[..p].to_string();
                                    let after: String = joined[p + old.len()..].to_string();
                                    let new_joined = format!("{before}{new}{after}");
                                    *l = new_joined.lines().map(|s| s.to_string()).collect();
                                    if new_joined.ends_with('\n') {
                                        l.push(String::new());
                                    }
                                }
                                Ok(())
                            }
                        }),
                    });
                } else {
                    anyhow::bail!("replace: old_text not found in file");
                }
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

impl CoreToolRuntime for HashlinePatchHandler {
    fn call(&self, args: serde_json::Value, _context: Arc<dyn codex_tools::tool_executor::ToolCallContext>) -> ToolCallResult {
        Box::pin(async move {
            let args: PatchArgs = serde_json::from_value(args)
                .map_err(|e| anyhow::anyhow!("invalid patch args: {e}"))?;

            let path = PathBuf::from(&args.path);
            let content = tokio::fs::read_to_string(&path).await
                .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", args.path))?;

            let new_content = apply_edits(&content, &args.edits)?;

            // Only write if content actually changed
            if new_content != content {
                tokio::fs::write(&path, &new_content).await
                    .map_err(|e| anyhow::anyhow!("cannot write {}: {e}", args.path))?;
            }

            let new_lines: Vec<&str> = new_content.lines().collect();
            let changed = args.edits.len();

            Ok(Box::new(JsonToolOutput::new(serde_json::json!({
                "status": "patched",
                "path": args.path,
                "edits_applied": changed,
                "total_lines": new_lines.len(),
            }))))
        })
    }

    fn tool_name(&self) -> String { "hashline_patch".into() }
    fn tool_description(&self) -> String {
        "Edit a file using hash-verified line anchors. \
         Operations: set_line (replace single line), replace_lines (range), \
         insert_after (insert after line), replace (fuzzy substring). \
         Edits validated against content hashes; stale-anchor errors include current hashes. \
         Pass anchors exactly as returned by read_file (e.g. `42:ab`). \
         Use read_file first to get current anchors."
            .into()
    }
    fn tool_input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path"
                },
                "edits": {
                    "type": "array",
                    "description": "Edit operations (applied bottom-up)",
                    "items": {
                        "oneOf": [
                            {
                                "type": "object",
                                "properties": {
                                    "type": { "type": "string", "const": "set_line" },
                                    "anchor": { "type": "string", "description": "Line anchor (LINE:HEX)" },
                                    "new_text": { "type": "string" }
                                },
                                "required": ["type", "anchor", "new_text"]
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "type": { "type": "string", "const": "replace_lines" },
                                    "start_anchor": { "type": "string" },
                                    "end_anchor": { "type": "string" },
                                    "new_text": { "type": "string" }
                                },
                                "required": ["type", "start_anchor", "end_anchor", "new_text"]
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "type": { "type": "string", "const": "insert_after" },
                                    "anchor": { "type": "string" },
                                    "text": { "type": "string" }
                                },
                                "required": ["type", "anchor", "text"]
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "type": { "type": "string", "const": "replace" },
                                    "old_text": { "type": "string" },
                                    "new_text": { "type": "string" }
                                },
                                "required": ["type", "old_text", "new_text"]
                            }
                        ]
                    }
                }
            },
            "required": ["path", "edits"]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_line() {
        let content = "line1\nline2\nline3";
        let anchor = format!("2:{}", hashline::line_anchor("line2"));
        let result = apply_edits(content, &[EditOp::SetLine {
            anchor,
            new_text: "modified".into(),
        }]).unwrap();
        assert_eq!(result, "line1\nmodified\nline3");
    }

    #[test]
    fn test_replace_lines() {
        let content = "a\nb\nc\nd";
        let sa = format!("1:{}", hashline::line_anchor("a"));
        let ea = format!("2:{}", hashline::line_anchor("b"));
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
        let anchor = format!("2:{}", hashline::line_anchor("b"));
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
        let a1 = format!("1:{}", hashline::line_anchor("a"));
        let a3 = format!("3:{}", hashline::line_anchor("c"));
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
        assert!(result.unwrap_err().to_string().contains("anchor mismatch"));
    }
}
