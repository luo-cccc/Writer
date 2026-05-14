//! Search tools: `grep_files` for code search
//!
//! These tools provide powerful code search capabilities within the workspace,
//! similar to ripgrep/grep functionality.

use super::spec::{
    ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec, optional_bool, optional_str,
    optional_u64, required_str,
};
use async_trait::async_trait;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

/// Maximum number of results to return to avoid overwhelming output
const MAX_RESULTS: usize = 100;

/// Maximum file size to search (skip large binaries)
const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024; // 10MB

/// Result of a grep match
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepMatch {
    pub file: String,
    pub line_number: usize,
    pub line: String,
    pub context_before: Vec<String>,
    pub context_after: Vec<String>,
}

/// Tool for searching files using regex patterns
pub struct GrepFilesTool;

#[async_trait]
impl ToolSpec for GrepFilesTool {
    fn name(&self) -> &'static str {
        "grep_files"
    }

    fn description(&self) -> &'static str {
        "Search for a regex pattern in workspace files. Use this instead of `grep -r`, `rg`, or `find ... -exec grep` in `exec_shell` — pure-Rust, faster, and respects `.gitignore`. Returns matching lines with context (default: 2 lines before/after each match)."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regular expression pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "Directory or file to search (relative to workspace, default: .)"
                },
                "include": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Glob patterns for files to include (e.g., ['*.rs', '*.ts'])"
                },
                "exclude": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Glob patterns for files to exclude (e.g., ['*.min.js', 'node_modules/*'])"
                },
                "context_lines": {
                    "type": "integer",
                    "description": "Number of context lines before and after each match (default: 2)"
                },
                "case_insensitive": {
                    "type": "boolean",
                    "description": "Whether to perform case-insensitive matching (default: false)"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 100)"
                }
            },
            "required": ["pattern"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly, ToolCapability::Sandboxable]
    }

    fn supports_parallel(&self) -> bool {
        true
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let pattern_str = required_str(&input, "pattern")?;
        let path_str = optional_str(&input, "path").unwrap_or(".");
        let context_lines =
            usize::try_from(optional_u64(&input, "context_lines", 2)).unwrap_or(usize::MAX);
        let case_insensitive = optional_bool(&input, "case_insensitive", false);
        let max_results = usize::try_from(optional_u64(&input, "max_results", MAX_RESULTS as u64))
            .unwrap_or(MAX_RESULTS);

        // Parse include patterns
        let include_patterns: Vec<String> = input
            .get("include")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        // Parse exclude patterns
        let exclude_patterns: Vec<String> =
            input.get("exclude").and_then(|v| v.as_array()).map_or_else(
                || {
                    // Default exclusions for common non-code directories
                    vec![
                        "node_modules/**".to_string(),
                        ".git/**".to_string(),
                        "target/**".to_string(),
                        "*.min.js".to_string(),
                        "*.min.css".to_string(),
                        "dist/**".to_string(),
                        "build/**".to_string(),
                        "__pycache__/**".to_string(),
                        ".venv/**".to_string(),
                        "venv/**".to_string(),
                    ]
                },
                |arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                },
            );

        // Build regex
        let regex_pattern = if case_insensitive {
            format!("(?i){pattern_str}")
        } else {
            pattern_str.to_string()
        };

        let regex = Regex::new(&regex_pattern)
            .map_err(|e| ToolError::invalid_input(format!("Invalid regex pattern: {e}")))?;

        // Resolve search path
        let search_path = context.resolve_path(path_str)?;

        let include_matcher = GlobMatcher::new(&include_patterns)?;
        let exclude_matcher = GlobMatcher::new(&exclude_patterns)?;
        let results = search_path_streaming(
            &search_path,
            &context.workspace,
            &regex,
            &include_matcher,
            &exclude_matcher,
            context_lines,
            max_results,
        )?;

        let matches_json: Vec<Value> = results
            .matches
            .iter()
            .map(|item| grep_match_to_json(item, context_lines))
            .collect();

        // Build result. When context_lines == 1, return the single context
        // line as a string instead of a one-item array. That keeps the common
        // "show just the adjacent line" case easy for model callers to read.
        let result = json!({
            "matches": matches_json,
            "total_matches": results.total_matches,
            "files_searched": results.files_searched,
            "truncated": results.truncated,
        });

        ToolResult::json(&result).map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

fn grep_match_to_json(item: &GrepMatch, context_lines: usize) -> Value {
    if context_lines == 1 {
        json!({
            "file": item.file,
            "line_number": item.line_number,
            "line": item.line,
            "context_before": item.context_before.first().cloned().unwrap_or_default(),
            "context_after": item.context_after.first().cloned().unwrap_or_default(),
        })
    } else {
        json!(item)
    }
}

#[derive(Debug)]
struct GrepSearchResult {
    matches: Vec<GrepMatch>,
    files_searched: usize,
    total_matches: usize,
    truncated: bool,
}

fn search_path_streaming(
    root: &Path,
    workspace: &Path,
    regex: &Regex,
    include_matcher: &GlobMatcher,
    exclude_matcher: &GlobMatcher,
    context_lines: usize,
    max_results: usize,
) -> Result<GrepSearchResult, ToolError> {
    let mut result = GrepSearchResult {
        matches: Vec::new(),
        files_searched: 0,
        total_matches: 0,
        truncated: false,
    };
    let workspace_canonical = workspace.canonicalize().ok();

    if root.is_file() {
        let relative_path = normalized_file_name(root);
        if !exclude_matcher.is_match(&relative_path)
            && (include_matcher.is_empty() || include_matcher.is_match(&relative_path))
        {
            search_one_file(
                root,
                workspace,
                workspace_canonical.as_deref(),
                regex,
                context_lines,
                max_results,
                &mut result,
            );
        }
        return Ok(result);
    }

    if !root.exists() {
        return Err(ToolError::invalid_input(format!(
            "Search path does not exist: {}",
            root.display()
        )));
    }

    let mut builder = WalkBuilder::new(root);
    builder.hidden(false).follow_links(false).require_git(false);
    let filter_root = root.to_path_buf();
    let filter_excludes = exclude_matcher.clone();
    builder.filter_entry(move |entry| {
        if entry.depth() == 0 {
            return true;
        }
        let relative_path = normalized_relative_path(&filter_root, entry.path());
        !filter_excludes.is_match(&relative_path)
    });

    let walker = builder.build();
    for entry in walker {
        if result.matches.len() >= max_results {
            result.truncated = true;
            break;
        }

        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() || !file_type.is_file() {
            continue;
        }

        let relative_path = normalized_relative_path(root, entry.path());
        if include_matcher.is_empty() || include_matcher.is_match(&relative_path) {
            search_one_file(
                entry.path(),
                workspace,
                workspace_canonical.as_deref(),
                regex,
                context_lines,
                max_results,
                &mut result,
            );
        }
    }

    Ok(result)
}

fn search_one_file(
    file_path: &Path,
    workspace: &Path,
    workspace_canonical: Option<&Path>,
    regex: &Regex,
    context_lines: usize,
    max_results: usize,
    result: &mut GrepSearchResult,
) {
    if result.matches.len() >= max_results {
        result.truncated = true;
        return;
    }

    if let Ok(metadata) = fs::metadata(file_path)
        && metadata.len() > MAX_FILE_SIZE
    {
        return;
    }

    let Ok(file_content) = fs::read_to_string(file_path) else {
        return;
    };

    result.files_searched += 1;
    let lines: Vec<&str> = file_content.lines().collect();

    for (line_idx, line) in lines.iter().enumerate() {
        if !regex.is_match(line) {
            continue;
        }

        result.total_matches += 1;

        let context_before: Vec<String> = (line_idx.saturating_sub(context_lines)..line_idx)
            .filter_map(|i| lines.get(i).map(|s| (*s).to_string()))
            .collect();

        let context_after_end = line_idx
            .saturating_add(context_lines)
            .min(lines.len().saturating_sub(1));
        let context_after: Vec<String> = ((line_idx + 1)..=context_after_end)
            .filter_map(|i| lines.get(i).map(|s| (*s).to_string()))
            .collect();

        let relative_path =
            normalized_relative_path_with_fallback(workspace, workspace_canonical, file_path);

        result.matches.push(GrepMatch {
            file: relative_path,
            line_number: line_idx + 1,
            line: (*line).to_string(),
            context_before,
            context_after,
        });

        if result.matches.len() >= max_results {
            result.truncated = true;
            return;
        }
    }
}

fn normalized_file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|| path.to_string_lossy().replace('\\', "/"))
}

fn normalized_relative_path(root: &Path, path: &Path) -> String {
    normalized_relative_path_with_fallback(root, None, path)
}

fn normalized_relative_path_with_fallback(
    root: &Path,
    canonical_root: Option<&Path>,
    path: &Path,
) -> String {
    if let Some(relative_path) = try_normalized_relative_path(root, path) {
        return relative_path;
    }

    if let Some(canonical_root) = canonical_root
        && let Some(relative_path) = try_normalized_relative_path(canonical_root, path)
    {
        return relative_path;
    }

    normalized_file_name_or_path(path)
}

fn try_normalized_relative_path(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).unwrap_or(path);
    if relative == path {
        return None;
    }
    if relative.as_os_str().is_empty() {
        Some(normalized_file_name(path))
    } else {
        Some(relative.to_string_lossy().replace('\\', "/"))
    }
}

fn normalized_file_name_or_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[derive(Clone, Debug)]
pub(crate) struct GlobMatcher {
    set: GlobSet,
    is_empty: bool,
}

impl GlobMatcher {
    pub(crate) fn new(patterns: &[String]) -> Result<Self, ToolError> {
        let mut builder = GlobSetBuilder::new();
        let mut count = 0;

        for pattern in patterns {
            let pattern = pattern.trim();
            if pattern.is_empty() {
                continue;
            }

            let normalized = normalize_glob_pattern(pattern);
            let glob = GlobBuilder::new(&normalized)
                .literal_separator(true)
                .build()
                .map_err(|e| {
                    ToolError::invalid_input(format!("Invalid glob pattern {pattern:?}: {e}"))
                })?;
            builder.add(glob);
            count += 1;
        }

        let set = builder
            .build()
            .map_err(|e| ToolError::invalid_input(format!("Invalid glob patterns: {e}")))?;

        Ok(Self {
            set,
            is_empty: count == 0,
        })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.is_empty
    }

    pub(crate) fn is_match(&self, path: &str) -> bool {
        if self.is_empty {
            return false;
        }
        self.set.is_match(path.replace('\\', "/"))
    }
}

fn normalize_glob_pattern(pattern: &str) -> String {
    let pattern = pattern.replace('\\', "/");
    if pattern.contains('/') {
        if let Some(prefix) = pattern.strip_suffix("/*") {
            return format!("{prefix}/**");
        }
        return pattern;
    }
    format!("**/{pattern}")
}

// === Unit Tests ===

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::{Value, json};
    use tempfile::tempdir;

    use crate::tools::spec::{ApprovalRequirement, ToolContext, ToolSpec};

    use super::{GlobMatcher, GrepFilesTool};

    fn matches_glob(path: &str, pattern: &str) -> bool {
        GlobMatcher::new(&[pattern.to_string()]).is_ok_and(|matcher| matcher.is_match(path))
    }

    #[test]
    fn test_matches_glob_star() {
        assert!(matches_glob("test.rs", "*.rs"));
        assert!(matches_glob("foo.rs", "*.rs"));
        assert!(!matches_glob("test.ts", "*.rs"));
        assert!(!matches_glob("test.rs.bak", "*.rs"));
    }

    #[test]
    fn test_matches_glob_question() {
        assert!(matches_glob("test.rs", "test.??"));
        assert!(!matches_glob("test.rs", "test.?"));
    }

    #[test]
    fn test_matches_glob_double_star() {
        assert!(matches_glob("src/main.rs", "src/**"));
        assert!(matches_glob("src/lib/mod.rs", "src/**"));
        assert!(matches_glob("node_modules/pkg/index.js", "node_modules/*"));
    }

    #[test]
    fn test_matches_glob_path() {
        assert!(matches_glob("src/main.rs", "src/*.rs"));
        assert!(!matches_glob("lib/main.rs", "src/*.rs"));
    }

    #[test]
    fn test_matches_glob_normalizes_windows_separators() {
        assert!(matches_glob("target\\debug\\app.exe", "target/**"));
        assert!(matches_glob("src\\main.rs", "src/*.rs"));
        assert!(matches_glob("src/main.rs", "src\\*.rs"));
    }

    /// Regression for #249: byte-index slicing panics on multi-byte
    /// characters inside filenames like `dialogue_line__冰糖.mp3`.
    #[test]
    fn test_matches_glob_unicode_filename() {
        let filename = "dialogue_line__冰糖.mp3";
        // The filename should match *.mp3 without panicking.
        assert!(matches_glob(filename, "*.mp3"));
        // Asterisk matching against multi-byte characters must succeed.
        assert!(matches_glob(filename, "dialogue_line__*"));
        // Literal multi-byte characters inside the pattern must match.
        assert!(matches_glob(filename, "*冰糖*"));
        // Non-matching pattern must not panic either.
        assert!(!matches_glob(filename, "nonexistent*"));
    }

    #[tokio::test]
    async fn test_grep_files_basic() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        // Create test files
        fs::write(
            tmp.path().join("test.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .expect("write");
        fs::write(
            tmp.path().join("lib.rs"),
            "pub fn hello() {}\npub fn world() {}\n",
        )
        .expect("write");

        let tool = GrepFilesTool;
        let result = tool
            .execute(json!({"pattern": "fn"}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        assert!(result.content.contains("main"));
        assert!(result.content.contains("hello"));
    }

    #[tokio::test]
    async fn test_grep_files_with_context() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        fs::write(
            tmp.path().join("test.txt"),
            "line1\nline2\nMATCH\nline4\nline5\n",
        )
        .expect("write");

        let tool = GrepFilesTool;
        let result = tool
            .execute(json!({"pattern": "MATCH", "context_lines": 1}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        assert!(result.content.contains("line2")); // context before
        assert!(result.content.contains("line4")); // context after

        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let matches = parsed["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0]["context_before"], "line2");
        assert_eq!(matches[0]["context_after"], "line4");
        assert!(matches[0]["context_before"].is_string());
        assert!(matches[0]["context_after"].is_string());
    }

    #[tokio::test]
    async fn test_grep_files_multi_line_context_remains_arrays() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        fs::write(tmp.path().join("test.txt"), "a\nb\nMATCH\nd\ne\n").expect("write");

        let tool = GrepFilesTool;
        let result = tool
            .execute(json!({"pattern": "MATCH", "context_lines": 2}), &ctx)
            .await
            .expect("execute");

        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let matches = parsed["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0]["context_before"], json!(["a", "b"]));
        assert_eq!(matches[0]["context_after"], json!(["d", "e"]));
    }

    #[tokio::test]
    async fn test_grep_files_case_insensitive() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        fs::write(
            tmp.path().join("test.txt"),
            "Hello World\nHELLO WORLD\nhello world\n",
        )
        .expect("write");

        let tool = GrepFilesTool;
        let result = tool
            .execute(json!({"pattern": "hello", "case_insensitive": true}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        // Should find all 3 lines
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["total_matches"].as_u64().unwrap(), 3);
    }

    #[tokio::test]
    async fn test_grep_files_include_filter() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        fs::write(tmp.path().join("test.rs"), "fn test() {}\n").expect("write");
        fs::write(tmp.path().join("test.js"), "function test() {}\n").expect("write");

        let tool = GrepFilesTool;
        let result = tool
            .execute(json!({"pattern": "test", "include": ["*.rs"]}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        // Should only match .rs file
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        let matches = parsed["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
        let file = matches[0]["file"].as_str().unwrap();
        assert!(
            file.rsplit('.')
                .next()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("rs"))
        );
    }

    #[tokio::test]
    async fn test_grep_files_respects_gitignore() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        fs::write(tmp.path().join(".gitignore"), "ignored.txt\n").expect("write gitignore");
        fs::write(tmp.path().join("ignored.txt"), "NEEDLE\n").expect("write ignored");
        fs::write(tmp.path().join("keep.txt"), "NEEDLE\n").expect("write keep");

        let tool = GrepFilesTool;
        let result = tool
            .execute(json!({"pattern": "NEEDLE"}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["total_matches"].as_u64().unwrap(), 1);
        assert!(result.content.contains("keep.txt"));
        assert!(!result.content.contains("ignored.txt"));
    }

    #[tokio::test]
    async fn test_grep_files_exclude_filter_matches_nested_paths() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        fs::create_dir_all(tmp.path().join("target").join("debug")).expect("mkdir target");
        fs::write(
            tmp.path().join("target").join("debug").join("needle.txt"),
            "NEEDLE\n",
        )
        .expect("write target");
        fs::write(tmp.path().join("needle.txt"), "NEEDLE\n").expect("write root");

        let tool = GrepFilesTool;
        let result = tool
            .execute(json!({"pattern": "NEEDLE"}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["total_matches"].as_u64().unwrap(), 1);
        let matches = parsed["matches"].as_array().unwrap();
        assert_eq!(matches[0]["file"], "needle.txt");
    }

    #[tokio::test]
    async fn test_grep_files_stops_after_max_results() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        for idx in 0..10 {
            fs::write(tmp.path().join(format!("needle_{idx}.txt")), "NEEDLE\n")
                .expect("write file");
        }

        let tool = GrepFilesTool;
        let result = tool
            .execute(json!({"pattern": "NEEDLE", "max_results": 1}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["matches"].as_array().unwrap().len(), 1);
        assert_eq!(parsed["total_matches"].as_u64().unwrap(), 1);
        assert!(parsed["truncated"].as_bool().unwrap());
        assert_eq!(parsed["files_searched"].as_u64().unwrap(), 1);
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_grep_files_does_not_follow_symlinked_files() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path().join("workspace");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&root).expect("mkdir workspace");
        std::fs::create_dir_all(&outside).expect("mkdir outside");
        let outside_file = outside.join("secret.txt");
        fs::write(&outside_file, "NEEDLE\n").expect("write outside");
        std::os::unix::fs::symlink(&outside_file, root.join("secret.txt")).expect("symlink");

        let ctx = ToolContext::new(root);
        let tool = GrepFilesTool;
        let result = tool
            .execute(json!({"pattern": "NEEDLE"}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["total_matches"].as_u64().unwrap(), 0);
        assert_eq!(parsed["files_searched"].as_u64().unwrap(), 0);
    }

    #[tokio::test]
    async fn test_grep_files_invalid_regex() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let tool = GrepFilesTool;
        let result = tool.execute(json!({"pattern": "[invalid"}), &ctx).await;

        assert!(result.is_err());
    }

    #[test]
    fn test_grep_files_tool_properties() {
        let tool = GrepFilesTool;
        assert_eq!(tool.name(), "grep_files");
        assert!(tool.is_read_only());
        assert!(tool.is_sandboxable());
        assert_eq!(tool.approval_requirement(), ApprovalRequirement::Auto);
    }

    #[test]
    fn test_parallel_support_flags() {
        let tool = GrepFilesTool;
        assert!(tool.supports_parallel());
    }
}
