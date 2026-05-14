//! Tool for long-form memory and continuity diagnosis.
//!
//! The tool name remains `review` for protocol compatibility, but the active
//! Novel Studio behavior is a read-only diagnosis of manuscript assets.

use std::fs;
use std::path::Path;
use std::process::Command;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::DeepSeekClient;
use crate::llm_client::LlmClient;
use crate::models::{ContentBlock, Message, MessageRequest, SystemPrompt, Usage};
use crate::utils::truncate_with_ellipsis;

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
    optional_bool, optional_str, optional_u64, required_str,
};

const DEFAULT_MAX_CHARS: usize = 200_000;
const MAX_MAX_CHARS: usize = 1_000_000;
const REVIEW_MAX_TOKENS: u32 = 2048;
const FALLBACK_MAX_CHARS: usize = 4000;

const REVIEW_SYSTEM_PROMPT: &str = "You are a long-form fiction memory diagnostician. Return ONLY valid JSON with \
the following schema:\n\
{\n\
  \"summary\": \"short overview of continuity state\",\n\
  \"issues\": [\n\
    {\n\
      \"severity\": \"error|warning|info\",\n\
      \"title\": \"memory or continuity issue title\",\n\
      \"description\": \"details, story impact, and compatible fix\",\n\
      \"path\": \"relative/file/path or null\",\n\
      \"line\": 123\n\
    }\n\
  ],\n\
  \"suggestions\": [\n\
    {\n\
      \"path\": \"relative/file/path or null\",\n\
      \"line\": 123,\n\
      \"suggestion\": \"memory update or continuity-preserving revision\"\n\
    }\n\
  ],\n\
  \"affected_nodes\": [\"character:lin_mo\", \"promise:accident_truth\"],\n\
  \"candidate_memory_updates\": [\n\
    {\n\
      \"chapter\": 12,\n\
      \"kind\": \"knowledge|relationship|promise|event|character_state|location_state|object_state|memory\",\n\
      \"target\": \"character or story entity\",\n\
      \"change\": \"durable change to remember\",\n\
      \"evidence\": \"relative/path.md:line or short source quote\",\n\
      \"confidence\": 0.8,\n\
      \"affects\": [\"character:lin_mo\"]\n\
    }\n\
  ],\n\
  \"overall_assessment\": \"final continuity assessment\"\n\
}\n\
If a field is unknown, use an empty string or null. Diagnose canon conflicts, timeline drift, \
character knowledge leaks, broken promises, missing memory updates, and reader-promise risk. \
Return candidate_memory_updates only for durable facts that should enter reviewable memory candidates. \
Do not grade prose with a formula and do not force a chapter into a template.";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewIssue {
    #[serde(default)]
    pub severity: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub line: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewSuggestion {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub line: Option<u32>,
    #[serde(default)]
    pub suggestion: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewMemoryCandidate {
    #[serde(default)]
    pub chapter: Option<u32>,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub change: String,
    #[serde(default)]
    pub evidence: String,
    #[serde(default)]
    pub confidence: f32,
    #[serde(default)]
    pub affects: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewOutput {
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub issues: Vec<ReviewIssue>,
    #[serde(default)]
    pub suggestions: Vec<ReviewSuggestion>,
    #[serde(default)]
    pub affected_nodes: Vec<String>,
    #[serde(default)]
    pub candidate_memory_updates: Vec<ReviewMemoryCandidate>,
    #[serde(default)]
    pub overall_assessment: String,
}

impl ReviewOutput {
    #[must_use]
    pub fn from_str(raw: &str) -> Self {
        if let Ok(parsed) = serde_json::from_str::<ReviewOutput>(raw) {
            return parsed.normalize();
        }
        if let Some(json_block) = extract_json_block(raw)
            && let Ok(parsed) = serde_json::from_str::<ReviewOutput>(json_block)
        {
            return parsed.normalize();
        }
        ReviewOutput::fallback(raw)
    }

    fn fallback(raw: &str) -> Self {
        let trimmed = raw.trim();
        let summary = if trimmed.is_empty() {
            "Memory diagnosis completed but no structured output was returned.".to_string()
        } else {
            truncate_with_ellipsis(trimmed, FALLBACK_MAX_CHARS, "\n...[truncated]\n")
        };
        Self {
            summary,
            issues: Vec::new(),
            suggestions: Vec::new(),
            affected_nodes: Vec::new(),
            candidate_memory_updates: Vec::new(),
            overall_assessment: String::new(),
        }
    }

    fn normalize(mut self) -> Self {
        self.summary = self.summary.trim().to_string();
        self.overall_assessment = self.overall_assessment.trim().to_string();
        for issue in &mut self.issues {
            issue.severity = normalize_severity(&issue.severity);
            issue.title = issue.title.trim().to_string();
            issue.description = issue.description.trim().to_string();
            issue.path = normalize_optional(issue.path.take());
        }
        for suggestion in &mut self.suggestions {
            suggestion.suggestion = suggestion.suggestion.trim().to_string();
            suggestion.path = normalize_optional(suggestion.path.take());
        }
        self.affected_nodes = self
            .affected_nodes
            .into_iter()
            .map(|node| node.trim().to_string())
            .filter(|node| !node.is_empty())
            .collect();
        for candidate in &mut self.candidate_memory_updates {
            candidate.kind = canonical_review_candidate_kind(&candidate.kind);
            candidate.target = candidate.target.trim().to_string();
            candidate.change = candidate.change.trim().to_string();
            candidate.evidence = candidate.evidence.trim().to_string();
            candidate.confidence = candidate.confidence.clamp(0.0, 1.0);
            candidate.affects = candidate
                .affects
                .iter()
                .map(|node| node.trim().to_string())
                .filter(|node| !node.is_empty())
                .collect();
        }
        self.candidate_memory_updates.retain(|candidate| {
            !candidate.kind.is_empty()
                && !candidate.target.is_empty()
                && !candidate.change.is_empty()
        });
        self
    }
}

pub struct ReviewTool {
    client: Option<DeepSeekClient>,
    model: String,
}

impl ReviewTool {
    #[must_use]
    pub fn new(client: Option<DeepSeekClient>, model: String) -> Self {
        Self { client, model }
    }
}

#[async_trait]
impl ToolSpec for ReviewTool {
    fn name(&self) -> &'static str {
        "review"
    }

    fn description(&self) -> &'static str {
        "Diagnose long-form novel continuity and memory risks in a file or chapter asset."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "target": {
                    "type": "string",
                    "description": "Novel asset path such as chapters/012/draft.md, chapters/012/final.md, a bible/card/outline/memory file, or a legacy diff/PR target."
                },
                "kind": {
                    "type": "string",
                    "description": "Optional explicit target type: file, diff, or pr. Use file for normal Novel Studio memory diagnosis."
                },
                "base": {
                    "type": "string",
                    "description": "Legacy optional git base ref when using diff target."
                },
                "staged": {
                    "type": "boolean",
                    "description": "Legacy: inspect staged changes when using diff target (default: false)."
                },
                "max_chars": {
                    "type": "integer",
                    "description": "Maximum characters to include from the source (default: 200000)."
                }
            },
            "required": ["target"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly, ToolCapability::Network]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let Some(client) = self.client.clone() else {
            return Err(ToolError::not_available(
                "Memory diagnosis tool requires an active DeepSeek client".to_string(),
            ));
        };

        let target = required_str(&input, "target")?.trim();
        if target.is_empty() {
            return Err(ToolError::invalid_input("target cannot be empty"));
        }

        let kind = optional_str(&input, "kind").map(|s| s.trim().to_ascii_lowercase());
        let base = optional_str(&input, "base").map(|s| s.trim().to_string());
        let staged = optional_bool(&input, "staged", false);
        let max_chars =
            usize::try_from(optional_u64(&input, "max_chars", DEFAULT_MAX_CHARS as u64))
                .unwrap_or(DEFAULT_MAX_CHARS)
                .clamp(1, MAX_MAX_CHARS);

        let source =
            resolve_review_source(target, kind.as_deref(), staged, base.as_deref(), context)?;
        let prompt = build_review_prompt(&source, max_chars);

        let request = MessageRequest {
            model: self.model.clone(),
            messages: vec![Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: prompt,
                    cache_control: None,
                }],
            }],
            max_tokens: REVIEW_MAX_TOKENS,
            system: Some(SystemPrompt::Text(REVIEW_SYSTEM_PROMPT.to_string())),
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: None,
            stream: Some(false),
            temperature: Some(0.2),
            top_p: Some(0.9),
        };

        let response = client.create_message(request).await.map_err(|e| {
            ToolError::execution_failed(format!("Memory diagnosis request failed: {e}"))
        })?;

        let response_text = extract_text(&response.content);
        let output = ReviewOutput::from_str(&response_text);
        let metadata = review_usage_metadata(&response.model, &response.usage);
        let result =
            ToolResult::json(&output).map_err(|e| ToolError::execution_failed(e.to_string()))?;
        Ok(result.with_metadata(metadata))
    }
}

fn review_usage_metadata(model: &str, usage: &Usage) -> Value {
    json!({
        "tool": "review",
        "input_tokens": usage.input_tokens,
        "output_tokens": usage.output_tokens,
        "child_model": model,
        "child_input_tokens": usage.input_tokens,
        "child_output_tokens": usage.output_tokens,
        "child_prompt_cache_hit_tokens": usage.prompt_cache_hit_tokens,
        "child_prompt_cache_miss_tokens": usage.prompt_cache_miss_tokens,
        "child_reasoning_tokens": usage.reasoning_tokens,
    })
}

enum ReviewSource {
    File { display: String, content: String },
    Diff { label: String, diff: String },
    PullRequest { label: String, diff: String },
}

fn resolve_review_source(
    target: &str,
    kind: Option<&str>,
    staged: bool,
    base: Option<&str>,
    context: &ToolContext,
) -> Result<ReviewSource, ToolError> {
    if let Some(kind) = kind {
        return match kind {
            "file" => resolve_file_target(target, context),
            "diff" => resolve_diff_target(context.workspace.as_path(), staged, base).map(|diff| {
                ReviewSource::Diff {
                    label: "git diff".to_string(),
                    diff,
                }
            }),
            "pr" | "pull" | "pull_request" => {
                let pr = parse_pr_url(target)
                    .ok_or_else(|| ToolError::invalid_input("Invalid pull request URL"))?;
                let diff = gh_pr_diff(&pr, &context.workspace)?;
                Ok(ReviewSource::PullRequest {
                    label: pr.label(),
                    diff,
                })
            }
            other => Err(ToolError::invalid_input(format!(
                "Unknown memory diagnosis target kind '{other}'"
            ))),
        };
    }

    if let Some(pr) = parse_pr_url(target) {
        let diff = gh_pr_diff(&pr, &context.workspace)?;
        return Ok(ReviewSource::PullRequest {
            label: pr.label(),
            diff,
        });
    }

    if let Some(staged_override) = diff_mode_from_target(target) {
        let staged = staged || staged_override;
        let diff = resolve_diff_target(context.workspace.as_path(), staged, base)?;
        return Ok(ReviewSource::Diff {
            label: if staged {
                "git diff --cached"
            } else {
                "git diff"
            }
            .to_string(),
            diff,
        });
    }

    resolve_file_target(target, context)
}

fn resolve_file_target(target: &str, context: &ToolContext) -> Result<ReviewSource, ToolError> {
    let path = context.resolve_path(target)?;
    if !path.is_file() {
        return Err(ToolError::invalid_input(format!(
            "Target is not a file: {}",
            path.display()
        )));
    }
    let content = fs::read_to_string(&path).map_err(|e| {
        ToolError::execution_failed(format!("Failed to read file {}: {e}", path.display()))
    })?;
    let display = path
        .strip_prefix(&context.workspace)
        .unwrap_or(&path)
        .to_string_lossy()
        .to_string();
    Ok(ReviewSource::File { display, content })
}

fn resolve_diff_target(
    workspace: &Path,
    staged: bool,
    base: Option<&str>,
) -> Result<String, ToolError> {
    let mut cmd = Command::new("git");
    cmd.arg("diff");
    if staged {
        cmd.arg("--cached");
    }
    if let Some(base) = base
        && !base.trim().is_empty()
    {
        cmd.arg(format!("{base}...HEAD"));
    }
    cmd.current_dir(workspace);

    let output = cmd
        .output()
        .map_err(|e| ToolError::execution_failed(format!("Failed to run git diff: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ToolError::execution_failed(format!(
            "git diff failed: {}",
            stderr.trim()
        )));
    }
    let diff = String::from_utf8_lossy(&output.stdout).to_string();
    if diff.trim().is_empty() {
        return Err(ToolError::invalid_input("No diff to diagnose"));
    }
    Ok(diff)
}

fn gh_pr_diff(pr: &PullRequestRef, workspace: &Path) -> Result<String, ToolError> {
    let mut cmd = Command::new("gh");
    cmd.arg("pr")
        .arg("diff")
        .arg(&pr.number)
        .arg("--repo")
        .arg(format!("{}/{}", pr.owner, pr.repo))
        .current_dir(workspace);

    let output = cmd.output().map_err(|e| {
        ToolError::execution_failed(format!("Failed to run gh pr diff (is gh installed?): {e}"))
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ToolError::execution_failed(format!(
            "gh pr diff failed: {}",
            stderr.trim()
        )));
    }
    let diff = String::from_utf8_lossy(&output.stdout).to_string();
    if diff.trim().is_empty() {
        return Err(ToolError::invalid_input("Pull request diff is empty."));
    }
    Ok(diff)
}

fn build_review_prompt(source: &ReviewSource, max_chars: usize) -> String {
    match source {
        ReviewSource::File {
            display, content, ..
        } => {
            let numbered = format_with_line_numbers(content);
            let truncated = truncate_with_ellipsis(&numbered, max_chars, "\n...[truncated]\n");
            format!(
                "Diagnose the following novel asset for long-form memory and continuity risks.\n\
Focus on canon conflicts, timeline drift, character knowledge boundaries, relationship state, location/object state, foreshadowing, missing memory updates, and compatible fixes. Do not score prose or force a template.\n\
When durable facts should be remembered, return them in `candidate_memory_updates` with chapter, kind, target, change, evidence, confidence, and affects. Keep them reviewable; do not claim they were written to memory ledgers.\n\
Path: {display}\n\n{truncated}\n\nEnd of file."
            )
        }
        ReviewSource::Diff { label, diff } => {
            let truncated = truncate_with_ellipsis(diff, max_chars, "\n...[truncated]\n");
            format!(
                "Diagnose the following legacy {label} for changes that could affect novel continuity or memory assets. Return affected_nodes and reviewable candidate_memory_updates when durable facts should be remembered.\n\n{truncated}\n\nEnd of diff."
            )
        }
        ReviewSource::PullRequest { label, diff } => {
            let truncated = truncate_with_ellipsis(diff, max_chars, "\n...[truncated]\n");
            format!(
                "Diagnose the following legacy pull request diff ({label}) for changes that could affect novel continuity or memory assets. Return affected_nodes and reviewable candidate_memory_updates when durable facts should be remembered.\n\n{truncated}\n\nEnd of diff."
            )
        }
    }
}

fn format_with_line_numbers(content: &str) -> String {
    content
        .lines()
        .enumerate()
        .map(|(idx, line)| format!("{:>4} | {}", idx + 1, line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn extract_text(blocks: &[ContentBlock]) -> String {
    let mut output = String::new();
    for block in blocks {
        if let ContentBlock::Text { text, .. } = block {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(text);
        }
    }
    output.trim().to_string()
}

fn normalize_optional(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn normalize_severity(value: &str) -> String {
    let lower = value.trim().to_ascii_lowercase();
    if lower.starts_with("err") || lower == "critical" || lower == "high" {
        "error".to_string()
    } else if lower.starts_with("warn") || lower == "medium" {
        "warning".to_string()
    } else {
        "info".to_string()
    }
}

fn canonical_review_candidate_kind(kind: &str) -> String {
    match kind.trim().to_ascii_lowercase().as_str() {
        "foreshadowing" | "foreshadow" => "promise".to_string(),
        "timeline" => "event".to_string(),
        "knowledge" | "relationship" | "promise" | "event" | "character_state"
        | "location_state" | "object_state" | "memory" => kind.trim().to_ascii_lowercase(),
        other => other.to_string(),
    }
}

fn extract_json_block(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end <= start {
        None
    } else {
        Some(&raw[start..=end])
    }
}

fn diff_mode_from_target(target: &str) -> Option<bool> {
    match target.trim().to_ascii_lowercase().as_str() {
        "diff" | "git diff" | "changes" | "working tree" | "working-tree" => Some(false),
        "staged" | "cached" | "git diff --cached" | "git diff --staged" => Some(true),
        _ => None,
    }
}

#[derive(Debug, Clone)]
struct PullRequestRef {
    owner: String,
    repo: String,
    number: String,
}

impl PullRequestRef {
    fn label(&self) -> String {
        format!("{}/{}#{}", self.owner, self.repo, self.number)
    }
}

fn parse_pr_url(url: &str) -> Option<PullRequestRef> {
    let trimmed = url.trim().trim_end_matches('/');
    if !trimmed.starts_with("http") {
        return None;
    }
    let parts: Vec<&str> = trimmed.split('/').collect();
    let pull_idx = parts.iter().position(|part| *part == "pull")?;
    if pull_idx < 2 || pull_idx + 1 >= parts.len() {
        return None;
    }
    let owner = parts.get(pull_idx.saturating_sub(2))?;
    let repo = parts.get(pull_idx.saturating_sub(1))?;
    let number = parts.get(pull_idx + 1)?;
    if owner.is_empty() || repo.is_empty() || number.is_empty() {
        return None;
    }
    Some(PullRequestRef {
        owner: (*owner).to_string(),
        repo: (*repo).to_string(),
        number: (*number).to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pr_url() {
        let pr =
            parse_pr_url("https://github.com/deepseek-ai/deepseek-cli/pull/123").expect("parse pr");
        assert_eq!(pr.owner, "deepseek-ai");
        assert_eq!(pr.repo, "deepseek-cli");
        assert_eq!(pr.number, "123");
    }

    #[test]
    fn ignores_non_pr_url() {
        assert!(parse_pr_url("https://github.com/deepseek-ai/deepseek-cli").is_none());
        assert!(parse_pr_url("not-a-url").is_none());
    }

    #[test]
    fn extracts_json_block() {
        let raw = "prefix {\"summary\":\"ok\"} suffix";
        let block = extract_json_block(raw).expect("block");
        assert!(block.contains("\"summary\""));
    }

    #[test]
    fn memory_diagnosis_output_fallback_keeps_summary() {
        let output = ReviewOutput::from_str("Not JSON");
        assert!(!output.summary.is_empty());
        assert!(output.issues.is_empty());
    }

    #[test]
    fn memory_diagnosis_output_keeps_affected_nodes_and_candidates() {
        let output = ReviewOutput::from_str(
            r#"{
              "summary": "ok",
              "affected_nodes": [" character:lin_mo ", "", "promise:accident_truth"],
              "candidate_memory_updates": [
                {
                  "chapter": 12,
                  "kind": "foreshadowing",
                  "target": "事故真相",
                  "change": "推进为林墨主动追查",
                  "evidence": "chapters/012/final.md:88",
                  "confidence": 1.5,
                  "affects": [" character:lin_mo ", ""]
                },
                {
                  "kind": "memory",
                  "target": "",
                  "change": "should be dropped"
                }
              ]
            }"#,
        );

        assert_eq!(
            output.affected_nodes,
            vec!["character:lin_mo", "promise:accident_truth"]
        );
        assert_eq!(output.candidate_memory_updates.len(), 1);
        let candidate = &output.candidate_memory_updates[0];
        assert_eq!(candidate.chapter, Some(12));
        assert_eq!(candidate.kind, "promise");
        assert_eq!(candidate.target, "事故真相");
        assert_eq!(candidate.confidence, 1.0);
        assert_eq!(candidate.affects, vec!["character:lin_mo"]);
    }

    #[test]
    fn memory_diagnosis_usage_metadata_reports_child_tokens_for_cost_accrual() {
        let metadata = review_usage_metadata(
            "deepseek-v4-flash",
            &Usage {
                input_tokens: 123,
                output_tokens: 45,
                prompt_cache_hit_tokens: Some(100),
                prompt_cache_miss_tokens: Some(23),
                reasoning_tokens: Some(7),
                ..Default::default()
            },
        );

        assert_eq!(metadata["tool"], "review");
        assert_eq!(metadata["child_model"], "deepseek-v4-flash");
        assert_eq!(metadata["child_input_tokens"], 123);
        assert_eq!(metadata["child_output_tokens"], 45);
        assert_eq!(metadata["child_prompt_cache_hit_tokens"], 100);
        assert_eq!(metadata["child_prompt_cache_miss_tokens"], 23);
        assert_eq!(metadata["child_reasoning_tokens"], 7);
    }
}
