//! Project mapping tool for understanding novel workspace structure.

use crate::novel;
use crate::utils::is_key_file;
use anyhow::Result;
use async_trait::async_trait;
use serde::Serialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec, optional_u64,
};

pub struct ProjectMapTool;

#[derive(Debug, Serialize)]
struct ProjectMap {
    kind: &'static str,
    tree: String,
    summary: String,
    key_files: Vec<String>,
}

#[derive(Debug, Serialize)]
struct NovelProjectMap {
    kind: &'static str,
    map: String,
}

#[async_trait]
impl ToolSpec for ProjectMapTool {
    fn name(&self) -> &'static str {
        "project_map"
    }

    fn description(&self) -> &'static str {
        "Get a high-level map of the novel workspace, including book assets and a tree view."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "max_depth": {
                    "type": "integer",
                    "description": "Maximum depth for the tree view (default: 3)."
                }
            }
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly, ToolCapability::Sandboxable]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let max_depth = optional_u64(&input, "max_depth", 3) as usize;
        if context.workspace.join("book.toml").is_file() {
            let map = novel::project_map_packet(&context.workspace)
                .map_err(|e| ToolError::execution_failed(e.to_string()))?;
            return ToolResult::json(&NovelProjectMap { kind: "novel", map })
                .map_err(|e| ToolError::execution_failed(e.to_string()));
        }
        let map = generate_project_map(&context.workspace, max_depth)?;
        ToolResult::json(&map).map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

fn generate_project_map(root: &Path, max_depth: usize) -> Result<ProjectMap, ToolError> {
    let mut tree_entries: Vec<ProjectTreeEntry> = Vec::new();
    let mut key_files = Vec::new();

    let mut builder = ignore::WalkBuilder::new(root);
    builder
        .hidden(false)
        .follow_links(false)
        .max_depth(Some(max_depth.max(2) + 1));
    let walker = builder.build();

    for entry in walker {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }

        let depth = entry.depth();
        if depth == 0 {
            continue;
        }

        let rel_path = entry
            .path()
            .strip_prefix(root)
            .unwrap_or(entry.path())
            .to_path_buf();

        if depth <= max_depth {
            tree_entries.push(ProjectTreeEntry {
                rel_path: rel_path.clone(),
                is_dir: file_type.is_dir(),
            });
        }

        if depth <= 2 && is_key_file(entry.path()) {
            key_files.push(rel_path.to_string_lossy().to_string());
        }
    }

    tree_entries.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    key_files.sort();

    let tree = render_project_tree(tree_entries);
    let summary = summarize_key_files(&key_files);

    Ok(ProjectMap {
        kind: "directory",
        tree,
        summary,
        key_files,
    })
}

#[derive(Debug)]
struct ProjectTreeEntry {
    rel_path: PathBuf,
    is_dir: bool,
}

fn render_project_tree(entries: Vec<ProjectTreeEntry>) -> String {
    let mut tree_lines = Vec::with_capacity(entries.len());
    for entry in entries {
        let depth = entry.rel_path.components().count();
        let indent = "  ".repeat(depth.saturating_sub(1));
        let prefix = if entry.is_dir { "DIR: " } else { "FILE: " };
        tree_lines.push(format!(
            "{}{}{}",
            indent,
            prefix,
            entry
                .rel_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        ));
    }
    tree_lines.join("\n")
}

fn summarize_key_files(key_files: &[String]) -> String {
    if key_files.is_empty() {
        return "Unknown project type".to_string();
    }

    let mut types = Vec::new();
    if key_files
        .iter()
        .any(|f| f.to_lowercase().contains("cargo.toml"))
    {
        types.push("Rust");
    }
    if key_files
        .iter()
        .any(|f| f.to_lowercase().contains("package.json"))
    {
        types.push("JavaScript/Node.js");
    }
    if key_files
        .iter()
        .any(|f| f.to_lowercase().contains("requirements.txt"))
    {
        types.push("Python");
    }

    if types.is_empty() {
        format!("Project with key files: {}", key_files.join(", "))
    } else {
        format!("A {} project", types.join(" and "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::novel::{self, NovelInitOptions};
    use crate::tools::spec::ToolContext;
    use tempfile::TempDir;

    fn tool_context(workspace: &std::path::Path) -> ToolContext {
        ToolContext::new(workspace.to_path_buf())
    }

    #[tokio::test]
    async fn project_map_tool_returns_novel_map_for_book_workspaces() {
        let tmp = TempDir::new().expect("temp dir");
        novel::initialize_project(
            tmp.path(),
            NovelInitOptions {
                title: Some("工具地图测试".to_string()),
                genre: Some("悬疑".to_string()),
                premise: None,
                target_words: 80_000,
                language: "zh-CN".to_string(),
                force: false,
            },
        )
        .expect("init");
        std::fs::write(
            tmp.path().join("cards/characters/lin_mo.yaml"),
            "name: 林墨\n",
        )
        .expect("character");

        let result = ProjectMapTool
            .execute(json!({}), &tool_context(tmp.path()))
            .await
            .expect("tool result");
        let value: serde_json::Value = serde_json::from_str(&result.content).expect("json result");

        assert_eq!(value["kind"], "novel");
        assert!(
            value["map"]
                .as_str()
                .unwrap_or_default()
                .contains("# Novel Project Map")
        );
        assert!(
            value["map"]
                .as_str()
                .unwrap_or_default()
                .contains("工具地图测试")
        );
        assert!(
            value["map"]
                .as_str()
                .unwrap_or_default()
                .contains("cards/characters/lin_mo.yaml")
        );
    }

    #[tokio::test]
    async fn project_map_tool_keeps_directory_map_for_plain_workspaces() {
        let tmp = TempDir::new().expect("temp dir");
        std::fs::write(tmp.path().join("README.md"), "# Plain").expect("readme");

        let result = ProjectMapTool
            .execute(json!({ "max_depth": 1 }), &tool_context(tmp.path()))
            .await
            .expect("tool result");
        let value: serde_json::Value = serde_json::from_str(&result.content).expect("json result");

        assert_eq!(value["kind"], "directory");
        assert!(result.content.contains("README.md"));
        assert!(!result.content.contains("# Novel Project Map"));
    }
}
