//! File search tool with fuzzy matching and scoring.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use ignore::WalkBuilder;
use serde::Serialize;
use serde_json::{Value, json};

use crate::tools::search::GlobMatcher;

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
    optional_str, optional_u64, required_str,
};

#[derive(Debug, Clone, Serialize)]
struct FileSearchMatch {
    path: String,
    name: String,
    score: f64,
}

#[derive(Debug, Clone)]
struct FileCandidate {
    path: String,
    name: String,
    extension: Option<String>,
}

#[derive(Debug, Clone)]
struct CachedFileIndex {
    root_mtime: Option<SystemTime>,
    shallow_fingerprint: ShallowFingerprint,
    indexed_at: Instant,
    paths: Arc<Vec<FileCandidate>>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct ShallowFingerprint {
    entries: Vec<ShallowEntry>,
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct ShallowEntry {
    path: String,
    is_dir: bool,
    modified: Option<SystemTime>,
}

static FILE_INDEX_CACHE: OnceLock<Mutex<HashMap<PathBuf, CachedFileIndex>>> = OnceLock::new();
const FILE_INDEX_TTL: Duration = Duration::from_secs(2);

pub struct FileSearchTool;

#[async_trait]
impl ToolSpec for FileSearchTool {
    fn name(&self) -> &'static str {
        "file_search"
    }

    fn description(&self) -> &'static str {
        "Find files by name using fuzzy matching with score-based ranking. Use this instead of `find -name` or `fd` in `exec_shell` for filename search. Pass `extensions` to filter by suffix."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query (file name or path fragment)."
                },
                "path": {
                    "type": "string",
                    "description": "Optional base path to search (relative to workspace)."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 20)."
                },
                "extensions": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional list of file extensions to include (e.g. [\"rs\", \"md\"])."
                },
                "exclude": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional glob patterns to exclude, matching grep_files' convention (e.g. [\"target/**\", \"*.lock\"])."
                }
            },
            "required": ["query"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly, ToolCapability::Sandboxable]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let query = required_str(&input, "query")?.trim();
        if query.is_empty() {
            return Err(ToolError::invalid_input("query cannot be empty"));
        }

        let limit = optional_u64(&input, "limit", 20).clamp(1, 200) as usize;
        let base_path = match optional_str(&input, "path") {
            Some(path) if !path.trim().is_empty() => context.resolve_path(path)?,
            _ => context.workspace.clone(),
        };

        let extensions = parse_extensions(&input);
        let exclude_patterns = parse_exclude_patterns(&input);
        let exclude_matcher = GlobMatcher::new(&exclude_patterns)?;
        let matches = search_files(query, &base_path, extensions, &exclude_matcher, limit)?;
        ToolResult::json(&matches).map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

fn parse_extensions(input: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(values) = input.get("extensions").and_then(|v| v.as_array()) {
        for value in values {
            if let Some(ext) = value.as_str() {
                let ext = ext.trim().trim_start_matches('.').to_ascii_lowercase();
                if !ext.is_empty() {
                    out.push(ext);
                }
            }
        }
    }
    if out.is_empty()
        && let Some(value) = input.get("extension").and_then(|v| v.as_str())
    {
        let ext = value.trim().trim_start_matches('.').to_ascii_lowercase();
        if !ext.is_empty() {
            out.push(ext);
        }
    }
    out
}

fn parse_exclude_patterns(input: &Value) -> Vec<String> {
    if let Some(values) = input.get("exclude").and_then(Value::as_array) {
        return values
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|pattern| !pattern.is_empty())
            .map(ToOwned::to_owned)
            .collect();
    }

    [
        "target/**",
        "node_modules/**",
        ".git/**",
        "DerivedData/**",
        "dist/**",
        "build/**",
        "*.lock",
        "*.plist",
    ]
    .into_iter()
    .map(ToOwned::to_owned)
    .collect()
}

fn search_files(
    query: &str,
    base_path: &Path,
    extensions: Vec<String>,
    exclude_matcher: &GlobMatcher,
    limit: usize,
) -> Result<Vec<FileSearchMatch>, ToolError> {
    if !base_path.exists() {
        return Err(ToolError::invalid_input(format!(
            "Base path does not exist: {}",
            base_path.display()
        )));
    }

    let query_norm = query.to_ascii_lowercase();
    let mut top = TopFileMatches::new(limit);

    let candidates = cached_file_candidates(base_path);
    for candidate in candidates.iter() {
        if exclude_matcher.is_match(&candidate.path) {
            continue;
        }

        if !extensions.is_empty() && !candidate_extension_matches(candidate, &extensions) {
            continue;
        }

        let score = match score_match(&query_norm, &candidate.path, &candidate.name) {
            Some(score) => score,
            None => continue,
        };

        top.push(FileSearchMatch {
            path: candidate.path.clone(),
            name: candidate.name.clone(),
            score,
        });
    }

    Ok(top.into_sorted_vec())
}

fn cached_file_candidates(base_path: &Path) -> Arc<Vec<FileCandidate>> {
    let cache_key = file_index_cache_key(base_path);
    let root_mtime = path_mtime(base_path);
    let shallow_fingerprint = shallow_fingerprint(base_path);
    let cache = FILE_INDEX_CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    {
        let guard = cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(cached) = guard.get(&cache_key)
            && cached.root_mtime == root_mtime
            && cached.shallow_fingerprint == shallow_fingerprint
            && cached.indexed_at.elapsed() <= FILE_INDEX_TTL
        {
            return Arc::clone(&cached.paths);
        }
    }

    let paths = Arc::new(build_file_candidates(base_path));

    let mut guard = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.insert(
        cache_key,
        CachedFileIndex {
            root_mtime,
            shallow_fingerprint,
            indexed_at: Instant::now(),
            paths: Arc::clone(&paths),
        },
    );
    paths
}

fn file_index_cache_key(base_path: &Path) -> PathBuf {
    base_path
        .canonicalize()
        .unwrap_or_else(|_| base_path.to_path_buf())
}

fn path_mtime(path: &Path) -> Option<SystemTime> {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
}

fn shallow_fingerprint(base_path: &Path) -> ShallowFingerprint {
    let mut entries = Vec::new();
    collect_shallow_fingerprint(base_path, base_path, 0, &mut entries);
    entries.sort();
    ShallowFingerprint { entries }
}

fn collect_shallow_fingerprint(
    root: &Path,
    current: &Path,
    depth: usize,
    entries: &mut Vec<ShallowEntry>,
) {
    if depth >= 2 {
        return;
    }

    let Ok(read_dir) = fs::read_dir(current) else {
        return;
    };

    for entry in read_dir.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }

        let path = entry.path();
        let is_dir = file_type.is_dir();
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok();
        let rel_path = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        entries.push(ShallowEntry {
            path: rel_path,
            is_dir,
            modified,
        });

        if is_dir {
            collect_shallow_fingerprint(root, &path, depth + 1, entries);
        }
    }
}

fn build_file_candidates(base_path: &Path) -> Vec<FileCandidate> {
    let mut candidates = Vec::new();
    let mut builder = WalkBuilder::new(base_path);
    builder.hidden(false).follow_links(false).require_git(false);
    let walker = builder.build();

    for entry in walker {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }

        let path = entry.path();
        let rel_path = path
            .strip_prefix(base_path)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let rel_path = if rel_path.is_empty() {
            file_name(path)
        } else {
            rel_path
        };

        let name = file_name(path);
        candidates.push(FileCandidate {
            path: rel_path,
            name,
            extension: file_extension(path),
        });
    }

    candidates
}

fn candidate_extension_matches(candidate: &FileCandidate, extensions: &[String]) -> bool {
    let Some(ext) = candidate.extension.as_ref() else {
        return false;
    };
    extensions.iter().any(|wanted| wanted == ext)
}

fn file_extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

fn score_match(query: &str, rel_path: &str, name: &str) -> Option<f64> {
    let path_norm = rel_path.to_ascii_lowercase();
    let name_norm = name.to_ascii_lowercase();

    if name_norm == query {
        return Some(1.0);
    }
    if path_norm == query {
        return Some(0.98);
    }

    if name_norm.starts_with(query) {
        return Some(0.9 + length_bonus(query, &name_norm));
    }
    if path_norm.starts_with(query) {
        return Some(0.85 + length_bonus(query, &path_norm));
    }

    if name_norm.contains(query) {
        return Some(0.75 + length_bonus(query, &name_norm));
    }
    if path_norm.contains(query) {
        return Some(0.7 + length_bonus(query, &path_norm));
    }

    if let Some(score) = fuzzy_score(query, &name_norm) {
        return Some(0.6 + 0.4 * score);
    }
    if let Some(score) = fuzzy_score(query, &path_norm) {
        return Some(0.55 + 0.4 * score);
    }

    None
}

fn length_bonus(query: &str, target: &str) -> f64 {
    let q_len = query.chars().count().max(1) as f64;
    let t_len = target.chars().count().max(1) as f64;
    (q_len / t_len).min(1.0) * 0.08
}

fn fuzzy_score(query: &str, target: &str) -> Option<f64> {
    let mut positions = Vec::new();
    let mut query_chars = query.chars();
    let mut current = query_chars.next()?;

    for (idx, ch) in target.chars().enumerate() {
        if ch == current {
            positions.push(idx);
            if let Some(next) = query_chars.next() {
                current = next;
            } else {
                break;
            }
        }
    }

    if positions.len() != query.chars().count() {
        return None;
    }

    let first = *positions.first().unwrap_or(&0) as f64;
    let last = *positions.last().unwrap_or(&0) as f64;
    let span = (last - first + 1.0).max(1.0);
    let query_len = query.chars().count().max(1) as f64;
    let target_len = target.chars().count().max(1) as f64;

    let density = (query_len / span).min(1.0);
    let coverage = (query_len / target_len).min(1.0);
    Some((density * 0.7 + coverage * 0.3).min(1.0))
}

fn compare_match(a: &FileSearchMatch, b: &FileSearchMatch) -> Ordering {
    b.score
        .partial_cmp(&a.score)
        .unwrap_or(Ordering::Equal)
        .then_with(|| a.path.cmp(&b.path))
}

#[derive(Debug)]
struct RankedFileSearchMatch(FileSearchMatch);

impl Eq for RankedFileSearchMatch {}

impl PartialEq for RankedFileSearchMatch {
    fn eq(&self, other: &Self) -> bool {
        compare_match(&self.0, &other.0) == Ordering::Equal
    }
}

impl Ord for RankedFileSearchMatch {
    fn cmp(&self, other: &Self) -> Ordering {
        compare_match(&self.0, &other.0)
    }
}

impl PartialOrd for RankedFileSearchMatch {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

struct TopFileMatches {
    limit: usize,
    heap: BinaryHeap<RankedFileSearchMatch>,
}

impl TopFileMatches {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            heap: BinaryHeap::with_capacity(limit),
        }
    }

    fn push(&mut self, item: FileSearchMatch) {
        if self.heap.len() < self.limit {
            self.heap.push(RankedFileSearchMatch(item));
            return;
        }

        let should_replace = self
            .heap
            .peek()
            .is_some_and(|worst| compare_match(&item, &worst.0) == Ordering::Less);
        if should_replace {
            self.heap.pop();
            self.heap.push(RankedFileSearchMatch(item));
        }
    }

    fn into_sorted_vec(self) -> Vec<FileSearchMatch> {
        let mut results: Vec<FileSearchMatch> =
            self.heap.into_iter().map(|ranked| ranked.0).collect();
        results.sort_by(compare_match);
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn clear_file_search_cache() {
        if let Some(cache) = FILE_INDEX_CACHE.get() {
            cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clear();
        }
    }

    #[tokio::test]
    async fn test_file_search_basic() {
        clear_file_search_cache();
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        std::fs::create_dir_all(root.join("src")).expect("mkdir");
        std::fs::write(root.join("src").join("main.rs"), "fn main() {}\n").expect("write");
        std::fs::write(root.join("README.md"), "docs\n").expect("write");

        let ctx = ToolContext::new(root.to_path_buf());
        let tool = FileSearchTool;
        let result = tool
            .execute(json!({"query": "main", "limit": 5}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        assert!(result.content.contains("main.rs"));
    }

    #[tokio::test]
    async fn test_file_search_respects_gitignore() {
        clear_file_search_cache();
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        std::fs::write(root.join(".gitignore"), "ignored.txt\n").expect("write");
        std::fs::write(root.join("ignored.txt"), "nope\n").expect("write");
        std::fs::write(root.join("keep.txt"), "ok\n").expect("write");

        let ctx = ToolContext::new(root.to_path_buf());
        let tool = FileSearchTool;
        let result = tool
            .execute(json!({"query": "txt"}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        assert!(!result.content.contains("ignored.txt"));
        assert!(result.content.contains("keep.txt"));
    }

    #[tokio::test]
    async fn test_file_search_extension_filter() {
        clear_file_search_cache();
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        std::fs::write(root.join("main.rs"), "fn main() {}\n").expect("write");
        std::fs::write(root.join("notes.md"), "docs\n").expect("write");

        let ctx = ToolContext::new(root.to_path_buf());
        let tool = FileSearchTool;
        let result = tool
            .execute(json!({"query": "m", "extensions": ["rs"]}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        assert!(result.content.contains("main.rs"));
        assert!(!result.content.contains("notes.md"));
    }

    #[tokio::test]
    async fn test_file_search_exclude_filter() {
        clear_file_search_cache();
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        std::fs::create_dir_all(root.join("fixtures")).expect("mkdir");
        std::fs::write(root.join("fixtures").join("needle.txt"), "no\n").expect("write");
        std::fs::write(root.join("needle.txt"), "yes\n").expect("write");

        let ctx = ToolContext::new(root.to_path_buf());
        let tool = FileSearchTool;
        let result = tool
            .execute(json!({"query": "needle", "exclude": ["fixtures/**"]}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        assert!(result.content.contains("\"path\": \"needle.txt\""));
        assert!(!result.content.contains("fixtures/needle.txt"));
    }

    #[tokio::test]
    async fn test_file_search_default_excludes_build_artifacts() {
        clear_file_search_cache();
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        std::fs::create_dir_all(root.join("target")).expect("mkdir");
        std::fs::write(root.join("target").join("needle.txt"), "no\n").expect("write");
        std::fs::write(root.join("needle.txt"), "yes\n").expect("write");

        let ctx = ToolContext::new(root.to_path_buf());
        let tool = FileSearchTool;
        let result = tool
            .execute(json!({"query": "needle"}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        assert!(result.content.contains("\"path\": \"needle.txt\""));
        assert!(!result.content.contains("target/needle.txt"));
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_file_search_does_not_follow_symlinked_files() {
        clear_file_search_cache();
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path().join("workspace");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&root).expect("mkdir workspace");
        std::fs::create_dir_all(&outside).expect("mkdir outside");
        let outside_file = outside.join("secret.txt");
        std::fs::write(&outside_file, "outside\n").expect("write outside");
        std::os::unix::fs::symlink(&outside_file, root.join("secret.txt")).expect("symlink");

        let ctx = ToolContext::new(root);
        let tool = FileSearchTool;
        let result = tool
            .execute(json!({"query": "secret"}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        assert!(!result.content.contains("secret.txt"));
    }

    #[tokio::test]
    async fn test_file_search_cache_invalidates_on_root_mtime_change() {
        clear_file_search_cache();
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        std::fs::write(root.join("first.txt"), "one\n").expect("write first");

        let ctx = ToolContext::new(root.to_path_buf());
        let tool = FileSearchTool;
        let first = tool
            .execute(json!({"query": "second"}), &ctx)
            .await
            .expect("first execute");
        assert!(!first.content.contains("second.txt"));

        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(root.join("second.txt"), "two\n").expect("write second");

        let second = tool
            .execute(json!({"query": "second"}), &ctx)
            .await
            .expect("second execute");
        assert!(second.content.contains("second.txt"));
    }

    #[tokio::test]
    async fn test_file_search_cache_ttl_refreshes_deep_changes() {
        clear_file_search_cache();
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        let nested = root.join("src").join("deep").join("module");
        std::fs::create_dir_all(&nested).expect("mkdir nested");
        std::fs::write(nested.join("first.txt"), "one\n").expect("write first");

        let ctx = ToolContext::new(root.to_path_buf());
        let tool = FileSearchTool;
        let first = tool
            .execute(json!({"query": "second"}), &ctx)
            .await
            .expect("first execute");
        assert!(!first.content.contains("second.txt"));

        std::fs::write(nested.join("second.txt"), "two\n").expect("write second");
        let cache_key = file_index_cache_key(root);
        {
            let cache = FILE_INDEX_CACHE.get().expect("cache initialized");
            let mut guard = cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let cached = guard.get_mut(&cache_key).expect("cached index");
            cached.root_mtime = path_mtime(root);
            cached.shallow_fingerprint = shallow_fingerprint(root);
            cached.indexed_at = Instant::now();
        }

        let stale = tool
            .execute(json!({"query": "second"}), &ctx)
            .await
            .expect("stale execute");
        assert!(!stale.content.contains("second.txt"));

        {
            let cache = FILE_INDEX_CACHE.get().expect("cache initialized");
            let mut guard = cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let cached = guard.get_mut(&cache_key).expect("cached index");
            cached.indexed_at = Instant::now() - FILE_INDEX_TTL - Duration::from_millis(100);
        }

        let refreshed = tool
            .execute(json!({"query": "second"}), &ctx)
            .await
            .expect("refreshed execute");
        assert!(refreshed.content.contains("second.txt"));
    }

    #[tokio::test]
    async fn test_file_search_top_k_keeps_best_ranked_matches() {
        clear_file_search_cache();
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        std::fs::write(root.join("z-main-helper.rs"), "").expect("write helper");
        std::fs::write(root.join("main.rs"), "").expect("write exact");
        std::fs::write(root.join("amain.rs"), "").expect("write contains");

        let ctx = ToolContext::new(root.to_path_buf());
        let tool = FileSearchTool;
        let result = tool
            .execute(json!({"query": "main", "limit": 1}), &ctx)
            .await
            .expect("execute");

        assert!(result.success);
        let matches: Vec<Value> = serde_json::from_str(&result.content).unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0]["path"], "main.rs");
    }
}
