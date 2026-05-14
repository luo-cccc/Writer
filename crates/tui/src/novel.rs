//! Long-form novel project commands.
//!
//! The novel workflow is intentionally file-first: the model generates prose
//! and editorial artifacts, while Rust owns the project layout and writes only
//! known files under the active book root.

use std::collections::{BTreeMap, BTreeSet, BinaryHeap};
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};

use crate::client::DeepSeekClient;
use crate::config::{ApiProvider, Config, DEFAULT_TEXT_MODEL};
use crate::llm_client::LlmClient;
use crate::models::{ContentBlock, Message, MessageRequest, SystemPrompt};
use crate::snapshot::SnapshotRepo;

const BOOK_MANIFEST: &str = "book.toml";
const CONTEXT_LIMIT: usize = 90_000;
const RECENT_CHAPTER_LIMIT: usize = 24_000;
const CHAPTER_BRIDGE_EXCERPT_LIMIT: usize = 1_200;
const CHAPTER_BRIDGE_KEYWORD_LIMIT: usize = 12;
const MEMORY_SUMMARY_CONTEXT_LIMIT: usize = 3_000;
const MEMORY_SUMMARY_CONTEXT_SECTION_LIMIT: usize = 520;
const MEMORY_SUMMARY_CONTEXT_FALLBACK_LIMIT: usize = 1_200;
const MEMORY_SUMMARY_CONTEXT_BULLET_LIMIT: usize = 4;
const MEMORY_SUMMARY_TARGET_MAX_CHARS: usize = 2_500;
const MEMORY_SUMMARY_OVERWEIGHT_CHARS: usize = 4_000;
const MEMORY_CANDIDATE_TARGET_MAX_PER_CHAPTER: usize = 6;
const MANUSCRIPT_ANALYSIS_INDEX_LIMIT: usize = 170_000;
const MANUSCRIPT_ANALYSIS_CHAPTER_LIMIT: usize = 2_400;
const MANUSCRIPT_ANALYSIS_ASSET_LIMIT: usize = 1_200;
const NOVEL_MAX_TOKENS_ENV: &str = "DEEPSEEK_NOVEL_MAX_TOKENS";
const NOVEL_CONTEXT_LIMIT_ENV: &str = "DEEPSEEK_NOVEL_CONTEXT_CHARS";
const NOVEL_PROMPT_LIMIT_ENV: &str = "DEEPSEEK_NOVEL_PROMPT_CHARS";
const NOVEL_CONTEXT_PROJECT_BUDGET: usize = 70_000;
const NOVEL_CONTEXT_GRAPH_BUDGET: usize = 22_000;
const NOVEL_CONTEXT_SUPPORT_BUDGET: usize = 28_000;
const NOVEL_CONTEXT_CURRENT_BUDGET: usize = 80_000;
const NOVEL_CONTEXT_RECENT_SUMMARY_BUDGET: usize = 20_000;
const NOVEL_CONTEXT_RECENT_CHAPTER_BUDGET: usize = 72_000;
const MEMORY_SOURCE_FINGERPRINT_TTL: Duration = Duration::from_secs(2);
const MEMORY_SOURCE_FINGERPRINT_DEPTH: usize = 2;
const PROJECT_MAP_RECENT_CHAPTERS: usize = 24;
const READINESS_RECENT_WINDOW: u32 = 8;
const READINESS_ARCHIVE_WINDOW: u32 = 20;
const MEMORY_QUERY_SEED_LIMIT: usize = 16;
const MEMORY_FRONTIER_EDGE_FANOUT: usize = 24;
const MEMORY_NEIGHBORHOOD_EDGE_LIMIT: usize = 120;
const MEMORY_MENTION_ALIAS_LIMIT: usize = 8;
const MEMORY_MENTION_DEFAULT_LIMIT: usize = 8;
const REQUIRED_MEMORY_SUMMARY_HEADINGS: &[&str] = &[
    "章节摘要",
    "人物状态变化",
    "人物知识边界",
    "新增事实锁",
    "事件时间线",
    "承诺推进",
    "资源变化",
    "地点与世界状态",
    "伏笔台账",
    "人物体验沉淀",
    "写法反馈",
    "后续风险",
    "CANDIDATE_MEMORY_UPDATES",
];
const MEMORY_SUMMARY_CONTEXT_HEADINGS: &[&str] = &[
    "人物知识边界",
    "新增事实锁",
    "承诺推进",
    "资源变化",
    "地点与世界状态",
    "伏笔台账",
    "后续风险",
];
const MEMORY_SUMMARY_CANON_HEADINGS: &[&str] = &[
    "人物知识边界",
    "新增事实锁",
    "承诺推进",
    "资源变化",
    "地点与世界状态",
    "伏笔台账",
];

#[derive(Args, Debug, Clone)]
pub struct NovelArgs {
    #[command(subcommand)]
    pub command: NovelCommand,
}

#[derive(Subcommand, Debug, Clone)]
pub enum NovelCommand {
    /// Create a local long-form novel project in the workspace.
    Init(NovelInitArgs),
    /// Show project status and chapter completion counts.
    Status,
    /// Show a read-only map of book assets, chapters, cards, promises, and memory.
    Map {
        /// Include the full chapter and memory file lists instead of the compact late-book view.
        #[arg(long, default_value_t = false)]
        full: bool,
    },
    /// Generate or refresh the book bible and master outline.
    Plan {
        /// Override model for this operation.
        #[arg(long)]
        model: Option<String>,
        /// Additional planning direction for this run.
        #[arg(long)]
        brief: Option<String>,
        /// Number of chapter beats to request in the first planning pass.
        #[arg(long, default_value_t = 30)]
        chapters: u16,
    },
    /// Generate a focused chapter brief before drafting.
    Brief {
        /// Chapter number, starting at 1.
        chapter: u32,
        /// Override model for this operation.
        #[arg(long)]
        model: Option<String>,
        /// Additional chapter direction for this run.
        #[arg(long)]
        direction: Option<String>,
        /// Overwrite an existing brief.md.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Build an optional craft note for one chapter without constraining the draft.
    Empower {
        /// Chapter number, starting at 1.
        chapter: u32,
        /// Override model for this operation.
        #[arg(long)]
        model: Option<String>,
        /// Additional craft direction for this run.
        #[arg(long)]
        direction: Option<String>,
        /// Overwrite an existing craft_plan.md.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Draft one chapter from the book bible and outline.
    Write {
        /// Chapter number, starting at 1.
        chapter: u32,
        /// Override model for this operation.
        #[arg(long)]
        model: Option<String>,
        /// Target chapter word count.
        #[arg(long, default_value_t = 3500)]
        words: u32,
        /// Additional chapter direction for this run.
        #[arg(long)]
        direction: Option<String>,
        /// Overwrite an existing draft.md.
        #[arg(long, default_value_t = false)]
        force: bool,
        /// Continue drafting even when deterministic context checks report blockers.
        #[arg(long, default_value_t = false)]
        allow_degraded_context: bool,
    },
    /// Audit a drafted or finalized chapter for continuity and prose quality.
    Audit {
        /// Chapter number, starting at 1.
        chapter: u32,
        /// Override model for this operation.
        #[arg(long)]
        model: Option<String>,
        /// Overwrite an existing audit.md.
        #[arg(long, default_value_t = true)]
        force: bool,
    },
    /// Revise one chapter using its audit and the current book state.
    Revise {
        /// Chapter number, starting at 1.
        chapter: u32,
        /// Override model for this operation.
        #[arg(long)]
        model: Option<String>,
        /// Additional revision direction.
        #[arg(long)]
        direction: Option<String>,
        /// Overwrite an existing final.md.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Show chapter-level differences without relying on the user's git repo.
    Diff {
        /// Chapter number, starting at 1.
        chapter: u32,
    },
    /// Restore the latest saved version for one chapter file.
    Undo {
        /// Chapter number, starting at 1.
        chapter: u32,
    },
    /// Extract reviewable continuity memory candidates after a chapter is drafted or finalized.
    Remember {
        /// Chapter number, starting at 1.
        chapter: u32,
        /// Override model for this operation.
        #[arg(long)]
        model: Option<String>,
        /// Overwrite an existing memory summary.
        #[arg(long, default_value_t = true)]
        force: bool,
        /// Immediately confirm and apply extracted candidates into memory ledgers.
        #[arg(long, default_value_t = false)]
        apply: bool,
    },
    /// Build, inspect, and query the long-form memory graph.
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
    /// Manage novel-specific evaluation fixtures without running real validation.
    Eval {
        #[command(subcommand)]
        command: NovelEvalCommand,
    },
    /// Plan reproducible long-run experiments without executing them.
    Experiment {
        #[command(subcommand)]
        command: NovelExperimentCommand,
    },
    /// Export finalized chapters, falling back to drafts when final text is absent.
    Export {
        /// Output file path. Defaults to exports/<title>.md or .txt.
        #[arg(long)]
        output: Option<PathBuf>,
        /// Export format.
        #[arg(long, value_enum, default_value_t = ExportFormat::Markdown)]
        format: ExportFormat,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum NovelEvalCommand {
    /// Archive a local failure sample as an original, reviewable regression fixture.
    CollectFailure {
        /// Failure category, for example resource_without_cost or knowledge_leak.
        #[arg(value_enum)]
        kind: FailureKind,
        /// Chapter number associated with the failure, when known.
        #[arg(long)]
        chapter: Option<u32>,
        /// Source text/report path inside the workspace.
        #[arg(long)]
        source: PathBuf,
        /// Expected deterministic signal, e.g. xianxia_resource_anchor.
        #[arg(long)]
        expected_signal: String,
        /// Expected repair direction.
        #[arg(long)]
        expected_revision: String,
        /// Optional short note for why this fixture exists.
        #[arg(long)]
        note: Option<String>,
    },
    /// Archive a non-trigger fixture for a deterministic signal.
    CollectNonTrigger {
        /// Signal that should not fire for this sample.
        #[arg(long)]
        signal: String,
        /// Source text/report path inside the workspace.
        #[arg(long)]
        source: PathBuf,
        /// Optional short note for why this fixture is a negative control.
        #[arg(long)]
        note: Option<String>,
    },
    /// Report fixture coverage by deterministic signal.
    Coverage,
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    KnowledgeLeak,
    PromiseDrift,
    FakeEmotion,
    ResourceWithoutCost,
    CombatPowerSpam,
    ReviseOverwriteVoice,
}

impl FailureKind {
    fn as_str(self) -> &'static str {
        match self {
            FailureKind::KnowledgeLeak => "knowledge_leak",
            FailureKind::PromiseDrift => "promise_drift",
            FailureKind::FakeEmotion => "fake_emotion",
            FailureKind::ResourceWithoutCost => "resource_without_cost",
            FailureKind::CombatPowerSpam => "combat_power_spam",
            FailureKind::ReviseOverwriteVoice => "revise_overwrite_voice",
        }
    }
}

#[derive(Subcommand, Debug, Clone)]
pub enum NovelExperimentCommand {
    /// Write a reproducible experiment config; this does not generate chapters.
    Plan {
        /// Experiment label.
        #[arg(long)]
        name: String,
        /// Target chapter count for a future run.
        #[arg(long, default_value_t = 10)]
        chapters: u32,
        /// Workflow variant such as no_memory, memory, archive, targeted_revise, xianxia_skill.
        #[arg(long, default_value = "memory")]
        workflow: String,
        /// Model id to record in the config.
        #[arg(long)]
        model: Option<String>,
        /// Temperature to record in the config.
        #[arg(long)]
        temperature: Option<f32>,
        /// Optional skill package name.
        #[arg(long)]
        skill: Option<String>,
        /// Overwrite an existing config with the same generated name.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Collect deterministic reports from existing chapters into experiments/reports/.
    Snapshot {
        /// First chapter to inspect.
        #[arg(long, default_value_t = 1)]
        start: u32,
        /// Last chapter to inspect. Defaults to current known chapter.
        #[arg(long)]
        end: Option<u32>,
        /// Optional run id to attach this snapshot to.
        #[arg(long)]
        run_id: Option<String>,
        /// Overwrite an existing generated snapshot path when possible.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
}

#[derive(Args, Debug, Clone)]
pub struct NovelInitArgs {
    /// Book title. Defaults to the workspace directory name.
    #[arg(long)]
    pub title: Option<String>,
    /// Primary genre, e.g. 都市重生, 玄幻, 科幻, 悬疑.
    #[arg(long)]
    pub genre: Option<String>,
    /// One-sentence premise or seed idea.
    #[arg(long)]
    pub premise: Option<String>,
    /// Target total word count.
    #[arg(long, default_value_t = 800_000)]
    pub target_words: u32,
    /// Project language tag.
    #[arg(long, default_value = "zh-CN")]
    pub language: String,
    /// Overwrite an existing book.toml and template files.
    #[arg(long, default_value_t = false)]
    pub force: bool,
}

#[derive(Subcommand, Debug, Clone)]
pub enum MemoryCommand {
    /// Rebuild memory/graph.json from project assets and chapter memory.
    Build,
    /// Show memory graph statistics and high-degree entities.
    Status,
    /// List imported RLM manuscript analysis reports and their staged candidates.
    Reports,
    /// List promise / foreshadowing lifecycle status grouped by state.
    Promises,
    /// Archive a finished chapter range into a compact stage memory summary.
    Archive {
        /// First chapter in the archived stage.
        start: u32,
        /// Last chapter in the archived stage.
        end: u32,
        /// Optional human-readable volume/stage label.
        #[arg(long)]
        label: Option<String>,
    },
    /// Run a deterministic continuity regression report over chapter windows.
    Regression {
        /// Chapter window size, commonly 10, 20, or 50.
        #[arg(default_value_t = 20)]
        window: u32,
        /// Save the report under memory/reports/.
        #[arg(long, default_value_t = false)]
        write: bool,
    },
    /// Validate memory/graph.json against the v2 narrative schema contract.
    Validate,
    /// Migrate or refresh memory schema files and generated graph compatibility.
    Migrate,
    /// Clean historical memory ledgers, candidate files, and summaries.
    Cleanup {
        /// Write cleaned files after backing up originals. Defaults to dry-run.
        #[arg(long, default_value_t = false)]
        apply: bool,
    },
    /// Show a token-efficient context packet for a future chapter.
    Context {
        /// Chapter number, starting at 1.
        chapter: u32,
        /// Maximum graph hops to traverse from chapter seeds.
        #[arg(long, default_value_t = 2)]
        depth: usize,
        /// Maximum graph nodes to include.
        #[arg(long, default_value_t = 24)]
        limit: usize,
    },
    /// Show the relationship neighborhood around one entity or chapter node.
    Query {
        /// Entity name, chapter node, or substring to search.
        query: String,
        /// Maximum graph hops to traverse.
        #[arg(long, default_value_t = 2)]
        depth: usize,
        /// Maximum graph nodes to include.
        #[arg(long, default_value_t = 24)]
        limit: usize,
    },
    /// Show what memories may be affected by changing one chapter.
    Impact {
        /// Chapter number, starting at 1.
        chapter: u32,
        /// Maximum graph hops to traverse.
        #[arg(long, default_value_t = 2)]
        depth: usize,
        /// Maximum graph nodes to include.
        #[arg(long, default_value_t = 32)]
        limit: usize,
    },
    /// Show a resource economy ledger from resource cards and object-state candidates.
    ResourceLedger {
        /// Optional chapter filter for pending/applied resource changes.
        #[arg(long)]
        chapter: Option<u32>,
    },
    /// List pending candidate memory updates extracted from audits or chapters.
    Candidates {
        /// Optional chapter filter.
        #[arg(long)]
        chapter: Option<u32>,
        /// Include already applied candidates.
        #[arg(long, default_value_t = false)]
        all: bool,
    },
    /// Confirm candidate memory updates and append them to memory ledgers.
    Apply {
        /// Optional chapter filter.
        #[arg(long)]
        chapter: Option<u32>,
        /// Preview ledger writes without changing files.
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
    /// Import an RLM manuscript analysis report into memory reports and candidates.
    ImportAnalysis {
        /// Markdown or JSON report produced by /analyze or an RLM manuscript pass.
        report: PathBuf,
    },
    /// Promote a local material source into a cited chapter brief or canon card draft.
    CiteMaterial {
        /// Material file under materials/.
        source: PathBuf,
        /// Target chapter brief number.
        #[arg(long)]
        chapter: Option<u32>,
        /// Target card path under cards/, bible/, or outline/.
        #[arg(long)]
        card: Option<PathBuf>,
    },
    /// List reference material citations already promoted into briefs or canon drafts.
    References,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ExportFormat {
    Markdown,
    Txt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BookManifest {
    title: String,
    genre: String,
    language: String,
    target_words: u32,
    created_at: String,
    updated_at: String,
    current_volume: u32,
    current_chapter: u32,
}

impl BookManifest {
    fn new(title: String, genre: Option<String>, language: String, target_words: u32) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            title,
            genre: genre.unwrap_or_else(|| "未定题材".to_string()),
            language,
            target_words,
            created_at: now.clone(),
            updated_at: now,
            current_volume: 1,
            current_chapter: 0,
        }
    }

    fn touch(&mut self) {
        self.updated_at = chrono::Utc::now().to_rfc3339();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct MemoryGraph {
    schema_version: u32,
    updated_at: String,
    nodes: Vec<MemoryNode>,
    edges: Vec<MemoryEdge>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    candidate_updates: Vec<MemoryUpdateCandidate>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct MemorySourceFingerprint {
    entries: Vec<MemorySourceEntry>,
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct MemorySourceEntry {
    path: String,
    is_dir: bool,
    modified: Option<SystemTime>,
}

#[derive(Debug, Clone)]
struct CachedMemorySourceFingerprint {
    graph_modified: Option<SystemTime>,
    fingerprint: MemorySourceFingerprint,
    checked_at: Instant,
}

static MEMORY_SOURCE_FINGERPRINT_CACHE: OnceLock<
    Mutex<BTreeMap<PathBuf, CachedMemorySourceFingerprint>>,
> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MemoryArchiveManifest {
    schema_version: u32,
    generated_at: String,
    stages: Vec<MemoryArchiveStage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MemoryArchiveStage {
    id: String,
    label: String,
    start_chapter: u32,
    end_chapter: u32,
    summary_path: String,
    chapters: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MemoryNode {
    id: String,
    kind: String,
    label: String,
    source: String,
    summary: String,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    state: serde_json::Value,
    hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MemoryEdge {
    kind: String,
    source: String,
    target: String,
    evidence: String,
    confidence: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MemoryUpdateCandidate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    chapter: Option<u32>,
    kind: String,
    target: String,
    change: String,
    evidence: String,
    confidence: f32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    affects: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    applied_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum QualitySeverity {
    Blocker,
    Major,
    Minor,
    SignalOnly,
}

impl QualitySeverity {
    fn as_str(self) -> &'static str {
        match self {
            QualitySeverity::Blocker => "blocker",
            QualitySeverity::Major => "major",
            QualitySeverity::Minor => "minor",
            QualitySeverity::SignalOnly => "signal_only",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum QualityCategory {
    Context,
    Continuity,
    Craft,
    ResourceEconomy,
    ReaderPromise,
}

impl QualityCategory {
    fn as_str(self) -> &'static str {
        match self {
            QualityCategory::Context => "context",
            QualityCategory::Continuity => "continuity",
            QualityCategory::Craft => "craft",
            QualityCategory::ResourceEconomy => "resource_economy",
            QualityCategory::ReaderPromise => "reader_promise",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct QualitySignal {
    code: &'static str,
    severity: QualitySeverity,
    category: QualityCategory,
    message: String,
}

impl QualitySignal {
    fn new(
        code: &'static str,
        severity: QualitySeverity,
        category: QualityCategory,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            severity,
            category,
            message: message.into(),
        }
    }

    fn legacy_issue(&self) -> String {
        format!("{}: {}", self.code, self.message)
    }

    fn revision_target(&self) -> String {
        format!(
            "{} [{}|{}]: {}",
            self.code,
            self.severity.as_str(),
            self.category.as_str(),
            self.message
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SceneGear {
    HighPressure,
    Normal,
    LowBreath,
    Missing,
}

impl SceneGear {
    fn as_str(self) -> &'static str {
        match self {
            SceneGear::HighPressure => "A档 · 高压爆发",
            SceneGear::Normal => "B档 · 正常推进",
            SceneGear::LowBreath => "C档 · 低速呼吸",
            SceneGear::Missing => "missing",
        }
    }
}

#[derive(Debug, Clone, Default)]
struct StyleDisciplineStats {
    zero_tolerance_hits: usize,
    budget_hits: usize,
    budget_limit: usize,
    ai_summary_hits: usize,
    ai_opening_hits: usize,
    ai_structure_hits: usize,
    rule_of_three_hits: usize,
    long_paragraph_runs: usize,
    short_paragraph_runs: usize,
    max_paragraph_chars: usize,
    min_paragraph_chars: usize,
}

#[derive(Debug, Clone, Default)]
struct SceneFunctionStats {
    goal_hits: usize,
    consequence_hits: usize,
    dialogue_exchanges: usize,
    dialogue_change_hits: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CharacterBehaviorRecord {
    chapter: Option<u32>,
    character: String,
    situation: String,
    choice: String,
    result: String,
    evidence: String,
    source: String,
}

#[derive(Debug, Clone)]
struct GraphNeighborhood {
    nodes: Vec<MemoryNode>,
    edges: Vec<MemoryEdge>,
}

#[derive(Debug, Clone)]
pub(crate) struct NovelInitOptions {
    pub title: Option<String>,
    pub genre: Option<String>,
    pub premise: Option<String>,
    pub target_words: u32,
    pub language: String,
    pub force: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct NovelInitOutcome {
    pub root: PathBuf,
    pub title: String,
    pub manifest_path: PathBuf,
    pub memory_graph_path: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct NovelWorkspaceSummary {
    pub root: PathBuf,
    pub title: String,
    pub genre: String,
    pub target_words: u32,
    pub current_volume: u32,
    pub current_chapter: u32,
    pub next_chapter: u32,
    pub chapters: usize,
    pub drafts: usize,
    pub finals: usize,
    pub audits: usize,
    pub summaries: usize,
    pub materials: usize,
    pub memory_reports: usize,
    pub memory_nodes: usize,
    pub memory_edges: usize,
    pub memory_schema_status: String,
    pub candidate_updates: usize,
    pub memory_graph_ready: bool,
    pub promise_statuses: Vec<(String, usize)>,
    pub open_promises: Vec<String>,
    pub relationship_changes: usize,
    pub state_changes: usize,
    pub relationship_previews: Vec<String>,
    pub state_change_previews: Vec<String>,
    pub readiness: NovelWorkflowReadiness,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct NovelWorkflowReadiness {
    pub next_action: String,
    pub blocked: bool,
    pub blockers: Vec<String>,
    pub warnings: Vec<String>,
    pub quality_gate: String,
    pub context_score: Option<i32>,
    pub pending_candidates: usize,
    pub candidate_pressure_total: usize,
    pub candidate_pressure_max_per_chapter: usize,
    pub candidate_target_per_chapter: usize,
    pub recent_summary_avg_chars: usize,
    pub recent_summary_max_chars: usize,
    pub recent_summary_overweight: usize,
    pub recent_summary_canon_sparse: usize,
    pub missing_recent_summaries: usize,
    pub missing_recent_audits: usize,
    pub missing_recent_finals: usize,
    pub archive_due: bool,
    pub regression_due: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct NovelAuditRiskSummary {
    pub blockers: usize,
    pub majors: usize,
    pub affected_nodes: usize,
    pub pending_candidates: usize,
    pub layered_counts: Vec<(String, usize)>,
    pub layered_previews: Vec<String>,
    pub risk_previews: Vec<String>,
    pub affected_previews: Vec<String>,
    pub candidate_previews: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct NovelMemorySnapshot {
    pub root: PathBuf,
    pub graph_path: PathBuf,
    pub graph_ready: bool,
    pub updated_at: Option<String>,
    pub nodes: usize,
    pub edges: usize,
    pub candidate_updates: usize,
    pub top_kinds: Vec<(String, usize)>,
    pub top_hubs: Vec<(String, usize)>,
    pub summaries: usize,
    pub reports: usize,
    pub facts: usize,
    pub events: usize,
    pub foreshadowing: usize,
    pub schema_status: String,
    pub promise_statuses: Vec<(String, usize)>,
    pub relationship_changes: usize,
    pub state_changes: usize,
    pub relationship_previews: Vec<String>,
    pub state_change_previews: Vec<String>,
    pub readiness: NovelWorkflowReadiness,
}

#[derive(Debug, Clone, Default)]
struct MemoryGraphHealth {
    promise_statuses: Vec<(String, usize)>,
    relationship_changes: usize,
    state_changes: usize,
    relationship_previews: Vec<String>,
    state_change_previews: Vec<String>,
}

#[derive(Debug, Clone)]
struct WriteChapterOptions {
    model: Option<String>,
    words: u32,
    direction: Option<String>,
    force: bool,
    allow_degraded_context: bool,
}

pub async fn run(config: &Config, args: NovelArgs, workspace: PathBuf) -> Result<()> {
    match args.command {
        NovelCommand::Init(args) => {
            let outcome = initialize_project(
                &workspace,
                NovelInitOptions {
                    title: args.title,
                    genre: args.genre,
                    premise: args.premise,
                    target_words: args.target_words,
                    language: args.language,
                    force: args.force,
                },
            )?;
            println!("Novel workspace initialized: {}", outcome.root.display());
            println!("Title: {}", outcome.title);
            println!("Manifest: {}", outcome.manifest_path.display());
            println!("Memory graph: {}", outcome.memory_graph_path.display());
            println!("Next: deepseek plan");
            Ok(())
        }
        NovelCommand::Status => print_status(&workspace),
        NovelCommand::Map { full } => {
            println!("{}", project_map_packet_with_options(&workspace, full)?);
            Ok(())
        }
        NovelCommand::Plan {
            model,
            brief,
            chapters,
        } => plan(config, &workspace, model, brief, chapters).await,
        NovelCommand::Brief {
            chapter,
            model,
            direction,
            force,
        } => build_chapter_brief(config, &workspace, chapter, model, direction, force).await,
        NovelCommand::Empower {
            chapter,
            model,
            direction,
            force,
        } => build_chapter_empowerment(config, &workspace, chapter, model, direction, force).await,
        NovelCommand::Write {
            chapter,
            model,
            words,
            direction,
            force,
            allow_degraded_context,
        } => {
            write_chapter(
                config,
                &workspace,
                chapter,
                WriteChapterOptions {
                    model,
                    words,
                    direction,
                    force,
                    allow_degraded_context,
                },
            )
            .await
        }
        NovelCommand::Audit {
            chapter,
            model,
            force,
        } => audit_chapter(config, &workspace, chapter, model, force).await,
        NovelCommand::Revise {
            chapter,
            model,
            direction,
            force,
        } => revise_chapter(config, &workspace, chapter, model, direction, force).await,
        NovelCommand::Diff { chapter } => {
            println!("{}", chapter_diff_packet(&workspace, chapter)?);
            Ok(())
        }
        NovelCommand::Undo { chapter } => {
            println!("{}", chapter_undo_from_workspace(&workspace, chapter)?);
            Ok(())
        }
        NovelCommand::Remember {
            chapter,
            model,
            force,
            apply,
        } => remember_chapter(config, &workspace, chapter, model, force, apply).await,
        NovelCommand::Memory { command } => memory_command(&workspace, command),
        NovelCommand::Eval { command } => novel_eval_command(&workspace, command),
        NovelCommand::Experiment { command } => novel_experiment_command(&workspace, command),
        NovelCommand::Export { output, format } => export_book(&workspace, output, format),
    }
}

pub(crate) fn initialize_project(
    workspace: &Path,
    options: NovelInitOptions,
) -> Result<NovelInitOutcome> {
    let outcome = init_project(
        workspace,
        options.title,
        options.genre,
        options.premise,
        options.target_words,
        options.language,
        options.force,
    )?;
    Ok(outcome)
}

fn init_project(
    workspace: &Path,
    title: Option<String>,
    genre: Option<String>,
    premise: Option<String>,
    target_words: u32,
    language: String,
    force: bool,
) -> Result<NovelInitOutcome> {
    let title = title
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| {
            workspace
                .file_name()
                .and_then(OsStr::to_str)
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("未命名长篇")
                .to_string()
        });
    let manifest_path = workspace.join(BOOK_MANIFEST);
    if manifest_path.exists() && !force {
        bail!(
            "Novel project already exists at {}. Use --force to overwrite templates.",
            manifest_path.display()
        );
    }

    for dir in [
        "bible",
        "craft",
        "cards/characters",
        "cards/world",
        "cards/locations",
        "cards/resources",
        "materials/sources",
        "materials/notes",
        "outline",
        "chapters",
        "memory/summaries",
        "memory/candidates",
        "memory/characters",
        "memory/relations",
        "eval/failures/knowledge_leak",
        "eval/failures/promise_drift",
        "eval/failures/fake_emotion",
        "eval/failures/resource_without_cost",
        "eval/failures/combat_power_spam",
        "eval/failures/revise_overwrite_voice",
        "eval/fixtures",
        "eval/reports",
        "eval/rubrics",
        "experiments/configs",
        "experiments/baselines",
        "experiments/runs",
        "experiments/reports",
        "leaderboard",
        "exports",
    ] {
        std::fs::create_dir_all(workspace.join(dir))
            .with_context(|| format!("failed to create {}", workspace.join(dir).display()))?;
    }

    let manifest = BookManifest::new(title.clone(), genre.clone(), language, target_words);
    write_text(
        &manifest_path,
        &toml::to_string_pretty(&manifest).context("failed to encode book.toml")?,
        true,
    )?;

    write_text(
        &workspace.join("bible/premise.md"),
        &format!(
            "# {}\n\n## 核心灵感\n\n{}\n\n## 题材\n\n{}\n\n## 目标\n\n- 目标字数：{}\n- 创作方式：长篇连续项目，所有设定以本目录文件为准。\n",
            title,
            premise.unwrap_or_else(
                || "在这里写下故事的核心钩子、主角欲望、主要矛盾和读者承诺。".to_string()
            ),
            genre.unwrap_or_else(|| "未定题材".to_string()),
            target_words
        ),
        force,
    )?;
    write_text(
        &workspace.join("bible/world.md"),
        "# 世界观\n\n## 基础规则\n\n## 势力与阶层\n\n## 禁止破坏的设定\n\n",
        force,
    )?;
    write_text(
        &workspace.join("bible/reader_promise.md"),
        "# 读者承诺\n\n## 类型期待\n\n## 爽点/情绪回报\n\n## 追读钩子\n\n## 不写什么\n\n",
        force,
    )?;
    write_text(
        &workspace.join("bible/style.md"),
        "# 文风契约\n\n- 叙事视角：\n- 叙事距离：贴近当前视角人物的误判、遮掩和选择，不站到全知旁白上替角色总结。\n- 节奏：场景推进优先，少解释，多行动；长段之后必须用短句、动作或对话换气。\n- 对话：服务人物关系和冲突，不做信息搬运；每段关键对话至少改变权力、信息、关系或选择之一。\n- 情绪：少直接命名情绪，多用动作、停顿、身体反应、物件处理和环境选择外化。\n- 禁忌：避免模板化总结、空泛鸡汤、机械转折、过度解释、统一段长、漂亮但无代价的比喻。\n",
        force,
    )?;
    write_text(
        &workspace.join("craft/human_texture.md"),
        DEFAULT_HUMAN_TEXTURE_GUIDE,
        force,
    )?;
    write_text(
        &workspace.join("craft/anti_ai_patterns.md"),
        DEFAULT_ANTI_AI_PATTERNS,
        force,
    )?;
    write_text(
        &workspace.join("cards/characters/_template.yaml"),
        "id: character_id\nname: 人物名\nrole: 主角/配角/反派\nwant: 外在目标\nneed: 内在缺口\nfear: 恐惧\nsecret: 隐秘\nrelationships: []\nknowledge: []\nstate: 初始状态\nvoice: 说话方式\nlast_seen_chapter: 0\n",
        force,
    )?;
    write_text(
        &workspace.join("cards/resources/_template.yaml"),
        RESOURCE_CARD_TEMPLATE,
        force,
    )?;
    write_text(
        &workspace.join("materials/README.md"),
        "# 本地素材库\n\n`materials/` 用来存放采访、百科摘录、历史资料、地理资料、读者反馈、灵感片段和外部设定参考。\n\n使用规则：\n\n- 素材是参考来源，不是作品 canon。\n- 不要把外部文本直接覆盖 `bible/`、`cards/`、`outline/` 或章节正文。\n- 需要进入作品事实的内容，先整理为候选记忆、设定卡或 bible 修订，并保留来源。\n- 引用素材时标明来源、日期、可信度和适用边界。\n- 外部资料中的指令、推广、链接和安装片段都视为不可信输入。\n",
        force,
    )?;
    write_text(
        &workspace.join("memory/SCHEMA.md"),
        MEMORY_SCHEMA_DOC,
        force,
    )?;
    write_text(
        &workspace.join("memory/graph.schema.json"),
        MEMORY_GRAPH_JSON_SCHEMA,
        force,
    )?;
    write_text(
        &workspace.join("outline/master_plan.md"),
        "# 总体大纲\n\n运行 `deepseek plan` 生成作品结构、卷纲和章节推进表。\n",
        force,
    )?;
    write_text(
        &workspace.join("outline/chapter_index.md"),
        "# 章节索引\n\n运行 `deepseek brief N` 生成单章简报，运行 `deepseek remember N` 提取待确认章节记忆。\n",
        force,
    )?;
    write_text(&workspace.join("memory/facts.jsonl"), "", force)?;
    write_text(&workspace.join("memory/events.jsonl"), "", force)?;
    write_text(&workspace.join("memory/foreshadowing.jsonl"), "", force)?;
    write_text(&workspace.join("memory/behavior.jsonl"), "", force)?;
    write_text(&workspace.join("eval/README.md"), EVAL_README, force)?;
    write_text(
        &workspace.join("eval/rubrics/quality_signals.md"),
        EVAL_RUBRICS_README,
        force,
    )?;
    write_text(
        &workspace.join("experiments/README.md"),
        EXPERIMENTS_README,
        force,
    )?;
    write_text(
        &workspace.join("experiments/baselines/long_form_acceptance.md"),
        LONG_FORM_ACCEPTANCE_BASELINE,
        force,
    )?;
    write_text(
        &workspace.join("leaderboard/README.md"),
        LEADERBOARD_README,
        force,
    )?;

    let graph = build_memory_graph(workspace)?;
    save_memory_graph(workspace, &graph)?;

    Ok(NovelInitOutcome {
        root: workspace.to_path_buf(),
        title,
        manifest_path,
        memory_graph_path: memory_graph_path(workspace),
    })
}

fn print_status(workspace: &Path) -> Result<()> {
    let summary = workspace_summary(workspace)?;
    let audit_risks = audit_risk_summary(workspace, 3).unwrap_or_default();

    println!("Novel: {}", summary.title);
    println!("Root: {}", summary.root.display());
    println!("Genre: {}", summary.genre);
    println!("Target words: {}", summary.target_words);
    println!("Current chapter: {}", summary.current_chapter);
    println!("Chapters: {}", summary.chapters);
    println!("Drafts: {}", summary.drafts);
    println!("Finals: {}", summary.finals);
    println!("Audits: {}", summary.audits);
    println!("Memory summaries: {}", summary.summaries);
    println!("Memory reports: {}", summary.memory_reports);
    println!("Local materials: {}", summary.materials);
    println!(
        "Memory graph: {}",
        if summary.memory_graph_ready {
            "ready"
        } else {
            "missing"
        }
    );
    println!("Memory nodes: {}", summary.memory_nodes);
    println!("Memory edges: {}", summary.memory_edges);
    println!("Memory schema: {}", summary.memory_schema_status);
    println!("Candidate memory updates: {}", summary.candidate_updates);
    println!(
        "Workflow gate: {}{}",
        summary.readiness.quality_gate,
        if summary.readiness.blocked {
            " (blocked)"
        } else {
            ""
        }
    );
    println!("Next action: {}", summary.readiness.next_action);
    println!(
        "Late memory pressure: pending {}, recent {}, max/chapter {}, summary avg {}, max {}, overlong {}, canon sparse {}",
        summary.readiness.pending_candidates,
        summary.readiness.candidate_pressure_total,
        summary.readiness.candidate_pressure_max_per_chapter,
        summary.readiness.recent_summary_avg_chars,
        summary.readiness.recent_summary_max_chars,
        summary.readiness.recent_summary_overweight,
        summary.readiness.recent_summary_canon_sparse
    );
    for blocker in &summary.readiness.blockers {
        println!("Workflow blocker: {blocker}");
    }
    for warning in &summary.readiness.warnings {
        println!("Workflow warning: {warning}");
    }
    if !summary.promise_statuses.is_empty() {
        println!(
            "Promise lifecycle: {}",
            format_promise_status_counts(&summary.promise_statuses)
        );
    }
    println!("Relationship changes: {}", summary.relationship_changes);
    println!("State changes: {}", summary.state_changes);
    for preview in &summary.relationship_previews {
        println!("Relationship: {preview}");
    }
    for preview in &summary.state_change_previews {
        println!("State change: {preview}");
    }
    println!(
        "Audit risks: {} blockers, {} majors, {} affected nodes, {} pending candidates",
        audit_risks.blockers,
        audit_risks.majors,
        audit_risks.affected_nodes,
        audit_risks.pending_candidates
    );
    for risk in &audit_risks.risk_previews {
        println!("Risk: {risk}");
    }
    if !audit_risks.layered_counts.is_empty() {
        println!(
            "Audit risk categories: {}",
            format_audit_layer_counts(&audit_risks.layered_counts)
        );
    }
    for risk in &audit_risks.layered_previews {
        println!("Layered risk: {risk}");
    }
    for affected in &audit_risks.affected_previews {
        println!("Affected: {affected}");
    }
    for candidate in &audit_risks.candidate_previews {
        println!("Candidate: {candidate}");
    }
    Ok(())
}

pub(crate) fn workspace_summary(workspace: &Path) -> Result<NovelWorkspaceSummary> {
    let root = find_project_root(workspace)?;
    let manifest = load_manifest(&root)?;
    let mut drafts = 0_usize;
    let mut finals = 0_usize;
    let mut audits = 0_usize;
    let chapters = collect_chapter_dirs(&root)?;
    for (_, chapter_dir) in &chapters {
        if chapter_dir.join("draft.md").is_file() {
            drafts += 1;
        }
        if chapter_dir.join("final.md").is_file() {
            finals += 1;
        }
        if chapter_dir.join("audit.md").is_file() {
            audits += 1;
        }
    }
    let summaries = count_files_with_extension(&root.join("memory/summaries"), "md")?;
    let memory_reports = count_files_with_extension(&root.join("memory/reports"), "md")?;
    let materials = collect_material_files(&root)?.len();
    let memory_graph_ready = memory_graph_path(&root).is_file();
    let (memory_nodes, memory_edges, memory_schema_status, graph_candidate_updates, graph_health) =
        if memory_graph_ready {
            load_memory_graph(&root)
                .map(|graph| {
                    let schema_status = memory_schema_validation_status(&graph);
                    let health = memory_graph_health(&graph);
                    (
                        graph.nodes.len(),
                        graph.edges.len(),
                        schema_status,
                        graph.candidate_updates.len(),
                        health,
                    )
                })
                .unwrap_or((
                    0,
                    0,
                    "invalid: failed to parse memory/graph.json".to_string(),
                    0,
                    MemoryGraphHealth::default(),
                ))
        } else {
            (
                0,
                0,
                "missing: run `deepseek memory build`".to_string(),
                0,
                MemoryGraphHealth::default(),
            )
        };
    let candidate_updates = collect_memory_update_candidates(&root)
        .map(|candidates| {
            candidates
                .into_iter()
                .filter(|candidate| !candidate_is_applied(candidate))
                .count()
        })
        .unwrap_or(graph_candidate_updates);
    let current_chapter = manifest.current_chapter.max(
        chapters
            .iter()
            .map(|(chapter, _)| *chapter)
            .max()
            .unwrap_or(0),
    );
    let next_chapter = current_chapter.saturating_add(1).max(1);
    let readiness = workflow_readiness(
        &root,
        &manifest,
        &chapters,
        current_chapter,
        next_chapter,
        memory_graph_ready,
        &memory_schema_status,
        candidate_updates,
    )?;
    let open_promises = foreshadowing_preview(&root, 3).unwrap_or_default();

    Ok(NovelWorkspaceSummary {
        root,
        title: manifest.title,
        genre: manifest.genre,
        target_words: manifest.target_words,
        current_volume: manifest.current_volume,
        current_chapter,
        next_chapter,
        chapters: chapters.len(),
        drafts,
        finals,
        audits,
        summaries,
        materials,
        memory_reports,
        memory_nodes,
        memory_edges,
        memory_schema_status,
        candidate_updates,
        memory_graph_ready,
        promise_statuses: graph_health.promise_statuses,
        open_promises,
        relationship_changes: graph_health.relationship_changes,
        state_changes: graph_health.state_changes,
        relationship_previews: graph_health.relationship_previews,
        state_change_previews: graph_health.state_change_previews,
        readiness,
    })
}

pub(crate) fn audit_risk_summary(
    workspace: &Path,
    preview_limit: usize,
) -> Result<NovelAuditRiskSummary> {
    let root = find_project_root(workspace)?;
    let mut summary = NovelAuditRiskSummary::default();
    for (chapter, dir) in collect_chapter_dirs(&root)? {
        let path = dir.join("audit.md");
        let Some(text) = read_optional_limited(&path, 24_000) else {
            continue;
        };
        collect_audit_risk_lines(&text, chapter, preview_limit, &mut summary);
        collect_layered_audit_risks(&text, chapter, preview_limit, &mut summary);
        collect_audit_affected_nodes(&text, chapter, preview_limit, &mut summary);
    }

    let candidates = memory_candidates_for_display(&root, None, false)?;
    summary.pending_candidates = candidates.len();
    for candidate in candidates.into_iter().take(preview_limit) {
        let chapter = candidate
            .chapter
            .map(|chapter| format!("chapter {chapter:03}"))
            .unwrap_or_else(|| "chapter unknown".to_string());
        summary.candidate_previews.push(format!(
            "{chapter} [{}] {} -> {}",
            candidate.kind,
            candidate.target,
            limit_chars(&candidate.change, 96)
        ));
    }
    Ok(summary)
}

fn workflow_readiness(
    root: &Path,
    manifest: &BookManifest,
    chapters: &[(u32, PathBuf)],
    current_chapter: u32,
    next_chapter: u32,
    memory_graph_ready: bool,
    memory_schema_status: &str,
    pending_candidates: usize,
) -> Result<NovelWorkflowReadiness> {
    let mut readiness = NovelWorkflowReadiness {
        candidate_target_per_chapter: MEMORY_CANDIDATE_TARGET_MAX_PER_CHAPTER,
        pending_candidates,
        ..NovelWorkflowReadiness::default()
    };

    let plan_ready = master_plan_ready(root);
    if !plan_ready {
        readiness
            .blockers
            .push("outline/master_plan.md is missing or still a template".to_string());
    }
    let next_dir = chapter_dir(root, next_chapter);
    let character_cards = non_template_files(collect_asset_files(&root.join("cards/characters"))?)
        .into_iter()
        .filter(|path| path.is_file())
        .count();
    if character_cards == 0 {
        readiness
            .blockers
            .push("character cards are missing".to_string());
    }

    let recent_summary_paths =
        recent_summary_paths(root, next_chapter, READINESS_RECENT_WINDOW as usize)?;
    let summary_quality = recent_memory_summary_quality(root, &recent_summary_paths)?;
    readiness.recent_summary_avg_chars = summary_quality.avg_chars;
    readiness.recent_summary_max_chars = summary_quality.max_chars;
    readiness.recent_summary_overweight = summary_quality.overweight_count;
    readiness.recent_summary_canon_sparse = summary_quality.canon_sparse_count;

    let candidate_pressure = memory_candidate_pressure(root, next_chapter);
    readiness.candidate_pressure_total = candidate_pressure.total;
    readiness.candidate_pressure_max_per_chapter = candidate_pressure.max_per_chapter;

    let recent_start = next_chapter.saturating_sub(READINESS_RECENT_WINDOW).max(1);
    for chapter in recent_start..next_chapter {
        let dir = chapter_dir(root, chapter);
        if !dir.exists() {
            continue;
        }
        if !dir.join("final.md").is_file() {
            readiness.missing_recent_finals += 1;
        }
        if !dir.join("audit.md").is_file() {
            readiness.missing_recent_audits += 1;
        }
        if !root
            .join("memory/summaries")
            .join(format!("{chapter:03}.md"))
            .is_file()
        {
            readiness.missing_recent_summaries += 1;
        }
    }

    let completed_since_archive = chapters
        .iter()
        .filter(|(chapter, dir)| {
            *chapter <= current_chapter
                && (dir.join("final.md").is_file() || dir.join("draft.md").is_file())
        })
        .count() as u32;
    let archive_count =
        collect_files_with_extensions(&root.join("memory/archives"), &["md", "json"])?.len() as u32;
    readiness.archive_due =
        completed_since_archive >= READINESS_ARCHIVE_WINDOW && archive_count == 0;
    readiness.regression_due = current_chapter >= 10
        && collect_files_with_extensions(&root.join("memory/reports"), &["md", "json"])?.is_empty();

    if !memory_graph_ready {
        readiness
            .blockers
            .push("memory graph missing; run `deepseek memory build`".to_string());
    } else if !memory_schema_status.starts_with("ok:") {
        readiness.blockers.push(format!(
            "memory graph schema not healthy: {memory_schema_status}"
        ));
    }
    if summary_quality.overweight_count > 0 {
        readiness.warnings.push(format!(
            "{} recent memory summary file(s) exceed {} chars",
            summary_quality.overweight_count, MEMORY_SUMMARY_OVERWEIGHT_CHARS
        ));
    }
    if summary_quality.canon_sparse_count > 0 {
        readiness.warnings.push(format!(
            "{} recent memory summary file(s) lack enough canon sections",
            summary_quality.canon_sparse_count
        ));
    }
    if candidate_pressure.max_per_chapter > MEMORY_CANDIDATE_TARGET_MAX_PER_CHAPTER {
        readiness.warnings.push(format!(
            "pending memory candidates exceed target: max {} in one chapter, target {}",
            candidate_pressure.max_per_chapter, MEMORY_CANDIDATE_TARGET_MAX_PER_CHAPTER
        ));
    }
    if readiness.missing_recent_summaries > 0 && next_chapter > 2 {
        readiness.warnings.push(format!(
            "{} recent chapter(s) have no memory summary",
            readiness.missing_recent_summaries
        ));
    }
    if readiness.missing_recent_audits > 0 && next_chapter > 2 {
        readiness.warnings.push(format!(
            "{} recent chapter(s) have no audit report",
            readiness.missing_recent_audits
        ));
    }
    if readiness.archive_due {
        readiness.warnings.push(format!(
            "{} completed chapter(s) without a stage archive; run `deepseek memory archive <start> <end>`",
            completed_since_archive
        ));
    }
    if readiness.regression_due {
        readiness.warnings.push(
            "no memory regression report found for a 10+ chapter project; run `deepseek memory regression 10 --write`"
                .to_string(),
        );
    }

    readiness.next_action = next_workflow_action(
        root,
        manifest,
        chapters,
        next_chapter,
        plan_ready,
        pending_candidates,
    );
    if let Some(chapter) = readiness
        .next_action
        .strip_prefix("deepseek brief ")
        .and_then(|raw| raw.parse::<u32>().ok())
    {
        let brief_dir = if chapter == next_chapter {
            next_dir.clone()
        } else {
            chapter_dir(root, chapter)
        };
        if !brief_dir.join("brief.md").is_file() {
            readiness
                .blockers
                .push(format!("chapter {chapter:03} brief is missing"));
        }
    }
    readiness.blockers.sort();
    readiness.blockers.dedup();
    readiness.warnings.sort();
    readiness.warnings.dedup();
    let estimated_signal_count = readiness.blockers.len() + readiness.warnings.len();
    let score = 100_i32 - (estimated_signal_count as i32 * 10)
        + i32::from(plan_ready) * 8
        + i32::from(next_dir.join("brief.md").is_file()) * 8
        + (recent_summary_paths.len().min(4) as i32 * 3)
        + (character_cards.min(6) as i32);
    readiness.context_score = Some(score.clamp(0, 100));
    readiness.blocked = !readiness.blockers.is_empty();
    readiness.quality_gate = if readiness.blocked {
        "blocked".to_string()
    } else if pending_candidates > 0
        || readiness.candidate_pressure_max_per_chapter > MEMORY_CANDIDATE_TARGET_MAX_PER_CHAPTER
        || readiness.recent_summary_overweight > 0
        || readiness.recent_summary_canon_sparse > 0
        || readiness.missing_recent_audits > 0
    {
        "needs-review".to_string()
    } else {
        "ready".to_string()
    };
    Ok(readiness)
}

fn master_plan_ready(root: &Path) -> bool {
    let master_plan = root.join("outline/master_plan.md");
    std::fs::read_to_string(&master_plan)
        .ok()
        .map(|text| text.len() > 80 && !text.contains("运行 `deepseek plan`"))
        .unwrap_or(false)
}

fn next_workflow_action(
    root: &Path,
    _manifest: &BookManifest,
    chapters: &[(u32, PathBuf)],
    next_chapter: u32,
    plan_ready: bool,
    pending_candidates: usize,
) -> String {
    if !plan_ready {
        return "deepseek plan".to_string();
    }
    for (chapter, dir) in chapters {
        if !dir.join("brief.md").is_file() {
            return format!("deepseek brief {chapter}");
        }
        if !dir.join("draft.md").is_file() && !dir.join("final.md").is_file() {
            return format!("deepseek write {chapter}");
        }
        if dir.join("draft.md").is_file() && !dir.join("audit.md").is_file() {
            return format!("deepseek audit {chapter}");
        }
        if dir.join("audit.md").is_file() && !dir.join("final.md").is_file() {
            return format!("deepseek revise {chapter}");
        }
        if !root
            .join("memory/summaries")
            .join(format!("{chapter:03}.md"))
            .is_file()
        {
            return format!("deepseek remember {chapter}");
        }
    }
    if pending_candidates > 0 {
        return "deepseek memory candidates && deepseek memory apply".to_string();
    }
    format!("deepseek brief {next_chapter}")
}

pub(crate) fn project_map_packet(workspace: &Path) -> Result<String> {
    project_map_packet_with_options(workspace, false)
}

pub(crate) fn project_map_packet_with_options(workspace: &Path, full: bool) -> Result<String> {
    let root = find_project_root(workspace)?;
    let manifest = load_manifest(&root)?;
    let chapters = collect_chapter_dirs(&root)?;
    let bible_files = collect_asset_files(&root.join("bible"))?;
    let craft_files = collect_asset_files(&root.join("craft"))?;
    let outline_files = collect_asset_files(&root.join("outline"))?;
    let character_cards = non_template_files(collect_asset_files(&root.join("cards/characters"))?);
    let world_cards = non_template_files(collect_asset_files(&root.join("cards/world"))?);
    let location_cards = non_template_files(collect_asset_files(&root.join("cards/locations"))?);
    let material_files = collect_material_files(&root)?;
    let summary_files = collect_files_with_extensions(&root.join("memory/summaries"), &["md"])?;
    let report_files = collect_files_with_extensions(&root.join("memory/reports"), &["md"])?;
    let candidate_files =
        collect_files_with_extensions(&root.join("memory/candidates"), &["json"])?;
    let candidates = collect_memory_update_candidates(&root)?;
    let summary = workspace_summary(&root)?;
    let graph = if memory_graph_path(&root).is_file() {
        load_memory_graph(&root).ok()
    } else {
        None
    };

    let mut drafts = 0_usize;
    let mut finals = 0_usize;
    let mut audits = 0_usize;
    for (_, dir) in &chapters {
        drafts += usize::from(dir.join("draft.md").is_file());
        finals += usize::from(dir.join("final.md").is_file());
        audits += usize::from(dir.join("audit.md").is_file());
    }

    let mut out = String::new();
    let _ = writeln!(out, "# Novel Project Map\n");
    let _ = writeln!(out, "- Title: {}", manifest.title);
    let _ = writeln!(out, "- Genre: {}", manifest.genre);
    let _ = writeln!(out, "- Language: {}", manifest.language);
    let _ = writeln!(out, "- Current volume: {}", manifest.current_volume);
    let _ = writeln!(out, "- Current chapter: {}", manifest.current_chapter);
    let _ = writeln!(out, "- Target words: {}", manifest.target_words);
    let _ = writeln!(out, "- Root: {}", root.display());
    let _ = writeln!(
        out,
        "- Workflow gate: {}{}",
        summary.readiness.quality_gate,
        if summary.readiness.blocked {
            " (blocked)"
        } else {
            ""
        }
    );
    let _ = writeln!(out, "- Next action: {}", summary.readiness.next_action);
    let _ = writeln!(
        out,
        "- Context score: {}",
        summary
            .readiness
            .context_score
            .map(|score| format!("{score}/100"))
            .unwrap_or_else(|| "n/a".to_string())
    );

    let _ = writeln!(out, "\n## Structure\n");
    let _ = writeln!(out, "- Chapters: {}", chapters.len());
    let _ = writeln!(out, "- Drafts: {drafts}");
    let _ = writeln!(out, "- Finals: {finals}");
    let _ = writeln!(out, "- Audits: {audits}");
    let _ = writeln!(out, "- Memory summaries: {}", summary_files.len());
    let _ = writeln!(out, "- Memory reports: {}", report_files.len());
    let _ = writeln!(out, "- Local materials: {}", material_files.len());

    let _ = writeln!(out, "\n## Book Assets\n");
    append_file_list(&mut out, &root, "Bible", &bible_files);
    append_file_list(&mut out, &root, "Craft", &craft_files);
    append_file_list(&mut out, &root, "Outline", &outline_files);

    let _ = writeln!(out, "\n## Cards\n");
    append_file_list(&mut out, &root, "Characters", &character_cards);
    append_file_list(&mut out, &root, "World", &world_cards);
    append_file_list(&mut out, &root, "Locations", &location_cards);

    let _ = writeln!(out, "\n## Local Materials\n");
    let _ = writeln!(
        out,
        "Materials are reference sources only; promote durable facts into bible/cards/memory before treating them as canon."
    );
    append_file_list(&mut out, &root, "Materials", &material_files);

    let _ = writeln!(out, "\n## Chapters\n");
    if chapters.is_empty() {
        let _ = writeln!(out, "- none");
    } else {
        let chapter_rows = chapter_map_rows(&root, &chapters)?;
        if full || chapter_rows.len() <= PROJECT_MAP_RECENT_CHAPTERS {
            for row in &chapter_rows {
                append_chapter_map_row(&mut out, row);
            }
        } else {
            let recent_start = chapter_rows
                .len()
                .saturating_sub(PROJECT_MAP_RECENT_CHAPTERS);
            let recent_chapters = &chapter_rows[recent_start..];
            let issue_rows = chapter_rows
                .iter()
                .filter(|row| row_has_map_issue(row))
                .collect::<Vec<_>>();
            let _ = writeln!(
                out,
                "- Compact view: showing {} recent chapters and {} issue chapters. Use `deepseek map --full` for all {} chapters.",
                recent_chapters.len(),
                issue_rows.len(),
                chapter_rows.len()
            );
            let _ = writeln!(out, "\n### Recent Chapters\n");
            for row in recent_chapters {
                append_chapter_map_row(&mut out, row);
            }
            if !issue_rows.is_empty() {
                let _ = writeln!(out, "\n### Missing / Attention\n");
                for row in issue_rows.into_iter().take(48) {
                    append_chapter_map_row(&mut out, row);
                }
            }
        }
    }

    let _ = writeln!(out, "\n## Memory\n");
    let _ = writeln!(
        out,
        "- Graph: {} ({})",
        if graph.is_some() { "ready" } else { "missing" },
        display_relative(&root, &memory_graph_path(&root))
    );
    if let Some(graph) = &graph {
        let _ = writeln!(out, "- Schema: {}", memory_schema_validation_status(graph));
    }
    let _ = writeln!(
        out,
        "- Nodes: {}",
        graph.as_ref().map(|graph| graph.nodes.len()).unwrap_or(0)
    );
    let _ = writeln!(
        out,
        "- Edges: {}",
        graph.as_ref().map(|graph| graph.edges.len()).unwrap_or(0)
    );
    let _ = writeln!(
        out,
        "- Facts: {}",
        count_jsonl_records(&root.join("memory/facts.jsonl"))?
    );
    let _ = writeln!(
        out,
        "- Events: {}",
        count_jsonl_records(&root.join("memory/events.jsonl"))?
    );
    let _ = writeln!(
        out,
        "- Foreshadowing: {}",
        count_jsonl_records(&root.join("memory/foreshadowing.jsonl"))?
    );
    let _ = writeln!(out, "- Candidate files: {}", candidate_files.len());
    let _ = writeln!(out, "- Candidate updates: {}", candidates.len());
    let _ = writeln!(
        out,
        "- Memory pressure: pending {}, recent {}, max/chapter {}, target/chapter {}",
        summary.readiness.pending_candidates,
        summary.readiness.candidate_pressure_total,
        summary.readiness.candidate_pressure_max_per_chapter,
        summary.readiness.candidate_target_per_chapter
    );
    let _ = writeln!(
        out,
        "- Summary density: avg {}, max {}, overlong {}, canon sparse {}",
        summary.readiness.recent_summary_avg_chars,
        summary.readiness.recent_summary_max_chars,
        summary.readiness.recent_summary_overweight,
        summary.readiness.recent_summary_canon_sparse
    );
    for blocker in &summary.readiness.blockers {
        let _ = writeln!(out, "- Workflow blocker: {blocker}");
    }
    for warning in summary.readiness.warnings.iter().take(8) {
        let _ = writeln!(out, "- Workflow warning: {warning}");
    }
    if full || chapters.len() <= PROJECT_MAP_RECENT_CHAPTERS {
        append_file_list(&mut out, &root, "Summaries", &summary_files);
        append_file_list(&mut out, &root, "Reports", &report_files);
        append_file_list(&mut out, &root, "Candidates", &candidate_files);
    } else {
        append_recent_file_list(
            &mut out,
            &root,
            "Recent summaries",
            &summary_files,
            PROJECT_MAP_RECENT_CHAPTERS,
        );
        append_recent_file_list(&mut out, &root, "Recent reports", &report_files, 8);
        append_recent_file_list(&mut out, &root, "Recent candidates", &candidate_files, 8);
    }

    let _ = writeln!(out, "\n## Promises / Foreshadowing\n");
    let promises = foreshadowing_preview(&root, 8)?;
    if promises.is_empty() {
        let _ = writeln!(out, "- none");
    } else {
        for line in promises {
            let _ = writeln!(out, "- {line}");
        }
    }

    Ok(out)
}

#[derive(Debug)]
struct ChapterMapRow {
    chapter: u32,
    path: String,
    markers: Vec<String>,
    has_draft: bool,
    has_final: bool,
    has_audit: bool,
    has_summary: bool,
}

fn chapter_map_rows(root: &Path, chapters: &[(u32, PathBuf)]) -> Result<Vec<ChapterMapRow>> {
    let mut rows = Vec::with_capacity(chapters.len());
    for (chapter, dir) in chapters {
        let has_brief = dir.join("brief.md").is_file();
        let has_craft_plan = dir.join("craft_plan.md").is_file();
        let has_draft = dir.join("draft.md").is_file();
        let has_audit = dir.join("audit.md").is_file();
        let has_final = dir.join("final.md").is_file();
        let has_summary = root
            .join("memory/summaries")
            .join(format!("{chapter:03}.md"))
            .is_file();
        let mut markers = Vec::new();
        if has_brief {
            markers.push("brief".to_string());
        }
        if has_craft_plan {
            markers.push("craft_plan".to_string());
        }
        if has_draft {
            markers.push("draft".to_string());
        }
        if has_audit {
            markers.push("audit".to_string());
        }
        if has_final {
            markers.push("final".to_string());
        }
        let versions = count_directory_files(&dir.join(".versions"))?;
        if versions > 0 {
            markers.push(format!("versions:{versions}"));
        }
        if has_summary {
            markers.push("summary".to_string());
        }
        if markers.is_empty() {
            markers.push("empty".to_string());
        }

        rows.push(ChapterMapRow {
            chapter: *chapter,
            path: display_relative(root, dir),
            markers,
            has_draft,
            has_final,
            has_audit,
            has_summary,
        });
    }
    Ok(rows)
}

fn append_chapter_map_row(out: &mut String, row: &ChapterMapRow) {
    let _ = writeln!(
        out,
        "- Chapter {:03}: {} ({})",
        row.chapter,
        row.path,
        row.markers.join(", ")
    );
}

fn row_has_map_issue(row: &ChapterMapRow) -> bool {
    !row.has_draft || !row.has_final || !row.has_audit || !row.has_summary
}

pub(crate) fn manuscript_analysis_index_packet(workspace: &Path, focus: &str) -> Result<String> {
    let root = find_project_root(workspace)?;
    let manifest = load_manifest(&root)?;
    let chapters = collect_chapter_dirs(&root)?;
    let graph = if memory_graph_path(&root).is_file() {
        load_memory_graph(&root).ok()
    } else {
        None
    };

    let mut out = String::new();
    let _ = writeln!(out, "# Novel Manuscript Analysis Index\n");
    let _ = writeln!(
        out,
        "This bounded packet is for RLM manuscript analysis. It indexes book assets and selected excerpts; local materials are reference only, not canon."
    );
    let _ = writeln!(out, "\n## Request\n");
    let _ = writeln!(out, "- Focus: {focus}");
    let _ = writeln!(out, "- Workspace: {}", root.display());
    let _ = writeln!(out, "- Title: {}", manifest.title);
    let _ = writeln!(out, "- Genre: {}", manifest.genre);
    let _ = writeln!(out, "- Language: {}", manifest.language);
    let _ = writeln!(out, "- Current volume: {}", manifest.current_volume);
    let _ = writeln!(out, "- Current chapter: {}", manifest.current_chapter);

    append_analysis_assets(
        &mut out,
        &root,
        "Bible",
        &collect_asset_files(&root.join("bible"))?,
        8,
    );
    append_analysis_assets(
        &mut out,
        &root,
        "Outline",
        &collect_asset_files(&root.join("outline"))?,
        8,
    );
    append_analysis_assets(
        &mut out,
        &root,
        "Character Cards",
        &non_template_files(collect_asset_files(&root.join("cards/characters"))?),
        24,
    );
    append_analysis_assets(
        &mut out,
        &root,
        "World Cards",
        &non_template_files(collect_asset_files(&root.join("cards/world"))?),
        16,
    );
    append_analysis_assets(
        &mut out,
        &root,
        "Location Cards",
        &non_template_files(collect_asset_files(&root.join("cards/locations"))?),
        16,
    );
    append_analysis_assets(
        &mut out,
        &root,
        "Local Materials (reference only)",
        &collect_material_files(&root)?,
        12,
    );

    let _ = writeln!(out, "\n## Memory Graph\n");
    if let Some(graph) = graph {
        let _ = writeln!(out, "- Schema version: {}", graph.schema_version);
        let _ = writeln!(
            out,
            "- Schema status: {}",
            memory_schema_validation_status(&graph)
        );
        let _ = writeln!(out, "- Nodes: {}", graph.nodes.len());
        let _ = writeln!(out, "- Edges: {}", graph.edges.len());
        let _ = writeln!(
            out,
            "- Pending candidate updates: {}",
            graph.candidate_updates.len()
        );
        for node in graph
            .nodes
            .iter()
            .filter(|node| {
                matches!(
                    node.kind.as_str(),
                    "character" | "location" | "event" | "relationship" | "promise" | "secret"
                ) && !node.id.starts_with("asset:")
            })
            .take(80)
        {
            let _ = writeln!(
                out,
                "- [{}] {} ({}) — {}",
                node.kind,
                node.label,
                node.id,
                limit_chars(&node.summary, 240)
            );
        }
    } else {
        let _ = writeln!(
            out,
            "- missing: run `deepseek memory build` to refresh memory/graph.json"
        );
    }

    append_analysis_ledger(&mut out, &root, "Facts", "memory/facts.jsonl", 40);
    append_analysis_ledger(&mut out, &root, "Events", "memory/events.jsonl", 40);
    append_analysis_ledger(
        &mut out,
        &root,
        "Foreshadowing",
        "memory/foreshadowing.jsonl",
        40,
    );

    let _ = writeln!(out, "\n## Chapters\n");
    if chapters.is_empty() {
        let _ = writeln!(out, "- none");
    } else {
        for (chapter, dir) in chapters {
            let markers = chapter_markers(&root, chapter, &dir)?;
            let _ = writeln!(out, "\n### Chapter {chapter:03} ({})", markers.join(", "));
            append_analysis_chapter_file(&mut out, &root, chapter, &dir, "brief.md", 900);
            append_analysis_chapter_file(&mut out, &root, chapter, &dir, "draft.md", 1_600);
            append_analysis_chapter_file(&mut out, &root, chapter, &dir, "final.md", 1_800);
            append_analysis_chapter_file(&mut out, &root, chapter, &dir, "audit.md", 1_200);
            let summary = root
                .join("memory/summaries")
                .join(format!("{chapter:03}.md"));
            if let Some(text) = read_optional_limited(&summary, MANUSCRIPT_ANALYSIS_CHAPTER_LIMIT) {
                let _ = writeln!(
                    out,
                    "- memory summary `{}`:\n{}",
                    display_relative(&root, &summary),
                    limit_chars(&text, 1_200)
                );
            }
            if out.len() >= MANUSCRIPT_ANALYSIS_INDEX_LIMIT {
                let _ = writeln!(
                    out,
                    "\n[analysis index truncated after chapter {chapter:03}; inspect later chapters with targeted RLM/file reads]"
                );
                break;
            }
        }
    }

    Ok(limit_chars(&out, MANUSCRIPT_ANALYSIS_INDEX_LIMIT))
}

fn append_analysis_assets(
    out: &mut String,
    root: &Path,
    label: &str,
    files: &[PathBuf],
    limit: usize,
) {
    let _ = writeln!(out, "\n## {label}\n");
    if files.is_empty() {
        let _ = writeln!(out, "- none");
        return;
    }
    for path in files.iter().take(limit) {
        let rel = display_relative(root, path);
        let _ = writeln!(out, "### {rel}");
        if let Some(text) = read_optional_limited(path, MANUSCRIPT_ANALYSIS_ASSET_LIMIT) {
            let _ = writeln!(
                out,
                "{}",
                limit_chars(&text, MANUSCRIPT_ANALYSIS_ASSET_LIMIT)
            );
        }
    }
    if files.len() > limit {
        let _ = writeln!(out, "- plus {} more file(s)", files.len() - limit);
    }
}

fn append_analysis_ledger(out: &mut String, root: &Path, label: &str, rel: &str, limit: usize) {
    let _ = writeln!(out, "\n## Memory Ledger: {label}\n");
    let path = root.join(rel);
    let Some(text) = read_optional_limited(&path, 24_000) else {
        let _ = writeln!(out, "- missing or empty: {rel}");
        return;
    };
    let mut count = 0_usize;
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if count >= limit {
            let _ = writeln!(out, "- plus more records...");
            break;
        }
        count += 1;
        let _ = writeln!(out, "- {}", limit_chars(line, 500));
    }
    if count == 0 {
        let _ = writeln!(out, "- empty: {rel}");
    }
}

fn append_analysis_chapter_file(
    out: &mut String,
    root: &Path,
    chapter: u32,
    dir: &Path,
    file: &str,
    limit: usize,
) {
    let path = dir.join(file);
    if let Some(text) = read_optional_limited(&path, limit) {
        let _ = writeln!(
            out,
            "- `{}` excerpt:\n{}",
            display_relative(root, &path),
            limit_chars(&text, limit)
        );
    } else if matches!(file, "draft.md" | "final.md") {
        let _ = writeln!(out, "- chapter {chapter:03} has no `{file}`");
    }
}

pub(crate) fn creative_relay_context_packet(workspace: &Path) -> Result<String> {
    let root = find_project_root(workspace)?;
    let manifest = load_manifest(&root)?;
    let chapters = collect_chapter_dirs(&root)?;
    let highest_chapter = chapters
        .iter()
        .map(|(chapter, _)| *chapter)
        .max()
        .unwrap_or(0);
    let active_chapter = manifest.current_chapter.max(highest_chapter);
    let next_chapter = active_chapter.saturating_add(1).max(1);
    let graph = if memory_graph_path(&root).is_file() {
        load_memory_graph(&root).ok()
    } else {
        None
    };
    let pending_candidates = collect_memory_update_candidates(&root)?
        .into_iter()
        .filter(|candidate| !candidate_is_applied(candidate))
        .count();

    let mut out = String::new();
    let _ = writeln!(out, "Novel creative relay context:");
    let _ = writeln!(out, "- Book: {}", manifest.title);
    let _ = writeln!(out, "- Genre: {}", manifest.genre);
    let _ = writeln!(out, "- Current volume: {}", manifest.current_volume);
    let _ = writeln!(out, "- Current chapter: {active_chapter}");
    let _ = writeln!(out, "- Next likely chapter: {next_chapter:03}");
    let _ = writeln!(
        out,
        "- Memory graph: {}",
        if graph.is_some() { "ready" } else { "missing" }
    );
    let _ = writeln!(
        out,
        "- Pending candidate memory updates: {pending_candidates}"
    );

    if let Some((chapter, dir)) = chapters.iter().rev().find(|(_, dir)| {
        dir.join("brief.md").is_file()
            || dir.join("draft.md").is_file()
            || dir.join("final.md").is_file()
            || dir.join("audit.md").is_file()
    }) {
        let markers = chapter_markers(&root, *chapter, dir)?;
        let _ = writeln!(
            out,
            "- Latest chapter artifact: Chapter {chapter:03} ({})",
            markers.join(", ")
        );
    }

    let character_states = graph
        .as_ref()
        .map(character_state_lines)
        .unwrap_or_default();
    if !character_states.is_empty() {
        let _ = writeln!(out, "\nCharacter states to preserve:");
        for line in character_states.into_iter().take(8) {
            let _ = writeln!(out, "- {line}");
        }
    }

    let promises = foreshadowing_preview(&root, 8)?;
    if !promises.is_empty() {
        let _ = writeln!(out, "\nOpen promises / foreshadowing to carry:");
        for promise in promises {
            let _ = writeln!(out, "- {promise}");
        }
    }

    let next_dir = chapter_dir(&root, next_chapter);
    let mut pressure = Vec::new();
    if !next_dir.join("brief.md").is_file() {
        pressure.push(format!("brief missing for chapter {next_chapter:03}"));
    }
    if !next_dir.join("draft.md").is_file() && !next_dir.join("final.md").is_file() {
        pressure.push(format!("draft/final missing for chapter {next_chapter:03}"));
    }
    if pending_candidates > 0 {
        pressure.push("review pending memory candidates before relying on graph state".to_string());
    }
    if !pressure.is_empty() {
        let _ = writeln!(out, "\nNext writing pressure:");
        for item in pressure {
            let _ = writeln!(out, "- {item}");
        }
    }

    Ok(out)
}

async fn plan(
    config: &Config,
    workspace: &Path,
    model: Option<String>,
    brief: Option<String>,
    chapters: u16,
) -> Result<()> {
    let root = find_project_root(workspace)?;
    let mut manifest = load_manifest(&root)?;
    let context = project_context(&root)?;
    let prompt = format!(
        "请为这部长篇小说生成可执行的创作方案。\n\n\
         目标：\n\
         - 形成一本长篇连载小说的总控文档。\n\
         - 给出世界观、主角团、关键配角、反派/阻力、主线、卷结构、前 {chapters} 章章节推进表。\n\
         - 明确事实锁、伏笔、人物弧线和每卷读者期待。\n\
         - 标注关键角色的初始欲望、恐惧、知识边界和状态更新节点。\n\
         - 给出每 10-15 章的追读钩子、信息释放点、爽点/情绪回报和伏笔回收计划。\n\
         - 输出 Markdown，只输出可保存到 `outline/master_plan.md` 的正文，不要解释你在做什么。\n\n\
         追加方向：{}\n\n\
         当前项目资料：\n\n{}",
        brief.unwrap_or_else(|| "无".to_string()),
        context
    );
    let output = complete(
        config,
        model.as_deref(),
        NOVEL_SYSTEM_PROMPT,
        &prompt,
        12_000,
        0.75,
    )
    .await?;
    write_text(&root.join("outline/master_plan.md"), &output, true)?;
    manifest.touch();
    save_manifest(&root, &manifest)?;
    println!(
        "Plan written: {}",
        root.join("outline/master_plan.md").display()
    );
    Ok(())
}

async fn build_chapter_brief(
    config: &Config,
    workspace: &Path,
    chapter: u32,
    model: Option<String>,
    direction: Option<String>,
    force: bool,
) -> Result<()> {
    let root = find_project_root(workspace)?;
    let chapter_dir = chapter_dir(&root, chapter);
    std::fs::create_dir_all(&chapter_dir)
        .with_context(|| format!("failed to create {}", chapter_dir.display()))?;
    let brief_path = chapter_dir.join("brief.md");
    if brief_path.exists() && !force {
        bail!(
            "{} already exists. Pass --force to overwrite it.",
            brief_path.display()
        );
    }

    let context = writing_context(&root, chapter)?;
    let prompt = format!(
        "请为第 {chapter:03} 章生成可执行的写作简报。\n\n\
         输出 Markdown，只输出可保存到 `chapters/{chapter:03}/brief.md` 的正文。\n\n\
         必须包含：\n\
         1. 本章功能：本章在主线/卷线中的作用。\n\
         2. 出场人物：每人的目标、情绪、知识边界、状态变化。\n\
         3. 场景推进：按场景列出目标、冲突、转折、结果。\n\
         4. 事实锁：本章不能违反的既有事实。\n\
         5. 信息释放与伏笔：新埋、推进、回收分别列出。\n\
         6. 追读钩子：章末必须留下的具体问题或期待。\n\
         7. 文风锚点：句式、节奏、禁忌和需要避免的 AI 腔。\n\n\
         追加方向：{}\n\n\
         创作上下文：\n\n{}",
        direction.unwrap_or_else(|| "按总纲推进。".to_string()),
        context
    );
    let output = complete(
        config,
        model.as_deref(),
        EDITOR_SYSTEM_PROMPT,
        &prompt,
        8_000,
        0.45,
    )
    .await?;
    write_text(&brief_path, &output, true)?;
    println!("Chapter brief written: {}", brief_path.display());
    Ok(())
}

async fn build_chapter_empowerment(
    config: &Config,
    workspace: &Path,
    chapter: u32,
    model: Option<String>,
    direction: Option<String>,
    force: bool,
) -> Result<()> {
    let root = find_project_root(workspace)?;
    let chapter_dir = chapter_dir(&root, chapter);
    std::fs::create_dir_all(&chapter_dir)
        .with_context(|| format!("failed to create {}", chapter_dir.display()))?;
    let craft_path = chapter_dir.join("craft_plan.md");
    if craft_path.exists() && !force {
        bail!(
            "{} already exists. Pass --force to overwrite it.",
            craft_path.display()
        );
    }

    let context = writing_context(&root, chapter)?;
    let prompt = format!(
        "请为第 {chapter:03} 章生成“可选写法札记”。\n\n\
         这个文件不是审查标准，也不是正文模板。它只是帮助后续写作想清楚人物、场景、语气和记忆关系。正文允许偏离它，只要不破坏项目事实、人物状态和读者承诺。\n\n\
         输出 Markdown，只输出可保存到 `chapters/{chapter:03}/craft_plan.md` 的正文。\n\n\
         必须包含：\n\
         1. 记忆关系：本章会触碰哪些人物、地点、规则、伏笔、前文事件。\n\
         2. 人物可能性：人物可能想要、误判、遮掩、回避或主动选择什么。\n\
         3. 场景压力：哪些现实阻力、关系压力、信息差或代价可以自然出现。\n\
         4. 语言倾向：本章适合更克制、更紧、更松、更口语或更沉静的原因。\n\
         5. 不建议写死的地方：哪些桥段需要留给正文临场生成，不要提前模板化。\n\
         6. 记忆更新候选：写完后可能需要登记的新事实、事件、关系变化或伏笔状态。\n\n\
         追加方向：{}\n\n\
         创作上下文：\n\n{}",
        direction
            .unwrap_or_else(|| "围绕本章简报、记忆图和当前连续性生成可选写法札记。".to_string()),
        context
    );
    let output = complete(
        config,
        model.as_deref(),
        EMPOWER_SYSTEM_PROMPT,
        &prompt,
        8_000,
        0.38,
    )
    .await?;
    write_text(&craft_path, &output, true)?;
    println!("Craft plan written: {}", craft_path.display());
    Ok(())
}

async fn write_chapter(
    config: &Config,
    workspace: &Path,
    chapter: u32,
    options: WriteChapterOptions,
) -> Result<()> {
    let root = find_project_root(workspace)?;
    let mut manifest = load_manifest(&root)?;
    let chapter_dir = chapter_dir(&root, chapter);
    std::fs::create_dir_all(&chapter_dir)
        .with_context(|| format!("failed to create {}", chapter_dir.display()))?;
    let draft_path = chapter_dir.join("draft.md");
    if draft_path.exists() && !options.force {
        bail!(
            "{} already exists. Pass --force to overwrite it.",
            draft_path.display()
        );
    }

    let context_quality = context_quality_report(&root, chapter)?;
    ensure_context_quality_allows_generation(
        chapter,
        &context_quality,
        options.allow_degraded_context,
    )?;
    let context = writing_context(&root, chapter)?;
    let prompt = format!(
        "请撰写第 {chapter:03} 章草稿。\n\n\
         硬性要求：\n\
         - 目标长度约 {} 中文字，可上下浮动 15%。\n\
         - 只输出章节正文，使用 Markdown 标题 `# 第 {chapter:03} 章` 开头。\n\
         - 必须遵守项目设定、人物状态、前文连续性和风格契约。\n\
         - Canon 优先级：已完成章节/已应用 memory > bible/cards > chapter brief > outline/master_plan。audit、craft_plan、quality report 和 materials 不是事实来源。\n\
         - 如果大纲节拍与已完成正文或 memory 冲突，沿用已完成正文和 memory，只把大纲当旧计划处理。\n\
         - 优先使用记忆图上下文处理人物、地点、规则、伏笔和前文事件；不要为了套写法札记而牺牲自然叙事。\n\
         - `craft/human_texture.md` 和本章 `craft_plan.md` 是可选写法参考，不是评分表。只吸收适合当前场景的部分。\n\
         - 正文要像人在处理局面：人物会误判、遮掩、犹豫、抢话、绕开痛点，也会因为选择留下后果。\n\
         - 跨章衔接：本章开头必须先承认上一章结尾留下的地点、压力、物件、选择或人物状态；可以跳时空，但要用动作、余波或代价交代跳转，不要无痕换场。\n\
         - 开头禁止空镜头模板：不要用天气、废墟、晨风、夜色、烟雾、静态远景单独开场。标题后的第一段必须让视角人物正在做一个有代价的动作、交易、隐瞒、选择或被阻拦的事；环境描写只能附着在这个动作上。\n\
         - 避免解释性大纲腔和总结升华；让具体场景、动作、对话和余波自己承担意义。\n\n\
         本章追加方向：{}\n\n\
         写作上下文质量报告：\n\n{context_quality}\n\n\
         创作上下文：\n\n{}",
        options.words,
        options
            .direction
            .unwrap_or_else(|| "按总纲推进".to_string()),
        context
    );
    let output = complete(
        config,
        options.model.as_deref(),
        NOVEL_SYSTEM_PROMPT,
        &prompt,
        14_000,
        0.82,
    )
    .await?;
    write_text(&draft_path, &output, true)?;
    manifest.current_chapter = manifest.current_chapter.max(chapter);
    manifest.touch();
    save_manifest(&root, &manifest)?;
    println!("Draft written: {}", draft_path.display());
    Ok(())
}

async fn audit_chapter(
    config: &Config,
    workspace: &Path,
    chapter: u32,
    model: Option<String>,
    force: bool,
) -> Result<()> {
    let root = find_project_root(workspace)?;
    let chapter_dir = chapter_dir(&root, chapter);
    let audit_path = chapter_dir.join("audit.md");
    if audit_path.exists() && !force {
        bail!(
            "{} already exists. Pass --force to overwrite it.",
            audit_path.display()
        );
    }
    let chapter_text = read_chapter_text(&chapter_dir)?;
    let context = writing_context(&root, chapter)?;
    let quality_report = chapter_quality_report(&root, chapter, &chapter_text)?;
    let prompt = format!(
        "请对第 {chapter:03} 章做记忆一致性诊断。\n\n\
         这不是打分审稿，也不是把正文套成统一写法。输出 Markdown 诊断报告，重点是维护长篇记忆图。\n\n\
         必须包含：\n\
         0. `## AUDIT_OVERVIEW`：用最多 5 条列出最高优先级问题，并标明来源类别；未发现实质问题时写 `- none`，不要硬批。\n\
         1. `## CONTINUITY_AUDIT`：人物状态、时间线、知识边界、地点状态、物件/资源归属、canon 冲突。\n\
         2. `## CRAFT_AUDIT`：人味、对白、节奏、战斗、世界观入戏、AI 腔。\n\
         3. `## MEMORY_CANDIDATE_AUDIT`：只判断哪些变化值得进入候选记忆，不在这里混写普通审稿意见。\n\
         4. `## READER_PROMISE_AUDIT`：本章承诺、追读压力、伏笔推进、回收和悬置。\n\
         5. `## PROTECTED_STRENGTHS`：最多 5 条列出修订时必须保护的有效段落、声口、场景顺序、动作余味或读者钩子；没有就写 `- none`。\n\
         兼容区块仍必须保留：\n\
         1. `## BLOCKER`：会破坏故事事实、人物可信度或主承诺的问题。\n\
         2. `## MAJOR`：会伤害连续性、伏笔、时间线或知识边界的问题。\n\
         3. `## MINOR`：局部可修问题。\n\
         4. `## AFFECTED_NODES`：受影响人物、地点、事件、伏笔、知识边界和后续章节。\n\
         5. `## CANDIDATE_MEMORY_UPDATES`：建议写入记忆图的候选项。\n\n\
         `## CANDIDATE_MEMORY_UPDATES` 中每条候选更新单独一行，优先输出 JSON 对象，供 Writer 写入 `memory/candidates/{chapter:03}.json` 等待确认：\n\
         {{\"chapter\":{chapter},\"kind\":\"knowledge|relationship|promise|event|location_state|object_state|memory\",\"target\":\"人物/地点/伏笔\",\"change\":\"变化\",\"evidence\":\"简短证据\",\"confidence\":0.80,\"affects\":[\"node_a\",\"node_b\"]}}\n\
         也可使用兼容文本格式：\n\
         - 候选记忆更新：target: 人物/地点/伏笔 change: 变化 evidence: 简短证据 confidence 0.80 affects: node_a,node_b\n\
         如果没有候选更新，写 `- none`。不要直接改写 `memory/facts.jsonl`、`memory/events.jsonl` 或 `memory/foreshadowing.jsonl`；候选项需要用户确认后再应用。\n\n\
         证据纪律：所有批评都必须引用正文、项目上下文或确定性质量报告中的依据；没有证据就写 `- none`。正向评价只能放入 `PROTECTED_STRENGTHS` 或对应审计说明，不能伪装成修订问题。\n\n\
         确定性章节质量报告：\n\n{quality_report}\n\n\
         项目上下文：\n\n{context}\n\n\
         待审章节：\n\n{chapter_text}",
    );
    let output = complete(
        config,
        model.as_deref(),
        EDITOR_SYSTEM_PROMPT,
        &prompt,
        8_000,
        0.35,
    )
    .await?;
    let backup = if audit_path.exists() && force {
        snapshot_chapter_file_before_write(&root, chapter, &audit_path, "pre-audit")?
    } else {
        None
    };
    write_text(&audit_path, &output, true)?;
    let candidate_count = write_memory_candidate_file(&root, chapter, &audit_path, &output)?;
    println!("Audit written: {}", audit_path.display());
    if let Some(backup) = backup {
        println!("Audit version saved: {}", backup.display());
    }
    println!("Memory candidates written: {candidate_count}");
    if candidate_count > 0 {
        println!("Review candidates: deepseek memory candidates --chapter {chapter}");
        println!("Apply after review: deepseek memory apply --chapter {chapter}");
    }
    Ok(())
}

async fn revise_chapter(
    config: &Config,
    workspace: &Path,
    chapter: u32,
    model: Option<String>,
    direction: Option<String>,
    force: bool,
) -> Result<()> {
    let root = find_project_root(workspace)?;
    let mut manifest = load_manifest(&root)?;
    let chapter_dir = chapter_dir(&root, chapter);
    let final_path = chapter_dir.join("final.md");
    if final_path.exists() && !force {
        bail!(
            "{} already exists. Pass --force to overwrite it.",
            final_path.display()
        );
    }
    let draft = read_required(&chapter_dir.join("draft.md"))?;
    let audit = read_optional_limited(&chapter_dir.join("audit.md"), CONTEXT_LIMIT)
        .unwrap_or_else(|| "暂无记忆诊断报告。".to_string());
    let revision_targets = targeted_revision_targets(&root, chapter, &draft, &audit)?;
    let protected_strengths = protected_revision_strengths(&audit);
    let context = writing_context(&root, chapter)?;
    let prompt = format!(
        "请根据记忆诊断报告定向修订第 {chapter:03} 章。\n\n\
         硬性要求：\n\
         - 只输出修订后的完整章节正文，使用 Markdown 标题 `# 第 {chapter:03} 章` 开头。\n\
         - 本次只围绕“Top 3 定向修订目标”修复；不要无差别整体重写。\n\
         - 保留应保留的剧情功能、场景顺序、人物语气和已经有效的段落。\n\
         - 不要输出修订说明，不要列清单。\n\
         - 优先修复记忆冲突和人物可信度问题；不要为了统一风格而重排所有场景。\n\
         - 可以参考 craft_plan，但不得把正文改成机械完成清单。\n\
         - 保留自然中文的顿挫、留白和人物临场反应。\n\n\
         - 跨章衔接：如果本章开头没有承接上一章结尾的地点、压力、物件、选择或人物状态，只做局部开头修复；不要因此重写全章。\n\n\
         Canon 优先级：已完成章节/已应用 memory > bible/cards > 本章 brief > outline/master_plan。audit 和 craft_plan 只用于定位问题或写法参考，不得直接变成新事实。\n\n\
         文件写入约束：\n\
         - 本次修订只允许更新 `chapters/{chapter:03}/final.md`。\n\
         - Writer 会在覆盖前保存章节版本备份；不要自行删除 `draft.md`、`audit.md`、`memory/` 或 `.versions/`。\n\
         - 修订完成后必须保留完整章节正文，不要只输出差异或修订说明。\n\n\
         Top 3 定向修订目标：\n\n{revision_targets}\n\n\
         修订保护项（不得洗掉）：\n\n{protected_strengths}\n\n\
         追加修订方向：{}\n\n\
         项目上下文：\n\n{context}\n\n\
         记忆诊断报告：\n\n{audit}\n\n\
         原草稿：\n\n{draft}",
        direction.unwrap_or_else(|| "优先修复记忆诊断报告中的连续性硬伤。".to_string()),
    );
    println!("Revision scope for chapter {chapter:03}:");
    println!("Read: {}", chapter_dir.join("draft.md").display());
    if chapter_dir.join("audit.md").is_file() {
        println!("Read: {}", chapter_dir.join("audit.md").display());
    }
    println!("Write: {}", final_path.display());
    println!(
        "Preserve versions under: {}",
        chapter_dir.join(".versions").display()
    );
    let output = complete(
        config,
        model.as_deref(),
        NOVEL_SYSTEM_PROMPT,
        &prompt,
        14_000,
        0.72,
    )
    .await?;
    let mut backup = snapshot_chapter_file_before_write(&root, chapter, &final_path, "pre-revise")?;
    if backup.is_none() {
        backup = snapshot_chapter_source_for_target(
            &root,
            chapter,
            &chapter_dir.join("draft.md"),
            "final.md",
            "draft-before-final",
        )?;
    }
    let snapshot_result = SnapshotRepo::open_or_init(&root)
        .and_then(|repo| repo.snapshot(&format!("novel-revise:{chapter:03}:pre")))
        .ok();
    write_text(&final_path, &output, true)?;
    manifest.current_chapter = manifest.current_chapter.max(chapter);
    manifest.touch();
    save_manifest(&root, &manifest)?;
    println!("Final chapter written: {}", final_path.display());
    if let Some(backup) = backup {
        println!("Chapter version saved: {}", backup.display());
    }
    if let Some(snapshot_id) = snapshot_result {
        println!("Workspace snapshot saved: {}", snapshot_id.as_str());
    }
    Ok(())
}

async fn remember_chapter(
    config: &Config,
    workspace: &Path,
    chapter: u32,
    model: Option<String>,
    force: bool,
    apply: bool,
) -> Result<()> {
    let root = find_project_root(workspace)?;
    let chapter_dir = chapter_dir(&root, chapter);
    let summary_path = root
        .join("memory/summaries")
        .join(format!("{chapter:03}.md"));
    if summary_path.exists() && !force {
        bail!(
            "{} already exists. Pass --force to overwrite it.",
            summary_path.display()
        );
    }
    let chapter_text = read_chapter_text(&chapter_dir)?;
    let context = project_context(&root)?;
    let prompt = chapter_memory_summary_prompt(chapter, &context, &chapter_text);
    let output = complete(
        config,
        model.as_deref(),
        EDITOR_SYSTEM_PROMPT,
        &prompt,
        8_000,
        0.25,
    )
    .await?;
    let output = normalize_memory_summary_output(&output);
    let backup = if summary_path.exists() && force {
        snapshot_memory_summary_before_write(&root, chapter, &summary_path, "pre-remember")?
    } else {
        None
    };
    write_text(&summary_path, &output, true)?;
    let candidate_count = write_memory_candidate_file_with_fallback(
        &root,
        chapter,
        &summary_path,
        &output,
        Some(&chapter_text),
    )?;
    println!("Memory summary written: {}", summary_path.display());
    if let Some(backup) = backup {
        println!("Memory summary version saved: {}", backup.display());
    }
    println!("Memory candidates written: {candidate_count}");
    if candidate_count > 0 {
        println!("Review candidates: deepseek memory candidates --chapter {chapter}");
        println!("Apply after review: deepseek memory apply --chapter {chapter}");
    }
    if apply {
        println!("{}", apply_memory_candidates(&root, Some(chapter), false)?);
    }
    Ok(())
}

fn chapter_memory_summary_prompt(chapter: u32, context: &str, chapter_text: &str) -> String {
    format!(
        "请为第 {chapter:03} 章提取长篇小说连续性记忆。\n\n\
         输出 Markdown，只输出可保存到 `memory/summaries/{chapter:03}.md` 的正文。正式记忆台账必须走候选确认流程，不要直接改写 `memory/facts.jsonl`、`memory/events.jsonl` 或 `memory/foreshadowing.jsonl`。\n\n\
         Canon 边界：\n\
         - 只把章节正文、已应用 memory 台账、bible/cards 中明确写出的内容当作事实来源。\n\
         - `audit.md`、`craft_plan.md`、质量报告和本地素材只能作为诊断/写法/参考，不得直接变成事实或候选记忆。\n\
         - 如果 `outline/master_plan.md` 与已完成章节或已应用 memory 冲突，以已完成章节和 memory 为准，并在“后续风险”标出冲突。\n\
         - 不确定、缺证据、只是审稿建议或只是写法评价的内容，不写入 `CANDIDATE_MEMORY_UPDATES`，候选区写 `- none`。\n\n\
         密度预算（必须遵守）：\n\
         - 全文目标 1500-{MEMORY_SUMMARY_TARGET_MAX_CHARS} 个中文字符；不得长于章节正文；宁可少写，不要复述正文。\n\
         - 每个分栏最多 3-5 条要点；每条只写可检索、可核验、会影响后文的变化。\n\
         - `写法反馈` 最多 3 条，只写写作风险，不得进入候选记忆。\n\
         - `CANDIDATE_MEMORY_UPDATES` 最多 3-6 条，只收录耐久事实变化；没有就写 `- none`。\n\
         - 章节摘要只负责因果定位，canon 以“人物知识边界 / 新增事实锁 / 承诺推进 / 资源变化 / 地点与世界状态 / 伏笔台账”为准。\n\n\
         必须使用以下 Markdown 分栏标题，标题文字不要改，方便后续检索和 memory graph 抽取：\n\
         1. `## 章节摘要`：200-400 字，保留因果链和角色选择。\n\
         2. `## 人物状态变化`：目标、关系、伤病、资源、情绪、身份位置的变化，最多 5 条。\n\
         3. `## 人物知识边界`：逐人物列出“本章前知道什么 / 本章新知道什么 / 仍不知道什么 / 误判什么”，严禁把读者或作者知道的事写成角色已知，最多 5 条。\n\
         4. `## 新增事实锁`：后续章节必须承认的事实，写明证据位置，最多 5 条。\n\
         5. `## 事件时间线`：按发生顺序列出事件、因果和结果，最多 5 条。\n\
         6. `## 承诺推进`：类型承诺、爽点承诺、人物关系承诺的推进或偏移，最多 5 条。\n\
         7. `## 资源变化`：钱、物品、法器、情报、人情债、伤势、地位等资源的获得/失去/控制权/代价，最多 5 条。\n\
         8. `## 地点与世界状态`：地点状态、势力反应、公开规则或隐藏规则的变化，最多 5 条。\n\
         9. `## 伏笔台账`：分为“新埋”“推进”“回收”“仍悬而未决”，每条给稳定 id 或短名，最多 5 条。\n\
         10. `## 人物体验沉淀`：本章人物做出的选择、付出的代价、遮掩的东西、未说出口的情绪，最多 4 条。\n\
         11. `## 写法反馈`：哪些人味正文策略有效，哪些地方仍有 AI 腔/解释过量风险，最多 3 条。\n\
         12. `## 后续风险`：容易遗忘或写崩的连续性点，最多 5 条。\n\n\
         末尾必须包含 `## CANDIDATE_MEMORY_UPDATES`。每条候选更新单独一行，优先输出 JSON 对象，供 Writer 写入 `memory/candidates/{chapter:03}.json` 等待确认：\n\
         {{\"chapter\":{chapter},\"kind\":\"knowledge|relationship|promise|event|location_state|object_state|memory\",\"target\":\"人物/地点/伏笔\",\"change\":\"变化\",\"evidence\":\"简短证据\",\"confidence\":0.80,\"affects\":[\"node_a\",\"node_b\"]}}\n\
         也可使用兼容文本格式：\n\
         - 候选记忆更新：target: 人物/地点/伏笔 change: 变化 evidence: 简短证据 confidence 0.80 affects: node_a,node_b\n\
         如果没有候选更新，写 `- none`。这些候选项稍后由 `deepseek memory candidates --chapter {chapter}` 复核，并由 `deepseek memory apply --chapter {chapter}` 确认写入正式台账。\n\n\
         项目上下文：\n\n{context}\n\n\
         章节正文：\n\n{chapter_text}"
    )
}

fn memory_command(workspace: &Path, command: MemoryCommand) -> Result<()> {
    let root = find_project_root(workspace)?;
    match command {
        MemoryCommand::Build => {
            let graph = build_memory_graph(&root)?;
            save_memory_graph(&root, &graph)?;
            println!(
                "Memory graph written: {}",
                memory_graph_path(&root).display()
            );
            print_memory_graph_status(&graph);
            Ok(())
        }
        MemoryCommand::Status => {
            let graph = load_or_build_memory_graph(&root)?;
            print_memory_graph_status(&graph);
            Ok(())
        }
        MemoryCommand::Reports => {
            println!("{}", memory_reports_packet(&root)?);
            Ok(())
        }
        MemoryCommand::Promises => {
            let graph = load_or_build_memory_graph(&root)?;
            println!("{}", memory_promises_packet(&root, &graph));
            Ok(())
        }
        MemoryCommand::Archive { start, end, label } => {
            println!(
                "{}",
                archive_memory_stage(&root, start, end, label.as_deref())?
            );
            Ok(())
        }
        MemoryCommand::Regression { window, write } => {
            println!("{}", memory_regression_report(&root, window, write)?);
            Ok(())
        }
        MemoryCommand::Validate => {
            println!("{}", memory_schema_validation_report(&root)?);
            Ok(())
        }
        MemoryCommand::Migrate => {
            println!("{}", migrate_memory_workspace(&root)?);
            Ok(())
        }
        MemoryCommand::Cleanup { apply } => {
            println!("{}", cleanup_memory_workspace(&root, apply)?);
            Ok(())
        }
        MemoryCommand::Context {
            chapter,
            depth,
            limit,
        } => {
            let graph = load_or_build_memory_graph(&root)?;
            let packet = memory_context_packet(&root, &graph, chapter, depth, limit)?;
            println!("{packet}");
            Ok(())
        }
        MemoryCommand::Query {
            query,
            depth,
            limit,
        } => {
            let graph = load_or_build_memory_graph(&root)?;
            let seeds = find_memory_seed_nodes(&graph, &query);
            if seeds.is_empty() {
                bail!("No memory graph nodes matched query: {query}");
            }
            let neighborhood = memory_neighborhood(&graph, &seeds, depth, limit);
            println!("{}", format_memory_neighborhood(&neighborhood));
            Ok(())
        }
        MemoryCommand::Impact {
            chapter,
            depth,
            limit,
        } => {
            println!("{}", memory_impact_packet(&root, chapter, depth, limit)?);
            Ok(())
        }
        MemoryCommand::ResourceLedger { chapter } => {
            println!("{}", resource_ledger_report(&root, chapter)?);
            Ok(())
        }
        MemoryCommand::Candidates { chapter, all } => {
            let candidates = memory_candidates_for_display(&root, chapter, all)?;
            if candidates.is_empty() {
                println!("No candidate memory updates found.");
            } else {
                println!("{}", format_memory_candidates(&candidates));
            }
            Ok(())
        }
        MemoryCommand::Apply { chapter, dry_run } => {
            let report = apply_memory_candidates(&root, chapter, dry_run)?;
            println!("{report}");
            Ok(())
        }
        MemoryCommand::ImportAnalysis { report } => {
            println!("{}", import_analysis_report_from_path(&root, &report)?);
            Ok(())
        }
        MemoryCommand::CiteMaterial {
            source,
            chapter,
            card,
        } => {
            println!(
                "{}",
                cite_material_from_path(&root, &source, chapter, card.as_deref())?
            );
            Ok(())
        }
        MemoryCommand::References => {
            println!("{}", material_references_packet(&root)?);
            Ok(())
        }
    }
}

fn export_book(workspace: &Path, output: Option<PathBuf>, format: ExportFormat) -> Result<()> {
    let root = find_project_root(workspace)?;
    let manifest = load_manifest(&root)?;
    let chapters = collect_chapter_dirs(&root)?;
    if chapters.is_empty() {
        bail!(
            "No chapters found under {}",
            root.join("chapters").display()
        );
    }

    let mut out = String::new();
    match format {
        ExportFormat::Markdown => {
            out.push_str(&format!("# {}\n\n", manifest.title));
        }
        ExportFormat::Txt => {
            out.push_str(&format!("{}\n\n", manifest.title));
        }
    }

    for (number, dir) in chapters {
        let text = read_optional_limited(&dir.join("final.md"), usize::MAX)
            .or_else(|| read_optional_limited(&dir.join("draft.md"), usize::MAX))
            .with_context(|| format!("chapter {number:03} has no final.md or draft.md"))?;
        out.push_str(text.trim());
        out.push_str("\n\n");
    }

    let default_ext = match format {
        ExportFormat::Markdown => "md",
        ExportFormat::Txt => "txt",
    };
    let output = output.unwrap_or_else(|| {
        root.join("exports").join(format!(
            "{}.{}",
            sanitize_file_name(&manifest.title),
            default_ext
        ))
    });
    write_text(&output, &out, true)?;
    println!("Export written: {}", output.display());
    Ok(())
}

async fn complete(
    config: &Config,
    model: Option<&str>,
    system: &str,
    prompt: &str,
    max_tokens: u32,
    temperature: f32,
) -> Result<String> {
    let client = DeepSeekClient::new(config)?;
    let selected_model = resolve_model(config, model);
    let reasoning_effort = if config.api_provider() == ApiProvider::Openrouter {
        config
            .reasoning_effort()
            .filter(|effort| !effort.trim().eq_ignore_ascii_case("auto"))
            .map(str::to_string)
    } else {
        config
            .reasoning_effort()
            .map(str::to_string)
            .or_else(|| Some("high".to_string()))
    };
    let max_tokens = novel_max_tokens_for_request(max_tokens);
    let request = MessageRequest {
        model: selected_model,
        messages: vec![Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: limit_novel_prompt(prompt),
                cache_control: None,
            }],
        }],
        max_tokens,
        system: Some(SystemPrompt::Text(system.to_string())),
        tools: None,
        tool_choice: None,
        metadata: None,
        thinking: None,
        reasoning_effort,
        stream: Some(false),
        temperature: Some(temperature),
        top_p: Some(0.92),
    };
    let response = client.create_message(request).await?;
    let mut output = String::new();
    for block in response.content {
        if let ContentBlock::Text { text, .. } = block {
            output.push_str(&text);
        }
    }
    let trimmed = output.trim();
    if trimmed.is_empty() {
        bail!("model returned empty output");
    }
    Ok(trimmed.to_string())
}

fn limit_novel_prompt(prompt: &str) -> String {
    let Some(limit) = std::env::var(NOVEL_PROMPT_LIMIT_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
    else {
        return prompt.to_string();
    };
    limit_chars(prompt, limit)
}

fn novel_max_tokens_for_request(requested: u32) -> u32 {
    let Some(limit) = std::env::var(NOVEL_MAX_TOKENS_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .filter(|value| *value > 0)
    else {
        return requested;
    };
    requested.min(limit)
}

fn resolve_model(config: &Config, model: Option<&str>) -> String {
    let selected = model
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| config.default_model());
    if selected.trim().eq_ignore_ascii_case("auto") {
        DEFAULT_TEXT_MODEL.to_string()
    } else {
        selected
    }
}

fn find_project_root(start: &Path) -> Result<PathBuf> {
    let mut current = if start.is_file() {
        start.parent().unwrap_or(start).to_path_buf()
    } else {
        start.to_path_buf()
    };
    loop {
        if current.join(BOOK_MANIFEST).is_file() {
            return Ok(current);
        }
        if !current.pop() {
            break;
        }
    }
    bail!(
        "No novel project found from {} upward. Run `deepseek init` first.",
        start.display()
    )
}

fn load_manifest(root: &Path) -> Result<BookManifest> {
    let path = root.join(BOOK_MANIFEST);
    let raw = read_required(&path)?;
    toml::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
}

fn save_manifest(root: &Path, manifest: &BookManifest) -> Result<()> {
    write_text(
        &root.join(BOOK_MANIFEST),
        &toml::to_string_pretty(manifest).context("failed to encode book.toml")?,
        true,
    )
}

fn project_context(root: &Path) -> Result<String> {
    let manifest = load_manifest(root)?;
    let mut sections = vec![format!(
        "# Project\n\nTitle: {}\nGenre: {}\nLanguage: {}\nTarget words: {}\nCurrent chapter: {}\n",
        manifest.title,
        manifest.genre,
        manifest.language,
        manifest.target_words,
        manifest.current_chapter
    )];
    sections.push(canon_source_context(root)?);
    for path in [
        "bible/premise.md",
        "bible/world.md",
        "bible/reader_promise.md",
        "bible/style.md",
        "memory/SCHEMA.md",
        "craft/human_texture.md",
        "craft/anti_ai_patterns.md",
        "outline/master_plan.md",
        "outline/chapter_index.md",
    ] {
        if let Some(text) = read_optional_limited(&root.join(path), CONTEXT_LIMIT) {
            sections.push(format!("## {path}\n\n{text}"));
        }
    }
    for dir in [
        "cards/characters",
        "cards/world",
        "cards/locations",
        "cards/resources",
    ] {
        for path in collect_asset_files(&root.join(dir))? {
            if is_template_asset(&path) {
                continue;
            }
            if let Some(text) = read_optional_limited(&path, 16_000) {
                sections.push(format!("## {}\n\n{text}", display_relative(root, &path)));
            }
        }
    }
    let material_files = collect_material_files(root)?;
    if !material_files.is_empty() {
        sections.push(
            "## Local materials boundary\n\nMaterials under `materials/` are reference sources only. They are not canon and must not override `book.toml`, `bible/`, `cards/`, `outline/`, chapters, or `memory/`. Promote durable facts into bible/cards/memory with source notes before relying on them.".to_string(),
        );
        for path in material_files.into_iter().take(12) {
            if let Some(text) = read_optional_limited(&path, 8_000) {
                sections.push(format!("## {}\n\n{text}", display_relative(root, &path)));
            }
        }
    }
    Ok(sections.join("\n\n"))
}

fn canon_source_context(root: &Path) -> Result<String> {
    let mut out = String::from(
        "## Canon Source Priority\n\n\
         - Applied memory ledgers and finished chapter text are the strongest continuity sources.\n\
         - `bible/` and non-template `cards/` define durable characters, world rules, locations, resources, and reader promises.\n\
         - `outline/master_plan.md` and `outline/chapter_index.md` are plans, not proof; if they conflict with finished chapters, summaries, or applied memory, treat the outline beat as stale.\n\
         - `audit.md`, `craft_plan.md`, quality reports, and `materials/` are non-canon until promoted into bible/cards/memory candidates and reviewed.\n",
    );

    let mut gaps = Vec::new();
    for path in ["bible/world.md", "bible/reader_promise.md"] {
        let text = std::fs::read_to_string(root.join(path)).unwrap_or_default();
        if !has_substantive_markdown_content(&text) {
            gaps.push(format!(
                "`{path}` has no substantive canon; do not invent durable rules from placeholders."
            ));
        }
    }

    for (dir, label) in [
        ("cards/characters", "character cards"),
        ("cards/world", "world/rule cards"),
        ("cards/locations", "location cards"),
    ] {
        if non_template_files(collect_asset_files(&root.join(dir))?)
            .into_iter()
            .filter(|path| path.is_file())
            .count()
            == 0
        {
            gaps.push(format!(
                "`{dir}` has no non-template {label}; rely on finished chapters and applied memory for continuity."
            ));
        }
    }

    if !gaps.is_empty() {
        out.push_str("\nCanon gap warnings:\n");
        for gap in gaps {
            let _ = writeln!(out, "- {gap}");
        }
    }

    Ok(out)
}

fn has_substantive_markdown_content(text: &str) -> bool {
    text.lines().any(|line| {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with('#')
            || trimmed == "---"
            || trimmed.eq_ignore_ascii_case("todo")
            || trimmed.eq_ignore_ascii_case("tbd")
        {
            return false;
        }
        let content = trimmed
            .trim_start_matches(['-', '*', ' ', '\t'])
            .trim()
            .trim_matches(['`', ':', '：'])
            .trim();
        content.chars().count() >= 8
    })
}

fn graph_context(root: &Path, chapter: u32) -> Result<Option<String>> {
    let path = memory_graph_path(root);
    if !path.is_file() {
        return Ok(None);
    }
    let graph = load_or_build_memory_graph(root)?;
    let packet = memory_context_packet(root, &graph, chapter, 2, 24)?;
    Ok(Some(packet))
}

fn writing_context(root: &Path, chapter: u32) -> Result<String> {
    let mut project_sections = vec![project_context(root)?];
    let mut graph_sections = Vec::new();
    let mut support_sections = Vec::new();
    let mut current_sections = Vec::new();
    let mut recent_summary_sections = Vec::new();
    let mut recent_chapter_sections = Vec::new();

    if let Some(packet) = graph_context(root, chapter)? {
        graph_sections.push(format!("## Long-form memory graph context\n\n{packet}"));
    }
    support_sections.push(scene_gear_context(root, chapter)?);
    if let Some(behaviors) = character_behavior_context(root, 5)? {
        support_sections.push(behaviors);
    }
    if let Some(examples) = reference_passage_context(root, chapter)? {
        support_sections.push(examples);
    }
    if let Some(bridge) = chapter_bridge_context(root, chapter) {
        current_sections.push(bridge);
    }
    let brief = chapter_dir(root, chapter).join("brief.md");
    if let Some(text) = read_optional_limited(&brief, CONTEXT_LIMIT) {
        current_sections.push(format!("## Current chapter brief\n\n{text}"));
    }
    let craft_plan = chapter_dir(root, chapter).join("craft_plan.md");
    if let Some(text) = read_optional_limited(&craft_plan, CONTEXT_LIMIT) {
        current_sections.push(format!("## Current chapter craft plan\n\n{text}"));
    }
    for path in recent_summary_paths(root, chapter, 8)? {
        if let Some(text) = recent_memory_summary_context(root, &path) {
            recent_summary_sections.push(text);
        }
    }
    let start = chapter.saturating_sub(3).max(1);
    for number in start..chapter {
        let dir = chapter_dir(root, number);
        let text = read_optional_limited(&dir.join("final.md"), RECENT_CHAPTER_LIMIT)
            .or_else(|| read_optional_limited(&dir.join("draft.md"), RECENT_CHAPTER_LIMIT));
        if let Some(text) = text {
            recent_chapter_sections.push(format!("## Recent chapter {number:03}\n\n{text}"));
        }
    }

    project_sections =
        trim_section_group_to_budget(project_sections, NOVEL_CONTEXT_PROJECT_BUDGET, true);
    graph_sections = trim_section_group_to_budget(graph_sections, NOVEL_CONTEXT_GRAPH_BUDGET, true);
    support_sections =
        trim_section_group_to_budget(support_sections, NOVEL_CONTEXT_SUPPORT_BUDGET, true);
    current_sections =
        trim_section_group_to_budget(current_sections, NOVEL_CONTEXT_CURRENT_BUDGET, true);
    recent_summary_sections = trim_section_group_to_budget(
        recent_summary_sections,
        NOVEL_CONTEXT_RECENT_SUMMARY_BUDGET,
        false,
    );
    recent_chapter_sections = trim_section_group_to_budget(
        recent_chapter_sections,
        NOVEL_CONTEXT_RECENT_CHAPTER_BUDGET,
        false,
    );

    let current_section_count = current_sections.len();
    let mut sections = Vec::new();
    sections.extend(project_sections);
    sections.extend(graph_sections);
    sections.extend(support_sections);
    sections.extend(recent_summary_sections);
    sections.extend(recent_chapter_sections);
    sections.extend(current_sections);
    Ok(limit_novel_context_prioritized(
        sections,
        current_section_count,
    ))
}

fn recent_memory_summary_context(root: &Path, path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let packet = compact_memory_summary_for_context(&raw);
    if packet.trim().is_empty() {
        return None;
    }
    Some(format!(
        "## {} (compact memory summary)\n\n{packet}",
        display_relative(root, path)
    ))
}

fn chapter_bridge_context(root: &Path, chapter: u32) -> Option<String> {
    if chapter <= 1 {
        return None;
    }
    let prev_chapter = chapter - 1;
    let previous = read_chapter_for_bridge(root, prev_chapter)?;
    let ending = chapter_bridge_ending_excerpt(&previous);
    if ending.trim().is_empty() {
        return None;
    }
    let current_opening = read_chapter_for_bridge(root, chapter)
        .as_deref()
        .and_then(first_prose_paragraph)
        .map(|opening| limit_chars(opening, CHAPTER_BRIDGE_EXCERPT_LIMIT));
    let ending_keywords = chapter_bridge_keywords(&ending);
    let mut out = String::new();
    out.push_str("## Chapter bridge continuity\n\n");
    let _ = writeln!(
        out,
        "- previous_chapter: {prev_chapter:03}; current_chapter: {chapter:03}"
    );
    if !ending_keywords.is_empty() {
        let _ = writeln!(out, "- ending_keywords: {}", ending_keywords.join(", "));
    }
    let _ = writeln!(
        out,
        "- rule: chapter {chapter:03} opening must account for the previous ending's location, active pressure, unresolved choice, and viewpoint state before moving to a new beat."
    );
    let _ = writeln!(out, "\n### Previous ending excerpt\n\n{ending}");
    if let Some(opening) = current_opening {
        let _ = writeln!(out, "\n### Current opening excerpt\n\n{opening}");
    }
    Some(out)
}

fn read_chapter_for_bridge(root: &Path, chapter: u32) -> Option<String> {
    let dir = chapter_dir(root, chapter);
    read_optional_limited(&dir.join("final.md"), RECENT_CHAPTER_LIMIT)
        .or_else(|| read_optional_limited(&dir.join("draft.md"), RECENT_CHAPTER_LIMIT))
}

fn chapter_bridge_ending_excerpt(text: &str) -> String {
    let paragraphs = prose_paragraphs(text);
    let mut selected = paragraphs.into_iter().rev().take(3).collect::<Vec<_>>();
    selected.reverse();
    limit_chars(&selected.join("\n\n"), CHAPTER_BRIDGE_EXCERPT_LIMIT)
}

fn chapter_bridge_keywords(text: &str) -> Vec<String> {
    let mut values = Vec::new();
    for term in CHAPTER_BRIDGE_KEY_TERMS {
        if text.contains(term) {
            values.push((*term).to_string());
            if values.len() >= CHAPTER_BRIDGE_KEYWORD_LIMIT {
                break;
            }
        }
    }
    values
}

#[derive(Debug, Clone, Default)]
struct ChapterBridgeQuality {
    has_previous: bool,
    previous_keywords: Vec<String>,
    opening_keywords: Vec<String>,
    keyword_overlap: usize,
    opening_has_transition: bool,
}

fn chapter_bridge_quality(root: &Path, chapter: u32, chapter_text: &str) -> ChapterBridgeQuality {
    if chapter <= 1 {
        return ChapterBridgeQuality::default();
    }
    let Some(previous) = read_chapter_for_bridge(root, chapter - 1) else {
        return ChapterBridgeQuality::default();
    };
    let ending = chapter_bridge_ending_excerpt(&previous);
    let opening = first_prose_paragraph(chapter_text).unwrap_or_default();
    let previous_keywords = chapter_bridge_keywords(&ending);
    let opening_keywords = chapter_bridge_keywords(opening);
    let previous_set = previous_keywords.iter().collect::<BTreeSet<_>>();
    let keyword_overlap = opening_keywords
        .iter()
        .filter(|keyword| previous_set.contains(keyword))
        .count();
    let opening_has_transition = count_terms(opening, CHAPTER_BRIDGE_TRANSITION_TERMS) > 0;
    ChapterBridgeQuality {
        has_previous: true,
        previous_keywords,
        opening_keywords,
        keyword_overlap,
        opening_has_transition,
    }
}

fn compact_memory_summary_for_context(raw: &str) -> String {
    let mut sections = Vec::new();
    for heading in MEMORY_SUMMARY_CONTEXT_HEADINGS {
        let Some(body) = markdown_section_body(raw, heading) else {
            continue;
        };
        let compact = compact_memory_summary_section(&body);
        if compact_memory_summary_section_is_useful(&compact) {
            sections.push(format!("### {heading}\n{compact}"));
        }
    }
    if sections.is_empty() {
        return compact_memory_summary_fallback(raw);
    }
    limit_chars(&sections.join("\n\n"), MEMORY_SUMMARY_CONTEXT_LIMIT)
}

fn compact_memory_summary_section(body: &str) -> String {
    let mut lines = Vec::new();
    let mut items = 0usize;
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if skip_summary_context_line(trimmed) {
            continue;
        }
        if trimmed.starts_with('#') {
            lines.push(trimmed.to_string());
            continue;
        }
        if items >= MEMORY_SUMMARY_CONTEXT_BULLET_LIMIT {
            break;
        }
        lines.push(trimmed.to_string());
        items += 1;
    }
    limit_chars(&lines.join("\n"), MEMORY_SUMMARY_CONTEXT_SECTION_LIMIT)
}

fn compact_memory_summary_section_is_useful(section: &str) -> bool {
    let trimmed = section.trim();
    !trimmed.is_empty()
        && !trimmed.eq_ignore_ascii_case("none")
        && !trimmed.eq_ignore_ascii_case("- none")
        && !trimmed.contains("generation omitted this required section")
}

fn compact_memory_summary_fallback(raw: &str) -> String {
    let mut lines = Vec::new();
    let mut excluded_level: Option<usize> = None;
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some((level, heading)) = markdown_heading_line(trimmed) {
            if summary_context_excluded_heading(heading) {
                excluded_level = Some(level);
                continue;
            }
            if excluded_level.is_some_and(|excluded| level <= excluded) {
                excluded_level = None;
            }
        }
        if excluded_level.is_some() || skip_summary_context_line(trimmed) {
            continue;
        }
        lines.push(line);
    }
    limit_chars(
        lines.join("\n").trim(),
        MEMORY_SUMMARY_CONTEXT_FALLBACK_LIMIT,
    )
}

fn skip_summary_context_line(trimmed: &str) -> bool {
    if trimmed.is_empty() {
        return false;
    }
    looks_like_memory_pollution(trimmed)
        || memory_candidate_from_line(0, "memory_summary_context", trimmed).is_some()
}

fn summary_context_excluded_heading(heading: &str) -> bool {
    markdown_heading_matches(heading, "写法反馈")
        || markdown_heading_matches(heading, "CANDIDATE_MEMORY_UPDATES")
        || markdown_heading_matches(heading, "MEMORY_CANDIDATE_AUDIT")
}

fn markdown_section_body(raw: &str, token: &str) -> Option<String> {
    let mut target_level = 0usize;
    let mut in_section = false;
    let mut lines = Vec::new();
    for line in raw.lines() {
        if let Some((level, heading)) = markdown_heading_line(line) {
            if in_section && level <= target_level {
                break;
            }
            if !in_section && markdown_heading_matches(heading, token) {
                in_section = true;
                target_level = level;
                continue;
            }
        }
        if in_section {
            lines.push(line);
        }
    }
    in_section
        .then(|| lines.join("\n").trim().to_string())
        .filter(|body| !body.is_empty())
}

fn markdown_heading_line(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim();
    let level = trimmed.chars().take_while(|ch| *ch == '#').count();
    if level == 0 {
        return None;
    }
    let heading = trimmed[level..].trim();
    if heading.is_empty() {
        None
    } else {
        Some((level, heading))
    }
}

fn limit_novel_context_prioritized(
    sections: Vec<String>,
    protected_tail_sections: usize,
) -> String {
    let limit = std::env::var(NOVEL_CONTEXT_LIMIT_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0);
    limit_novel_context_prioritized_with_limit(sections, protected_tail_sections, limit)
}

fn limit_novel_context_prioritized_with_limit(
    sections: Vec<String>,
    protected_tail_sections: usize,
    limit: Option<usize>,
) -> String {
    let Some(limit) = limit else {
        return sections.join("\n\n");
    };
    if sections.is_empty() {
        return String::new();
    }

    let reserve = 64usize;
    let per_section_limit = (limit / sections.len().max(1)).max(1_200);
    let mut trimmed = sections
        .into_iter()
        .map(|section| limit_chars(&section, per_section_limit))
        .collect::<Vec<_>>();
    let protected_start = trimmed.len().saturating_sub(protected_tail_sections);

    loop {
        let joined = trimmed.join("\n\n");
        if joined.len() <= limit {
            return joined;
        }
        if trimmed.len() <= 1 {
            return limit_chars(&joined, limit);
        }
        let current_len = joined.len();
        let overflow = current_len.saturating_sub(limit).saturating_add(reserve);
        let removable = protected_start.max(1);
        let remove_each = (overflow / removable).max(256);
        let mut changed = false;
        for section in trimmed.iter_mut().take(protected_start) {
            let len = section.len();
            if len <= 512 {
                continue;
            }
            let next_limit = len.saturating_sub(remove_each).max(512);
            if next_limit < len {
                *section = limit_chars(section, next_limit);
                changed = true;
            }
        }
        if !changed {
            return limit_chars_from_end(&joined, limit);
        }
    }
}

fn trim_section_group_to_budget(
    sections: Vec<String>,
    budget: usize,
    preserve_start: bool,
) -> Vec<String> {
    if budget == 0 || sections.is_empty() {
        return sections;
    }
    let mut total = sections.join("\n\n").len();
    if total <= budget {
        return sections;
    }
    let mut remaining = sections.len();
    sections
        .into_iter()
        .map(|section| {
            let share = (budget / remaining.max(1)).max(1_200);
            remaining = remaining.saturating_sub(1);
            if total <= budget || section.len() <= share {
                total = total.saturating_sub(section.len());
                return section;
            }
            total = total.saturating_sub(section.len());
            if preserve_start {
                limit_chars(&section, share)
            } else {
                limit_chars_from_end(&section, share)
            }
        })
        .collect()
}

fn limit_chars_from_end(raw: &str, limit: usize) -> String {
    if raw.len() <= limit {
        return raw.to_string();
    }
    let marker = "[...truncated from start...]\n\n";
    let available = limit.saturating_sub(marker.len()).max(1);
    let mut start = raw.len().saturating_sub(available);
    while start < raw.len() && !raw.is_char_boundary(start) {
        start += 1;
    }
    format!("{marker}{}", &raw[start..])
}

fn context_quality_report(root: &Path, chapter: u32) -> Result<String> {
    let manifest = load_manifest(root)?;
    let graph = load_or_build_memory_graph(root).ok();
    let brief = chapter_dir(root, chapter).join("brief.md").is_file();
    let craft_plan = chapter_dir(root, chapter).join("craft_plan.md").is_file();
    let recent_summary_paths = recent_summary_paths(root, chapter, 8)?;
    let summary_quality = recent_memory_summary_quality(root, &recent_summary_paths)?;
    let recent_summaries = recent_summary_paths.len();
    let candidate_pressure = memory_candidate_pressure(root, chapter);
    let recent_chapters = (chapter.saturating_sub(3).max(1)..chapter)
        .filter(|number| {
            let dir = chapter_dir(root, *number);
            dir.join("final.md").is_file() || dir.join("draft.md").is_file()
        })
        .count();
    let character_cards = non_template_files(collect_asset_files(&root.join("cards/characters"))?)
        .into_iter()
        .filter(|path| path.is_file())
        .count();
    let open_promises = graph
        .as_ref()
        .map(|graph| {
            graph
                .nodes
                .iter()
                .filter(|node| node.kind == "promise" && !node.id.starts_with("asset:"))
                .filter(|node| {
                    let status = state_string(node, &["status"])
                        .unwrap_or_else(|| "unknown".to_string())
                        .to_ascii_lowercase();
                    matches!(
                        status.as_str(),
                        "new" | "open" | "progress" | "advanced" | "advancing" | "unknown"
                    )
                })
                .count()
        })
        .unwrap_or(0);
    let memory_schema = graph
        .as_ref()
        .map(memory_schema_validation_status)
        .unwrap_or_else(|| "missing".to_string());
    let xianxia = xianxia_context_profile(root, &manifest)?;

    let mut signals = Vec::new();
    if !has_substantive_markdown_content(
        &std::fs::read_to_string(root.join("bible/world.md")).unwrap_or_default(),
    ) {
        signals.push(QualitySignal::new(
            "empty_world_bible",
            QualitySeverity::Major,
            QualityCategory::Context,
            "bible/world.md has no substantive canon",
        ));
    }
    if !has_substantive_markdown_content(
        &std::fs::read_to_string(root.join("bible/reader_promise.md")).unwrap_or_default(),
    ) {
        signals.push(QualitySignal::new(
            "empty_reader_promise_bible",
            QualitySeverity::Major,
            QualityCategory::ReaderPromise,
            "bible/reader_promise.md has no substantive canon",
        ));
    }
    if !brief {
        signals.push(QualitySignal::new(
            "missing_chapter_brief",
            QualitySeverity::Blocker,
            QualityCategory::Context,
            "missing chapter brief",
        ));
    }
    if character_cards == 0 {
        signals.push(QualitySignal::new(
            "missing_character_cards",
            QualitySeverity::Blocker,
            QualityCategory::Context,
            "missing character cards",
        ));
    }
    if recent_summaries == 0 && chapter > 1 {
        signals.push(QualitySignal::new(
            "missing_recent_summaries",
            QualitySeverity::Major,
            QualityCategory::Context,
            "missing recent memory summaries",
        ));
    }
    if summary_quality.overweight_count > 0 {
        signals.push(QualitySignal::new(
            "overlong_memory_summaries",
            QualitySeverity::Major,
            QualityCategory::Context,
            format!(
                "{} recent memory summary file(s) exceed {} chars; compact before relying on them as canon",
                summary_quality.overweight_count, MEMORY_SUMMARY_OVERWEIGHT_CHARS
            ),
        ));
    }
    if summary_quality.canon_sparse_count > 0 {
        signals.push(QualitySignal::new(
            "memory_summary_canon_sparse",
            QualitySeverity::Major,
            QualityCategory::Continuity,
            format!(
                "{} recent memory summary file(s) lack useful canon sections",
                summary_quality.canon_sparse_count
            ),
        ));
    }
    if candidate_pressure.max_per_chapter > MEMORY_CANDIDATE_TARGET_MAX_PER_CHAPTER {
        signals.push(QualitySignal::new(
            "memory_candidate_bloat",
            QualitySeverity::Major,
            QualityCategory::Continuity,
            format!(
                "pending memory candidates exceed target: max {} in one chapter, target {}",
                candidate_pressure.max_per_chapter, MEMORY_CANDIDATE_TARGET_MAX_PER_CHAPTER
            ),
        ));
    }
    if recent_chapters == 0 && chapter > 1 {
        signals.push(QualitySignal::new(
            "missing_nearby_chapter_text",
            QualitySeverity::Major,
            QualityCategory::Context,
            "missing nearby chapter text",
        ));
    }
    if open_promises == 0 {
        signals.push(QualitySignal::new(
            "missing_open_promises",
            QualitySeverity::Major,
            QualityCategory::ReaderPromise,
            "no open/progress promises visible",
        ));
    }
    if !memory_schema.starts_with("ok:") {
        signals.push(QualitySignal::new(
            "memory_graph_schema",
            QualitySeverity::Blocker,
            QualityCategory::Continuity,
            format!("memory graph {memory_schema}"),
        ));
    }
    if xianxia.active {
        if xianxia.world_cards == 0 {
            signals.push(QualitySignal::new(
                "xianxia_missing_world_cards",
                QualitySeverity::Major,
                QualityCategory::Context,
                "xianxia: missing world/rule cards",
            ));
        }
        if xianxia.resource_cards == 0 {
            signals.push(QualitySignal::new(
                "xianxia_missing_resource_cards",
                QualitySeverity::Major,
                QualityCategory::ResourceEconomy,
                "xianxia: missing resource cards",
            ));
        }
        if xianxia.resource_cards > 0 && xianxia.resource_value_cards == 0 {
            signals.push(QualitySignal::new(
                "xianxia_resource_card_value_anchor",
                QualitySeverity::Major,
                QualityCategory::ResourceEconomy,
                "xianxia: resource cards lack value/income/cost anchors",
            ));
        }
        if xianxia.resource_cards > 0 && xianxia.resource_control_cards == 0 {
            signals.push(QualitySignal::new(
                "xianxia_resource_card_control_anchor",
                QualitySeverity::Major,
                QualityCategory::ResourceEconomy,
                "xianxia: resource cards lack controller/obligation anchors",
            ));
        }
        if xianxia.realm_rule_hits == 0 {
            signals.push(QualitySignal::new(
                "xianxia_missing_realm_rules",
                QualitySeverity::Major,
                QualityCategory::Context,
                "xianxia: missing realm/cultivation rules",
            ));
        }
        if xianxia.faction_hits == 0 {
            signals.push(QualitySignal::new(
                "xianxia_missing_faction_pressure",
                QualitySeverity::Major,
                QualityCategory::Context,
                "xianxia: missing sect/faction pressure",
            ));
        }
        if xianxia.resource_anchor_hits == 0 {
            signals.push(QualitySignal::new(
                "xianxia_context_resource_anchor",
                QualitySeverity::Major,
                QualityCategory::ResourceEconomy,
                "xianxia: missing resource/economy anchor",
            ));
        }
        if xianxia.artifact_hits == 0 {
            signals.push(QualitySignal::new(
                "xianxia_missing_artifact_constraints",
                QualitySeverity::Major,
                QualityCategory::Context,
                "xianxia: missing artifact/spell constraints",
            ));
        }
    }

    let score = 100_i32 - (signals.len() as i32 * 12)
        + i32::from(brief) * 8
        + i32::from(craft_plan) * 4
        + (recent_summaries.min(4) as i32 * 3)
        + (recent_chapters.min(3) as i32 * 3)
        + (character_cards.min(6) as i32);
    let score = score.clamp(0, 100);

    let mut out = String::new();
    out.push_str("# ContextQualityReport\n\n");
    let _ = writeln!(out, "- score: {score}/100");
    let _ = writeln!(out, "- memory_schema: {memory_schema}");
    let _ = writeln!(
        out,
        "- brief: {}",
        if brief { "present" } else { "missing" }
    );
    let _ = writeln!(
        out,
        "- craft_plan: {}",
        if craft_plan { "present" } else { "missing" }
    );
    let _ = writeln!(out, "- character_cards: {character_cards}");
    let _ = writeln!(out, "- recent_summaries: {recent_summaries}");
    let _ = writeln!(
        out,
        "- recent_summary_chars: avg {}, max {}, overlong {}, canon_sparse {}",
        summary_quality.avg_chars,
        summary_quality.max_chars,
        summary_quality.overweight_count,
        summary_quality.canon_sparse_count
    );
    let _ = writeln!(
        out,
        "- recent_pending_candidates: total {}, max_per_chapter {}, target_per_chapter {}",
        candidate_pressure.total,
        candidate_pressure.max_per_chapter,
        MEMORY_CANDIDATE_TARGET_MAX_PER_CHAPTER
    );
    let _ = writeln!(out, "- recent_chapters: {recent_chapters}");
    let _ = writeln!(out, "- open_or_progress_promises: {open_promises}");
    let _ = writeln!(
        out,
        "- genre_profile: {}",
        if xianxia.active { "xianxia" } else { "general" }
    );
    if xianxia.active {
        out.push_str("- xianxia_context:\n");
        let _ = writeln!(out, "  - world_cards: {}", xianxia.world_cards);
        let _ = writeln!(out, "  - resource_cards: {}", xianxia.resource_cards);
        let _ = writeln!(
            out,
            "  - resource_cards_with_value_anchor: {}",
            xianxia.resource_value_cards
        );
        let _ = writeln!(
            out,
            "  - resource_cards_with_control_anchor: {}",
            xianxia.resource_control_cards
        );
        let _ = writeln!(out, "  - realm_rule_signals: {}", xianxia.realm_rule_hits);
        let _ = writeln!(out, "  - faction_signals: {}", xianxia.faction_hits);
        let _ = writeln!(
            out,
            "  - resource_anchor_signals: {}",
            xianxia.resource_anchor_hits
        );
        let _ = writeln!(out, "  - artifact_signals: {}", xianxia.artifact_hits);
    }
    out.push_str("- gaps:\n");
    if signals.is_empty() {
        out.push_str("  - none\n");
    } else {
        for signal in &signals {
            let _ = writeln!(out, "  - {}", signal.message);
        }
    }
    out.push_str("- structured_signals:\n");
    if signals.is_empty() {
        out.push_str("  - none\n");
    } else {
        for signal in &signals {
            let _ = writeln!(
                out,
                "  - code: {}, severity: {}, category: {}, message: {}",
                signal.code,
                signal.severity.as_str(),
                signal.category.as_str(),
                signal.message
            );
        }
    }
    Ok(out)
}

fn ensure_context_quality_allows_generation(
    chapter: u32,
    report: &str,
    allow_degraded_context: bool,
) -> Result<()> {
    let blockers = context_quality_blocker_lines(report);
    if blockers.is_empty() || allow_degraded_context {
        return Ok(());
    }
    bail!(
        "context quality blockers prevent drafting chapter {chapter:03}.\n\
         Fix the context issues first, or pass --allow-degraded-context to generate anyway.\n\n{}",
        blockers.join("\n")
    )
}

fn context_quality_blocker_lines(report: &str) -> Vec<String> {
    report
        .lines()
        .map(str::trim)
        .filter(|line| line.contains("severity: blocker"))
        .map(str::to_string)
        .collect()
}

#[derive(Debug, Default)]
struct MemorySummaryQuality {
    avg_chars: usize,
    max_chars: usize,
    overweight_count: usize,
    canon_sparse_count: usize,
}

fn recent_memory_summary_quality(_root: &Path, paths: &[PathBuf]) -> Result<MemorySummaryQuality> {
    let mut quality = MemorySummaryQuality::default();
    let mut total_chars = 0usize;
    for path in paths {
        let raw = read_required(path)?;
        let chars = raw.chars().count();
        total_chars += chars;
        quality.max_chars = quality.max_chars.max(chars);
        if chars > MEMORY_SUMMARY_OVERWEIGHT_CHARS {
            quality.overweight_count += 1;
        }
        if memory_summary_canon_signal_count(&raw) < 2 {
            quality.canon_sparse_count += 1;
        }
    }
    if !paths.is_empty() {
        quality.avg_chars = total_chars / paths.len();
    }
    Ok(quality)
}

fn memory_summary_canon_signal_count(raw: &str) -> usize {
    MEMORY_SUMMARY_CANON_HEADINGS
        .iter()
        .filter(|heading| {
            markdown_section_body(raw, heading)
                .map(|body| compact_memory_summary_section_is_useful(&body))
                .unwrap_or(false)
        })
        .count()
}

#[derive(Debug, Default)]
struct MemoryCandidatePressure {
    total: usize,
    max_per_chapter: usize,
}

fn memory_candidate_pressure(root: &Path, chapter: u32) -> MemoryCandidatePressure {
    let start = chapter.saturating_sub(8).max(1);
    let candidates = memory_candidates_for_display(root, None, false).unwrap_or_default();
    let mut counts = BTreeMap::<u32, usize>::new();
    for candidate in candidates {
        let Some(candidate_chapter) = candidate.chapter else {
            continue;
        };
        if candidate_chapter < start || candidate_chapter >= chapter {
            continue;
        }
        *counts.entry(candidate_chapter).or_default() += 1;
    }
    let total = counts.values().sum();
    let max_per_chapter = counts.values().copied().max().unwrap_or(0);
    MemoryCandidatePressure {
        total,
        max_per_chapter,
    }
}

fn chapter_quality_report(root: &Path, chapter: u32, chapter_text: &str) -> Result<String> {
    let manifest = load_manifest(root)?;
    let xianxia = xianxia_context_profile(root, &manifest)?;
    let graph = load_or_build_memory_graph(root).ok();
    let anchors = graph
        .as_ref()
        .map(|graph| {
            let mut set = BTreeSet::new();
            set.insert(chapter);
            regression_anchor_candidates(graph, &set)
        })
        .unwrap_or_default();
    let carry = score_anchor_carry_for_text(chapter_text, &anchors);
    let dialogue_lines = chapter_text.matches('“').count() + chapter_text.matches('"').count();
    let consequence_hits = count_terms(chapter_text, ANCHOR_CONSEQUENCE_TERMS);
    let action_hits = count_terms(chapter_text, ANCHOR_ACTION_TERMS);
    let payoff_hits = count_terms(chapter_text, ANCHOR_PAYOFF_TERMS);
    let xianxia_chapter = if xianxia.active {
        Some(xianxia_chapter_profile(chapter_text))
    } else {
        None
    };
    let generic_hits = count_terms(
        chapter_text,
        &[
            "然而",
            "与此同时",
            "值得注意的是",
            "总之",
            "空气仿佛凝固",
            "眼神复杂",
            "难以言说",
        ],
    );
    let length = chapter_text.chars().count();
    let style_stats = style_discipline_stats(chapter_text);
    let scene_stats = scene_function_stats(chapter_text);
    let bridge_quality = chapter_bridge_quality(root, chapter, chapter_text);
    let scene_gear = detect_scene_gear(root, chapter)?;
    let viewpoint_leaks = viewpoint_boundary_leaks(chapter_text);
    let mut signals = Vec::<QualitySignal>::new();
    if length < 800 {
        signals.push(QualitySignal::new(
            "length_compliance",
            QualitySeverity::SignalOnly,
            QualityCategory::Craft,
            "chapter text is short; verify this is intentional",
        ));
    }
    if dialogue_lines == 0 {
        signals.push(QualitySignal::new(
            "dialogue_function",
            QualitySeverity::Minor,
            QualityCategory::Craft,
            "no visible dialogue markers",
        ));
    }
    if consequence_hits == 0 {
        signals.push(QualitySignal::new(
            "scene_causality",
            QualitySeverity::Major,
            QualityCategory::Continuity,
            "no consequence/decision terms detected",
        ));
    }
    if payoff_hits == 0 {
        signals.push(QualitySignal::new(
            "promise_progress",
            QualitySeverity::Major,
            QualityCategory::ReaderPromise,
            "no promise/payoff pressure terms detected",
        ));
    }
    if carry.anchor_count > 0 && carry.carried_count == 0 {
        signals.push(QualitySignal::new(
            "anchor_carry",
            QualitySeverity::Major,
            QualityCategory::Continuity,
            "anchors are not carried by action/dialogue/consequence",
        ));
    }
    if generic_hits > 3 {
        signals.push(QualitySignal::new(
            "anti_ai_prose",
            QualitySeverity::Minor,
            QualityCategory::Craft,
            "repeated generic transition/atmosphere signals",
        ));
    }
    if style_stats.zero_tolerance_hits > 0 {
        signals.push(QualitySignal::new(
            "style_zero_tolerance_terms",
            QualitySeverity::Major,
            QualityCategory::Craft,
            format!(
                "{} zero-tolerance style term(s) detected",
                style_stats.zero_tolerance_hits
            ),
        ));
    }
    if style_stats.budget_hits > style_stats.budget_limit {
        signals.push(QualitySignal::new(
            "style_budget_terms",
            QualitySeverity::Minor,
            QualityCategory::Craft,
            format!(
                "budget terms {} exceed limit {}",
                style_stats.budget_hits, style_stats.budget_limit
            ),
        ));
    }
    if style_stats.ai_summary_hits > 0 {
        signals.push(QualitySignal::new(
            "style_ai_summary_sentence",
            QualitySeverity::Major,
            QualityCategory::Craft,
            "AI-style summary/elevation sentence detected",
        ));
    }
    if style_stats.ai_structure_hits > 0 {
        signals.push(QualitySignal::new(
            "style_ai_structure",
            QualitySeverity::Minor,
            QualityCategory::Craft,
            "AI-style structural phrase detected",
        ));
    }
    if style_stats.rule_of_three_hits > 2 {
        signals.push(QualitySignal::new(
            "style_rule_of_three",
            QualitySeverity::Minor,
            QualityCategory::Craft,
            "repeated three-part rhetorical list pattern detected",
        ));
    }
    if style_stats.ai_opening_hits > 0 {
        signals.push(QualitySignal::new(
            "style_ai_opening",
            QualitySeverity::Major,
            QualityCategory::Craft,
            "opening starts with static atmosphere before viewpoint action",
        ));
    }
    if style_stats.long_paragraph_runs > 0 || style_stats.short_paragraph_runs > 0 {
        signals.push(QualitySignal::new(
            "style_paragraph_rhythm",
            QualitySeverity::Minor,
            QualityCategory::Craft,
            "paragraph length rhythm violates discipline thresholds",
        ));
    }
    if !viewpoint_leaks.is_empty() {
        signals.push(QualitySignal::new(
            "viewpoint_inner_state_boundary",
            QualitySeverity::Major,
            QualityCategory::Continuity,
            "possible non-viewpoint inner-state narration detected",
        ));
    }
    if scene_gear == SceneGear::Missing {
        signals.push(QualitySignal::new(
            "scene_gear_missing",
            QualitySeverity::Major,
            QualityCategory::Context,
            "chapter brief/craft plan lacks A/B/C scene gear",
        ));
    }
    if bridge_quality.has_previous
        && !bridge_quality.previous_keywords.is_empty()
        && bridge_quality.keyword_overlap == 0
        && !bridge_quality.opening_has_transition
    {
        signals.push(QualitySignal::new(
            "chapter_bridge_opening",
            QualitySeverity::Major,
            QualityCategory::Continuity,
            "opening does not visibly bridge previous ending pressure/location/object/state",
        ));
    }
    if length >= 800 && scene_stats.goal_hits == 0 {
        signals.push(QualitySignal::new(
            "scene_goal_missing",
            QualitySeverity::Major,
            QualityCategory::Craft,
            "no immediate scene goal / avoidance / concealment signal detected",
        ));
    }
    if dialogue_lines > 0
        && scene_stats.dialogue_change_hits == 0
        && scene_stats.consequence_hits == 0
    {
        signals.push(QualitySignal::new(
            "dialogue_no_state_change",
            QualitySeverity::Major,
            QualityCategory::Craft,
            "dialogue appears without visible information, power, relationship, or choice change",
        ));
    }
    if scene_gear == SceneGear::HighPressure && style_stats.max_paragraph_chars > 120 {
        signals.push(QualitySignal::new(
            "scene_gear_a_paragraph_too_long",
            QualitySeverity::Minor,
            QualityCategory::Craft,
            "A档 high-pressure scene has paragraphs longer than 120 chars",
        ));
    }
    if scene_gear == SceneGear::LowBreath && style_stats.short_paragraph_runs > 0 {
        signals.push(QualitySignal::new(
            "scene_gear_c_too_choppy",
            QualitySeverity::Minor,
            QualityCategory::Craft,
            "C档 low-breath scene has repeated ultra-short paragraphs",
        ));
    }
    if let Some(profile) = &xianxia_chapter {
        if profile.abstract_emotion_hits >= 4
            && profile.concrete_reaction_hits < profile.abstract_emotion_hits / 2
        {
            signals.push(QualitySignal::new(
                "xianxia_emotion_texture",
                QualitySeverity::Minor,
                QualityCategory::Craft,
                "abstract emotion words outnumber body/object reactions",
            ));
        }
        if profile.resource_mentions > 0 && profile.resource_anchor_hits == 0 {
            signals.push(QualitySignal::new(
                "xianxia_resource_anchor",
                QualitySeverity::Major,
                QualityCategory::ResourceEconomy,
                "resources appear without price/income/cost anchor",
            ));
        }
        if profile.resource_mentions > 0
            && count_terms(chapter_text, XIANXIA_RESOURCE_OBLIGATION_TERMS) == 0
        {
            signals.push(QualitySignal::new(
                "xianxia_resource_obligation",
                QualitySeverity::Major,
                QualityCategory::ResourceEconomy,
                "resource gains lack debt/control/sect obligation signal",
            ));
        }
        if profile.combat_hits >= 4
            && (profile.combat_observation_hits == 0 || profile.combat_reversal_hits == 0)
        {
            signals.push(QualitySignal::new(
                "xianxia_combat_knowledge_loop",
                QualitySeverity::Major,
                QualityCategory::Craft,
                "combat lacks observation-to-reversal signals",
            ));
        }
        if profile.quoted_lines >= 3 && profile.repeated_dialogue_lines >= profile.quoted_lines / 2
        {
            signals.push(QualitySignal::new(
                "xianxia_dialogue_voice",
                QualitySeverity::Minor,
                QualityCategory::Craft,
                "dialogue lines repeat or lack distinguishable voice",
            ));
        }
        if profile.worldbuilding_hits >= 3 && action_hits == 0 && dialogue_lines == 0 {
            signals.push(QualitySignal::new(
                "xianxia_worldbuilding_action",
                QualitySeverity::Major,
                QualityCategory::Craft,
                "setting terms are not exposed through action or dialogue",
            ));
        }
        if length >= 800 && profile.short_sentence_count == 0 {
            signals.push(QualitySignal::new(
                "xianxia_rhythm",
                QualitySeverity::SignalOnly,
                QualityCategory::Craft,
                "long chapter has no short breath sentence",
            ));
        }
    }

    let score = (100_i32 - (signals.len() as i32 * 12)
        + (action_hits.min(12) as i32)
        + (consequence_hits.min(8) as i32 * 2)
        + (payoff_hits.min(6) as i32 * 2)
        + (carry.carry_rate * 10.0) as i32)
        .clamp(0, 100);

    let mut out = String::new();
    out.push_str("# ChapterQualityReport\n\n");
    let _ = writeln!(out, "- score: {score}/100");
    let _ = writeln!(out, "- length_chars: {length}");
    let _ = writeln!(out, "- dialogue_markers: {dialogue_lines}");
    let _ = writeln!(out, "- action_signals: {action_hits}");
    let _ = writeln!(out, "- consequence_signals: {consequence_hits}");
    let _ = writeln!(out, "- payoff_pressure_signals: {payoff_hits}");
    let _ = writeln!(
        out,
        "- scene_function: goals {}, consequences {}, dialogue_exchanges {}, dialogue_change_hits {}",
        scene_stats.goal_hits,
        scene_stats.consequence_hits,
        scene_stats.dialogue_exchanges,
        scene_stats.dialogue_change_hits
    );
    if bridge_quality.has_previous {
        let _ = writeln!(
            out,
            "- chapter_bridge: previous_keywords [{}], opening_keywords [{}], overlap {}, transition {}",
            bridge_quality.previous_keywords.join(", "),
            bridge_quality.opening_keywords.join(", "),
            bridge_quality.keyword_overlap,
            bridge_quality.opening_has_transition
        );
    }
    let _ = writeln!(
        out,
        "- genre_profile: {}",
        if xianxia.active { "xianxia" } else { "general" }
    );
    out.push_str(&style_discipline_report(
        chapter_text,
        &style_stats,
        scene_gear,
        &viewpoint_leaks,
    ));
    if let Some(profile) = &xianxia_chapter {
        let _ = writeln!(
            out,
            "- xianxia_abstract_emotion_terms: {}",
            profile.abstract_emotion_hits
        );
        let _ = writeln!(
            out,
            "- xianxia_body_object_reactions: {}",
            profile.concrete_reaction_hits
        );
        let _ = writeln!(
            out,
            "- xianxia_resource_anchor: resources {}, anchors {}",
            profile.resource_mentions, profile.resource_anchor_hits
        );
        let _ = writeln!(
            out,
            "- xianxia_combat_loop: combat {}, observation {}, reversal {}",
            profile.combat_hits, profile.combat_observation_hits, profile.combat_reversal_hits
        );
        let _ = writeln!(
            out,
            "- xianxia_dialogue_voice: quoted {}, repeated {}",
            profile.quoted_lines, profile.repeated_dialogue_lines
        );
        let _ = writeln!(
            out,
            "- xianxia_worldbuilding_terms: {}",
            profile.worldbuilding_hits
        );
        let _ = writeln!(
            out,
            "- short_sentences_le_5_chars: {}",
            profile.short_sentence_count
        );
    }
    let _ = writeln!(
        out,
        "- anchor_carry: anchors {}, mentioned {}, carried {}, rate {:.2}",
        carry.anchor_count, carry.mentioned_count, carry.carried_count, carry.carry_rate
    );
    out.push_str("- detected_issues:\n");
    if signals.is_empty() {
        out.push_str("  - none\n");
    } else {
        for signal in &signals {
            let _ = writeln!(out, "  - {}", signal.legacy_issue());
        }
    }
    out.push_str("- structured_signals:\n");
    if signals.is_empty() {
        out.push_str("  - none\n");
    } else {
        for signal in &signals {
            let _ = writeln!(
                out,
                "  - code: {}, severity: {}, category: {}, message: {}",
                signal.code,
                signal.severity.as_str(),
                signal.category.as_str(),
                signal.message
            );
        }
    }
    out.push_str("- top_revision_targets:\n");
    let targets = quality_revision_targets_from_signals(&signals, &carry);
    if targets.is_empty() {
        out.push_str("  - none\n");
    } else {
        for target in targets {
            let _ = writeln!(out, "  - {target}");
        }
    }
    Ok(out)
}

fn targeted_revision_targets(
    root: &Path,
    chapter: u32,
    draft: &str,
    audit: &str,
) -> Result<String> {
    let quality = chapter_quality_report(root, chapter, draft)?;
    let mut targets = extract_revision_targets_from_quality(&quality);
    targets.extend(extract_actionable_audit_targets(audit));
    targets.sort_by(|left, right| {
        revision_target_rank(left)
            .cmp(&revision_target_rank(right))
            .then_with(|| left.cmp(right))
    });
    targets.dedup();
    targets.truncate(3);
    let mut out = String::new();
    if targets.is_empty() {
        out.push_str(
            "- Preserve the chapter shape; only repair confirmed continuity or clarity problems.\n",
        );
    } else {
        for target in targets {
            let _ = writeln!(out, "- {target}");
        }
    }
    Ok(out)
}

fn revision_target_rank(target: &str) -> u8 {
    if target.starts_with("audit:blocker:") {
        0
    } else if target.contains("[blocker|") {
        1
    } else if target.starts_with("audit:major:") {
        2
    } else if target.starts_with("audit:overview:") {
        3
    } else if target.starts_with("audit:continuity:") || target.starts_with("audit:reader_promise:")
    {
        4
    } else if target.starts_with("audit:craft:") {
        5
    } else if target.starts_with("audit:minor:") {
        6
    } else if target.contains("[major|") || target.starts_with("quality:") {
        7
    } else if target.contains("[minor|continuity]")
        || target.contains("[minor|reader_promise]")
        || target.contains("[minor|craft]")
    {
        8
    } else {
        9
    }
}

fn extract_revision_targets_from_quality(quality: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let mut in_targets = false;
    for line in quality.lines() {
        let trimmed = line.trim();
        if trimmed == "- top_revision_targets:" {
            in_targets = true;
            continue;
        }
        if in_targets {
            if let Some(item) = trimmed.strip_prefix("- ") {
                if item != "none" {
                    targets.push(format!("quality: {}", item));
                }
            } else if !trimmed.is_empty() {
                break;
            }
        }
    }
    targets
}

fn extract_actionable_audit_targets(audit: &str) -> Vec<String> {
    let mut section = AuditTargetSection::General;
    let mut targets = Vec::new();
    for line in audit.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            section = audit_target_section(trimmed.trim_start_matches('#').trim());
            continue;
        }
        if !audit_section_can_drive_revision(section) || !is_revision_problem_line(trimmed) {
            continue;
        }
        targets.push(format!(
            "audit:{}: {}",
            section.as_label(),
            limit_chars(trimmed.trim_start_matches(['-', '*', ' ', '\t']), 180)
        ));
        if targets.len() >= 6 {
            break;
        }
    }
    targets
}

fn audit_section_can_drive_revision(section: AuditTargetSection) -> bool {
    matches!(
        section,
        AuditTargetSection::AuditOverview
            | AuditTargetSection::Continuity
            | AuditTargetSection::Craft
            | AuditTargetSection::ReaderPromise
            | AuditTargetSection::Blocker
            | AuditTargetSection::Major
            | AuditTargetSection::Minor
    )
}

fn protected_revision_strengths(audit: &str) -> String {
    let strengths = extract_protected_strengths(audit);
    let mut out = String::new();
    if strengths.is_empty() {
        out.push_str("- Preserve any working scene order, voice, action beats, and reader hooks unless a confirmed target requires a local edit.\n");
    } else {
        for strength in strengths {
            let _ = writeln!(out, "- {strength}");
        }
    }
    out
}

fn extract_protected_strengths(audit: &str) -> Vec<String> {
    let mut section = AuditTargetSection::General;
    let mut strengths = Vec::new();
    for line in audit.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            section = audit_target_section(trimmed.trim_start_matches('#').trim());
            continue;
        }
        if !is_actionable_audit_line(trimmed) {
            continue;
        }
        if section == AuditTargetSection::ProtectedStrengths || is_protective_audit_line(trimmed) {
            strengths.push(limit_chars(
                trimmed.trim_start_matches(['-', '*', ' ', '\t']),
                180,
            ));
        }
        if strengths.len() >= 5 {
            break;
        }
    }
    strengths.sort();
    strengths.dedup();
    strengths
}

fn is_revision_problem_line(line: &str) -> bool {
    let trimmed = line.trim().trim_start_matches(['-', '*', ' ', '\t']);
    if !is_actionable_audit_line(trimmed) || is_protective_audit_line(trimmed) {
        return false;
    }
    if is_no_problem_audit_line(trimmed) {
        return false;
    }
    contains_problem_marker(trimmed)
}

fn is_protective_audit_line(line: &str) -> bool {
    let trimmed = line.trim().trim_start_matches(['-', '*', ' ', '\t']);
    if contains_problem_marker(trimmed) {
        return false;
    }
    contains_any(
        trimmed,
        &[
            "有效",
            "做得很好",
            "很好",
            "极佳",
            "可保留",
            "保留",
            "符合",
            "合理",
            "自然",
            "精准",
            "克制",
            "有重量",
            "有信息量",
            "人味",
            "不是信息搬运",
            "服务了",
            "保护",
            "成立",
            "无冲突",
            "连贯",
        ],
    )
}

fn is_no_problem_audit_line(line: &str) -> bool {
    let trimmed = line.trim().trim_start_matches(['-', '*', ' ', '\t']);
    let lower = trimmed.to_ascii_lowercase();
    trimmed.eq_ignore_ascii_case("none")
        || trimmed.eq_ignore_ascii_case("n/a")
        || trimmed.eq_ignore_ascii_case("无")
        || trimmed.eq_ignore_ascii_case("暂无")
        || trimmed.starts_with("无。")
        || trimmed.starts_with("无 ")
        || trimmed.starts_with("没有问题")
        || trimmed.starts_with("未发现")
        || lower.starts_with("no ")
        || contains_any(
            trimmed,
            &[
                "无 blocker",
                "无 canonical 冲突",
                "无重大",
                "无时间跳跃",
                "无矛盾",
                "无冲突",
                "没有冲突",
                "没有矛盾",
                "没有破坏",
                "没有过度",
                "没有解释性",
                "没有风险",
                "不算 canon 违反",
            ],
        )
}

fn contains_problem_marker(line: &str) -> bool {
    contains_any(
        line,
        &[
            "问题",
            "冲突",
            "矛盾",
            "泄漏",
            "破坏",
            "伤害",
            "风险",
            "错误",
            "不合理",
            "不一致",
            "不符合",
            "不应",
            "不知道",
            "缺少",
            "缺失",
            "不足",
            "未建立",
            "未推进",
            "未回收",
            "未兑现",
            "无法",
            "不能",
            "过度",
            "过量",
            "偏慢",
            "偏描述",
            "突然",
            "没有观察链",
            "没有推进",
            "没有回收",
            "没有兑现",
            "没有建立",
            "没有变化",
            "没有代价",
            "没有后果",
            "没有目标",
            "没有选择",
            "没有反应",
            "视角",
            "AI腔",
            "AI 腔",
            "解释性",
            "修复",
            "建议",
            "BLOCKER",
            "MAJOR",
            "MINOR",
            "❌",
            "⚠️",
            "P0",
        ],
    )
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AuditTargetSection {
    AuditOverview,
    Continuity,
    Craft,
    MemoryCandidate,
    ReaderPromise,
    ProtectedStrengths,
    Blocker,
    Major,
    Minor,
    General,
}

impl AuditTargetSection {
    fn as_label(self) -> &'static str {
        match self {
            AuditTargetSection::AuditOverview => "overview",
            AuditTargetSection::Continuity => "continuity",
            AuditTargetSection::Craft => "craft",
            AuditTargetSection::MemoryCandidate => "memory_candidate",
            AuditTargetSection::ReaderPromise => "reader_promise",
            AuditTargetSection::ProtectedStrengths => "protected_strengths",
            AuditTargetSection::Blocker => "blocker",
            AuditTargetSection::Major => "major",
            AuditTargetSection::Minor => "minor",
            AuditTargetSection::General => "general",
        }
    }

    fn is_layered(self) -> bool {
        matches!(
            self,
            AuditTargetSection::Continuity
                | AuditTargetSection::Craft
                | AuditTargetSection::MemoryCandidate
                | AuditTargetSection::ReaderPromise
        )
    }
}

fn audit_target_section(heading: &str) -> AuditTargetSection {
    if markdown_heading_matches(heading, "AUDIT_OVERVIEW") {
        AuditTargetSection::AuditOverview
    } else if markdown_heading_matches(heading, "CONTINUITY_AUDIT") {
        AuditTargetSection::Continuity
    } else if markdown_heading_matches(heading, "CRAFT_AUDIT") {
        AuditTargetSection::Craft
    } else if markdown_heading_matches(heading, "MEMORY_CANDIDATE_AUDIT")
        || markdown_heading_matches(heading, "CANDIDATE_MEMORY_UPDATES")
    {
        AuditTargetSection::MemoryCandidate
    } else if markdown_heading_matches(heading, "READER_PROMISE_AUDIT") {
        AuditTargetSection::ReaderPromise
    } else if markdown_heading_matches(heading, "PROTECTED_STRENGTHS") {
        AuditTargetSection::ProtectedStrengths
    } else if markdown_heading_matches(heading, "BLOCKER") {
        AuditTargetSection::Blocker
    } else if markdown_heading_matches(heading, "MAJOR") {
        AuditTargetSection::Major
    } else if markdown_heading_matches(heading, "MINOR") {
        AuditTargetSection::Minor
    } else {
        AuditTargetSection::General
    }
}

fn quality_revision_targets_from_signals(
    signals: &[QualitySignal],
    carry: &AnchorCarryWindowReport,
) -> Vec<String> {
    let mut sorted = signals.to_vec();
    sorted.sort_by_key(|signal| {
        (
            quality_severity_rank(signal.severity),
            quality_category_rank(signal.category),
            signal.code,
        )
    });
    let mut targets = sorted
        .iter()
        .take(3)
        .map(QualitySignal::revision_target)
        .collect::<Vec<_>>();
    if let Some(resource_signal) = sorted
        .iter()
        .find(|signal| signal.category == QualityCategory::ResourceEconomy)
    {
        let resource_target = resource_signal.revision_target();
        if !targets.iter().any(|target| target == &resource_target) {
            if targets.len() >= 3 {
                targets.pop();
            }
            targets.push(resource_target);
        }
    }
    if targets.len() < 3 {
        for item in &carry.weak_items {
            targets.push(format!(
                "anchor_carry: make `{}` participate in action, dialogue, consequence, or payoff pressure",
                item.anchor
            ));
            if targets.len() >= 3 {
                break;
            }
        }
    }
    targets.truncate(3);
    targets
}

fn quality_severity_rank(severity: QualitySeverity) -> u8 {
    match severity {
        QualitySeverity::Blocker => 0,
        QualitySeverity::Major => 1,
        QualitySeverity::Minor => 2,
        QualitySeverity::SignalOnly => 3,
    }
}

fn quality_category_rank(category: QualityCategory) -> u8 {
    match category {
        QualityCategory::Continuity => 0,
        QualityCategory::ResourceEconomy => 1,
        QualityCategory::ReaderPromise => 2,
        QualityCategory::Context => 3,
        QualityCategory::Craft => 4,
    }
}

fn score_anchor_carry_for_text(text: &str, anchors: &[String]) -> AnchorCarryWindowReport {
    let mut mentioned = 0usize;
    let mut carried = 0usize;
    let mut weak_items = Vec::new();
    let sentences = split_anchor_sentences(text);
    for anchor in anchors {
        let item = score_anchor_carry_item(anchor, &sentences);
        if item.mentioned {
            mentioned += 1;
        }
        if item.carried {
            carried += 1;
        }
        if item.mentioned && !item.carried {
            weak_items.push(AnchorCarryWindowItem {
                anchor: anchor.clone(),
                modes: Vec::new(),
                terms: Vec::new(),
            });
        }
    }
    let anchor_count = anchors.len();
    AnchorCarryWindowReport {
        anchor_count,
        mentioned_count: mentioned,
        carried_count: carried,
        carry_rate: if anchor_count == 0 {
            0.0
        } else {
            carried as f32 / anchor_count as f32
        },
        weak_items,
    }
}

fn count_terms(text: &str, terms: &[&str]) -> usize {
    terms.iter().map(|term| text.matches(term).count()).sum()
}

fn style_discipline_stats(text: &str) -> StyleDisciplineStats {
    let paragraph_lengths = prose_paragraph_lengths(text);
    let max_paragraph_chars = paragraph_lengths.iter().copied().max().unwrap_or(0);
    let min_paragraph_chars = paragraph_lengths.iter().copied().min().unwrap_or(0);
    StyleDisciplineStats {
        zero_tolerance_hits: count_terms(text, STYLE_ZERO_TOLERANCE_TERMS)
            + count_terms(text, STYLE_AI_SUMMARY_TERMS),
        budget_hits: count_terms(text, STYLE_BUDGET_TERMS),
        budget_limit: text.chars().count().div_ceil(3_000).max(1) * 2,
        ai_summary_hits: count_terms(text, STYLE_AI_SUMMARY_TERMS),
        ai_opening_hits: ai_opening_hits(text),
        ai_structure_hits: count_terms(text, STYLE_AI_STRUCTURE_TERMS),
        rule_of_three_hits: rule_of_three_hits(text),
        long_paragraph_runs: count_consecutive_runs(&paragraph_lengths, |len| len > 250, 2),
        short_paragraph_runs: count_consecutive_runs(&paragraph_lengths, |len| len < 30, 3),
        max_paragraph_chars,
        min_paragraph_chars,
    }
}

fn rule_of_three_hits(text: &str) -> usize {
    text.split("\n\n")
        .map(str::trim)
        .filter(|paragraph| !paragraph.is_empty())
        .filter(|paragraph| {
            let comma_count = paragraph.matches('，').count()
                + paragraph.matches(',').count()
                + paragraph.matches('、').count();
            comma_count >= 2 && count_terms(paragraph, STYLE_RULE_OF_THREE_TERMS) >= 3
        })
        .count()
}

fn scene_function_stats(text: &str) -> SceneFunctionStats {
    let sentences = split_anchor_sentences(text);
    let mut stats = SceneFunctionStats {
        goal_hits: count_terms(text, SCENE_GOAL_TERMS),
        consequence_hits: count_terms(text, SCENE_CHANGE_TERMS)
            + count_terms(text, ANCHOR_CONSEQUENCE_TERMS),
        ..SceneFunctionStats::default()
    };
    for sentence in sentences {
        if sentence.contains('“') || sentence.contains('"') {
            stats.dialogue_exchanges += 1;
            if has_dialogue_change_signal(&sentence) {
                stats.dialogue_change_hits += 1;
            }
        }
    }
    stats
}

fn has_dialogue_change_signal(sentence: &str) -> bool {
    count_terms(sentence, SCENE_CHANGE_TERMS) > 0
        || count_terms(sentence, ANCHOR_CONSEQUENCE_TERMS) > 0
        || count_terms(sentence, ANCHOR_PAYOFF_TERMS) > 0
}

fn ai_opening_hits(text: &str) -> usize {
    let Some(opening) = first_prose_paragraph(text) else {
        return 0;
    };
    if opening.chars().count() > 140 {
        return 0;
    }
    let atmosphere_hits = count_terms(opening, STYLE_AI_OPENING_ATMOSPHERE_TERMS);
    if atmosphere_hits == 0 {
        return 0;
    }
    let action_hits = count_terms(opening, STYLE_OPENING_ACTION_TERMS);
    let character_hits = count_terms(opening, STYLE_OPENING_CHARACTER_TERMS);
    if action_hits == 0 || character_hits == 0 {
        return 1;
    }
    0
}

fn first_prose_paragraph(text: &str) -> Option<&str> {
    text.split("\n\n")
        .map(str::trim)
        .find(|paragraph| !paragraph.is_empty() && !paragraph.starts_with('#'))
}

fn prose_paragraph_lengths(text: &str) -> Vec<usize> {
    prose_paragraphs(text)
        .into_iter()
        .map(|paragraph| paragraph.chars().count())
        .collect()
}

fn prose_paragraphs(text: &str) -> Vec<String> {
    text.split("\n\n")
        .map(str::trim)
        .filter(|paragraph| !paragraph.is_empty())
        .filter(|paragraph| !paragraph.starts_with('#'))
        .map(ToString::to_string)
        .collect()
}

fn count_consecutive_runs(
    lengths: &[usize],
    predicate: impl Fn(usize) -> bool,
    threshold: usize,
) -> usize {
    let mut runs = 0;
    let mut current = 0;
    for &length in lengths {
        if predicate(length) {
            current += 1;
            if current == threshold {
                runs += 1;
            }
        } else {
            current = 0;
        }
    }
    runs
}

fn style_discipline_report(
    text: &str,
    stats: &StyleDisciplineStats,
    scene_gear: SceneGear,
    viewpoint_leaks: &[String],
) -> String {
    let mut out = String::new();
    out.push_str("- style_discipline:\n");
    let _ = writeln!(
        out,
        "  - zero_tolerance_hits: {}",
        stats.zero_tolerance_hits
    );
    let _ = writeln!(
        out,
        "  - budget_terms: {}/{}",
        stats.budget_hits, stats.budget_limit
    );
    let _ = writeln!(out, "  - ai_summary_hits: {}", stats.ai_summary_hits);
    let _ = writeln!(out, "  - ai_opening_hits: {}", stats.ai_opening_hits);
    let _ = writeln!(out, "  - ai_structure_hits: {}", stats.ai_structure_hits);
    let _ = writeln!(out, "  - rule_of_three_hits: {}", stats.rule_of_three_hits);
    let _ = writeln!(
        out,
        "  - paragraph_chars: min {}, max {}",
        stats.min_paragraph_chars, stats.max_paragraph_chars
    );
    let _ = writeln!(
        out,
        "  - long_paragraph_runs: {}",
        stats.long_paragraph_runs
    );
    let _ = writeln!(
        out,
        "  - short_paragraph_runs: {}",
        stats.short_paragraph_runs
    );
    let _ = writeln!(out, "  - scene_gear: {}", scene_gear.as_str());
    let _ = writeln!(out, "  - viewpoint_leaks: {}", viewpoint_leaks.len());
    let zero_examples = matched_terms(text, STYLE_ZERO_TOLERANCE_TERMS, 8);
    if !zero_examples.is_empty() {
        let _ = writeln!(out, "  - zero_examples: {}", zero_examples.join(", "));
    }
    let budget_examples = matched_terms(text, STYLE_BUDGET_TERMS, 8);
    if !budget_examples.is_empty() {
        let _ = writeln!(out, "  - budget_examples: {}", budget_examples.join(", "));
    }
    let structure_examples = matched_terms(text, STYLE_AI_STRUCTURE_TERMS, 8);
    if !structure_examples.is_empty() {
        let _ = writeln!(
            out,
            "  - structure_examples: {}",
            structure_examples.join(", ")
        );
    }
    if stats.ai_opening_hits > 0
        && let Some(opening) = first_prose_paragraph(text)
    {
        let _ = writeln!(out, "  - opening_signal: {}", limit_chars(opening, 120));
    }
    for leak in viewpoint_leaks.iter().take(5) {
        let _ = writeln!(out, "  - viewpoint_signal: {}", limit_chars(leak, 120));
    }
    out
}

fn matched_terms(text: &str, terms: &[&str], limit: usize) -> Vec<String> {
    let mut matches = Vec::new();
    for term in terms {
        if text.contains(term) {
            matches.push((*term).to_string());
            if matches.len() >= limit {
                break;
            }
        }
    }
    matches
}

fn detect_scene_gear(root: &Path, chapter: u32) -> Result<SceneGear> {
    let mut source = String::new();
    for file in ["brief.md", "craft_plan.md"] {
        if let Some(text) = read_optional_limited(&chapter_dir(root, chapter).join(file), 16_000) {
            source.push('\n');
            source.push_str(&text);
        }
    }
    Ok(scene_gear_from_text(&source))
}

fn scene_gear_from_text(text: &str) -> SceneGear {
    if text.contains("A档") || text.contains("高压爆发") {
        SceneGear::HighPressure
    } else if text.contains("C档") || text.contains("低速呼吸") {
        SceneGear::LowBreath
    } else if text.contains("B档") || text.contains("正常推进") {
        SceneGear::Normal
    } else {
        SceneGear::Missing
    }
}

fn scene_gear_context(root: &Path, chapter: u32) -> Result<String> {
    let gear = detect_scene_gear(root, chapter)?;
    let mut out = String::new();
    out.push_str("## SceneGear\n\n");
    let _ = writeln!(out, "- detected: {}", gear.as_str());
    match gear {
        SceneGear::HighPressure => out.push_str(
            "- discipline: short paragraphs, compressed environment, action and urgency before explanation.\n",
        ),
        SceneGear::Normal => out.push_str(
            "- discipline: balance action, detail, dialogue pressure, and character depth.\n",
        ),
        SceneGear::LowBreath => out.push_str(
            "- discipline: allow slower paragraphs, more interiority for the active viewpoint, and controlled atmosphere.\n",
        ),
        SceneGear::Missing => out.push_str(
            "- missing: add `场景档位: A档/B档/C档` to brief.md or craft_plan.md before drafting.\n",
        ),
    }
    Ok(out)
}

fn viewpoint_boundary_leaks(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| {
            VIEWPOINT_INNER_STATE_PATTERNS
                .iter()
                .any(|pattern| line.contains(pattern))
        })
        .map(str::to_string)
        .collect()
}

fn character_behavior_context(root: &Path, limit: usize) -> Result<Option<String>> {
    let records = read_character_behavior_records(root)?;
    if records.is_empty() {
        return Ok(None);
    }
    let mut out = String::new();
    out.push_str("## Recent Character Behavior Ledger\n\n");
    for record in records.iter().rev().take(limit) {
        let chapter = record
            .chapter
            .map(|chapter| format!("{chapter:03}"))
            .unwrap_or_else(|| "???".to_string());
        let _ = writeln!(
            out,
            "- {} | {} | {} | {} | chapter {}",
            record.character, record.situation, record.choice, record.result, chapter
        );
    }
    Ok(Some(out))
}

fn read_character_behavior_records(root: &Path) -> Result<Vec<CharacterBehaviorRecord>> {
    let path = root.join("memory/behavior.jsonl");
    let Some(text) = std::fs::read_to_string(&path).ok() else {
        return Ok(Vec::new());
    };
    let mut records = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let record = serde_json::from_str::<CharacterBehaviorRecord>(trimmed)
            .with_context(|| format!("failed to parse {} line {}", path.display(), index + 1))?;
        records.push(record);
    }
    Ok(records)
}

fn behavior_records_from_candidate(
    candidate: &MemoryUpdateCandidate,
) -> Vec<CharacterBehaviorRecord> {
    let kind = canonical_memory_kind(&candidate.kind);
    if !matches!(
        kind.as_str(),
        "knowledge" | "relationship" | "character_state" | "memory"
    ) {
        return Vec::new();
    }
    let character = candidate
        .affects
        .iter()
        .find_map(|affect| affect.strip_prefix("character:").map(str::to_string))
        .unwrap_or_else(|| candidate.target.clone());
    if character.trim().is_empty() {
        return Vec::new();
    }
    vec![CharacterBehaviorRecord {
        chapter: candidate.chapter,
        character,
        situation: limit_chars(&candidate.evidence, 120),
        choice: limit_chars(&candidate.change, 160),
        result: format!("memory candidate applied as {kind}"),
        evidence: candidate.evidence.clone(),
        source: "memory_candidate".to_string(),
    }]
}

fn reference_passage_context(root: &Path, chapter: u32) -> Result<Option<String>> {
    let gear = detect_scene_gear(root, chapter)?;
    let scene_kind = scene_kind_for_gear(gear);
    let mut passages =
        collect_reference_passages_from_dir(&root.join("craft/examples"), scene_kind)?;
    if passages.len() < 3 {
        passages.extend(collect_reference_passages_from_dir(
            &root.join("materials"),
            scene_kind,
        )?);
    }
    passages.truncate(3);
    if passages.is_empty() {
        return Ok(None);
    }
    let mut out = String::new();
    out.push_str("## Authorized Reference Passages\n\n");
    out.push_str(
        "Use these as distance, density, and rhythm anchors only. Do not copy their wording.\n",
    );
    for (index, (path, passage)) in passages.iter().enumerate() {
        let _ = writeln!(
            out,
            "{}. `{}`\n{}\n",
            index + 1,
            display_relative(root, path),
            limit_chars(passage, 700)
        );
    }
    Ok(Some(out))
}

fn scene_kind_for_gear(gear: SceneGear) -> &'static str {
    match gear {
        SceneGear::HighPressure => "battle",
        SceneGear::LowBreath => "emotion",
        SceneGear::Normal | SceneGear::Missing => "daily",
    }
}

fn collect_reference_passages_from_dir(
    dir: &Path,
    scene_kind: &str,
) -> Result<Vec<(PathBuf, String)>> {
    let mut passages = Vec::new();
    for path in collect_files_with_extensions(dir, &["md", "txt"])? {
        let Some(text) = read_optional_limited(&path, 16_000) else {
            continue;
        };
        if !reference_text_is_authorized(&text) || !reference_text_matches_scene(&text, scene_kind)
        {
            continue;
        }
        if let Some(passage) = extract_reference_passage(&text) {
            passages.push((path, passage));
        }
    }
    Ok(passages)
}

fn reference_text_is_authorized(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("authorized: true")
        || lower.contains("original: true")
        || text.contains("授权: 是")
        || text.contains("原创: 是")
}

fn reference_text_matches_scene(text: &str, scene_kind: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains(&format!("scene: {scene_kind}"))
        || match scene_kind {
            "battle" => text.contains("战斗") || text.contains("高压"),
            "emotion" => text.contains("情感") || text.contains("低速"),
            "daily" => text.contains("日常") || text.contains("过渡") || text.contains("对话"),
            _ => false,
        }
}

fn extract_reference_passage(text: &str) -> Option<String> {
    let body = text
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.starts_with("scene:")
                && !trimmed.starts_with("authorized:")
                && !trimmed.starts_with("original:")
                && !trimmed.starts_with("授权:")
                && !trimmed.starts_with("原创:")
                && !trimmed.starts_with('#')
        })
        .collect::<Vec<_>>()
        .join("\n");
    let passage = body.trim();
    (!passage.is_empty()).then(|| passage.to_string())
}

#[derive(Debug, Default)]
struct XianxiaContextProfile {
    active: bool,
    world_cards: usize,
    resource_cards: usize,
    resource_value_cards: usize,
    resource_control_cards: usize,
    realm_rule_hits: usize,
    faction_hits: usize,
    resource_anchor_hits: usize,
    artifact_hits: usize,
}

#[derive(Debug, Default)]
struct XianxiaChapterProfile {
    abstract_emotion_hits: usize,
    concrete_reaction_hits: usize,
    resource_mentions: usize,
    resource_anchor_hits: usize,
    combat_hits: usize,
    combat_observation_hits: usize,
    combat_reversal_hits: usize,
    worldbuilding_hits: usize,
    short_sentence_count: usize,
    quoted_lines: usize,
    repeated_dialogue_lines: usize,
}

fn xianxia_chapter_profile(text: &str) -> XianxiaChapterProfile {
    let quoted_lines = extract_quoted_lines(text);
    XianxiaChapterProfile {
        abstract_emotion_hits: count_terms(text, XIANXIA_ABSTRACT_EMOTION_TERMS),
        concrete_reaction_hits: count_terms(text, XIANXIA_CONCRETE_REACTION_TERMS),
        resource_mentions: count_terms(text, XIANXIA_RESOURCE_TERMS),
        resource_anchor_hits: count_terms(text, XIANXIA_RESOURCE_ANCHOR_TERMS),
        combat_hits: count_terms(text, XIANXIA_COMBAT_TERMS),
        combat_observation_hits: count_terms(text, XIANXIA_COMBAT_OBSERVATION_TERMS),
        combat_reversal_hits: count_terms(text, XIANXIA_COMBAT_REVERSAL_TERMS),
        worldbuilding_hits: count_terms(text, XIANXIA_WORLDBUILDING_TERMS),
        short_sentence_count: count_short_sentences(text, 5),
        quoted_lines: quoted_lines.len(),
        repeated_dialogue_lines: repeated_dialogue_line_count(&quoted_lines),
    }
}

fn xianxia_context_profile(root: &Path, manifest: &BookManifest) -> Result<XianxiaContextProfile> {
    let mut profile = XianxiaContextProfile {
        active: is_xianxia_genre(&manifest.genre),
        ..XianxiaContextProfile::default()
    };
    let mut corpus = manifest.genre.clone();
    for path in [
        "bible/premise.md",
        "bible/world.md",
        "bible/reader_promise.md",
        "bible/style.md",
    ] {
        if let Some(text) = read_optional_limited(&root.join(path), 24_000) {
            corpus.push('\n');
            corpus.push_str(&text);
        }
    }
    for dir in ["cards/world", "cards/locations"] {
        for path in collect_asset_files(&root.join(dir))? {
            if is_template_asset(&path) {
                continue;
            }
            if path.is_file() {
                profile.world_cards += 1;
            }
            if let Some(text) = read_optional_limited(&path, 12_000) {
                corpus.push('\n');
                corpus.push_str(&text);
            }
        }
    }
    for path in collect_asset_files(&root.join("cards/resources"))? {
        if is_template_asset(&path) {
            continue;
        }
        if path.is_file() {
            profile.resource_cards += 1;
        }
        if let Some(text) = read_optional_limited(&path, 12_000) {
            if resource_card_has_value_anchor(&text) {
                profile.resource_value_cards += 1;
            }
            if resource_card_has_control_anchor(&text) {
                profile.resource_control_cards += 1;
            }
            corpus.push('\n');
            corpus.push_str(&text);
        }
    }

    profile.active |= count_terms(&corpus, XIANXIA_GENRE_TERMS) > 0;
    if profile.active {
        profile.realm_rule_hits = count_terms(&corpus, XIANXIA_REALM_RULE_TERMS);
        profile.faction_hits = count_terms(&corpus, XIANXIA_FACTION_TERMS);
        profile.resource_anchor_hits = count_terms(&corpus, XIANXIA_RESOURCE_TERMS)
            + count_terms(&corpus, XIANXIA_CONTEXT_ECONOMY_TERMS);
        profile.artifact_hits = count_terms(&corpus, XIANXIA_ARTIFACT_TERMS);
    }
    Ok(profile)
}

fn is_xianxia_genre(genre: &str) -> bool {
    count_terms(genre, XIANXIA_GENRE_TERMS) > 0
}

fn resource_card_has_value_anchor(text: &str) -> bool {
    ["market_value", "ordinary_income_equivalent", "cost_to_use"]
        .iter()
        .any(|key| yaml_scalar_value(text, key).is_some())
        || count_terms(text, XIANXIA_RESOURCE_ANCHOR_TERMS) > 0
}

fn resource_card_has_control_anchor(text: &str) -> bool {
    [
        "who_controls_it",
        "debt_or_obligation",
        "canon_status",
        "evidence",
    ]
    .iter()
    .any(|key| yaml_scalar_value(text, key).is_some())
}

fn count_short_sentences(text: &str, max_chars: usize) -> usize {
    text.split(['。', '！', '？', '；', '\n'])
        .map(str::trim)
        .filter(|sentence| !sentence.is_empty())
        .filter(|sentence| sentence.chars().count() <= max_chars)
        .count()
}

fn extract_quoted_lines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    for ch in text.chars() {
        match ch {
            '“' => {
                in_quote = true;
                current.clear();
            }
            '”' if in_quote => {
                let line = current.trim();
                if !line.is_empty() {
                    lines.push(normalize_dialogue_line(line));
                }
                in_quote = false;
                current.clear();
            }
            '"' => {
                if in_quote {
                    let line = current.trim();
                    if !line.is_empty() {
                        lines.push(normalize_dialogue_line(line));
                    }
                    current.clear();
                }
                in_quote = !in_quote;
            }
            _ if in_quote => current.push(ch),
            _ => {}
        }
    }
    lines
}

fn normalize_dialogue_line(line: &str) -> String {
    line.chars()
        .filter(|ch| {
            !matches!(
                ch,
                '，' | '。' | '！' | '？' | '、' | '；' | '：' | ',' | '.' | '!' | '?' | ';' | ':'
            )
        })
        .take(40)
        .collect()
}

fn repeated_dialogue_line_count(lines: &[String]) -> usize {
    let mut counts = BTreeMap::<&str, usize>::new();
    for line in lines {
        *counts.entry(line.as_str()).or_default() += 1;
    }
    counts.values().filter(|count| **count > 1).sum()
}

fn read_chapter_text(chapter_dir: &Path) -> Result<String> {
    read_optional_limited(&chapter_dir.join("final.md"), CONTEXT_LIMIT)
        .or_else(|| read_optional_limited(&chapter_dir.join("draft.md"), CONTEXT_LIMIT))
        .ok_or_else(|| {
            anyhow!(
                "chapter {} has no final.md or draft.md",
                chapter_dir.display()
            )
        })
}

fn read_required(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))
}

fn read_optional_limited(path: &Path, limit: usize) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    Some(limit_chars(&raw, limit))
}

fn count_files_with_extension(dir: &Path, extension: &str) -> Result<usize> {
    if !dir.is_dir() {
        return Ok(0);
    }
    let mut count = 0;
    for entry in
        std::fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))?
    {
        let entry = entry?;
        if entry.file_type()?.is_file()
            && entry.path().extension().and_then(OsStr::to_str) == Some(extension)
        {
            count += 1;
        }
    }
    Ok(count)
}

fn count_jsonl_records(path: &Path) -> Result<usize> {
    let Some(text) = std::fs::read_to_string(path).ok() else {
        return Ok(0);
    };
    Ok(text.lines().filter(|line| !line.trim().is_empty()).count())
}

fn memory_graph_path(root: &Path) -> PathBuf {
    root.join("memory/graph.json")
}

fn load_memory_graph(root: &Path) -> Result<MemoryGraph> {
    let path = memory_graph_path(root);
    let raw = read_required(&path)?;
    serde_json::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
}

fn save_memory_graph(root: &Path, graph: &MemoryGraph) -> Result<()> {
    write_text(
        &memory_graph_path(root),
        &serde_json::to_string_pretty(graph).context("failed to encode memory graph")?,
        true,
    )
}

#[cfg(test)]
pub(crate) fn rebuild_memory_graph_for_test(root: &Path) -> Result<()> {
    let graph = build_memory_graph(root)?;
    save_memory_graph(root, &graph)
}

fn load_or_build_memory_graph(root: &Path) -> Result<MemoryGraph> {
    let graph_path = memory_graph_path(root);
    if !graph_path.is_file() || memory_graph_is_stale(root, &graph_path) {
        let graph = build_memory_graph(root)?;
        save_memory_graph(root, &graph)?;
        Ok(graph)
    } else {
        load_memory_graph(root)
    }
}

fn memory_graph_is_stale(root: &Path, graph_path: &Path) -> bool {
    let graph_modified = file_modified(graph_path);
    let cache_key = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let cache = MEMORY_SOURCE_FINGERPRINT_CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));

    {
        let guard = cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(cached) = guard.get(&cache_key)
            && cached.graph_modified == graph_modified
            && cached.checked_at.elapsed() <= MEMORY_SOURCE_FINGERPRINT_TTL
        {
            return fingerprint_is_newer_than_graph(&cached.fingerprint, graph_modified);
        }
    }

    let fingerprint = memory_graph_source_fingerprint(root);
    let stale = fingerprint_is_newer_than_graph(&fingerprint, graph_modified);

    let mut guard = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.insert(
        cache_key,
        CachedMemorySourceFingerprint {
            graph_modified,
            fingerprint,
            checked_at: Instant::now(),
        },
    );
    stale
}

fn memory_graph_source_paths(root: &Path) -> Vec<PathBuf> {
    [
        "book.toml",
        "bible",
        "cards",
        "outline",
        "chapters",
        "memory/summaries",
        "memory/facts.jsonl",
        "memory/events.jsonl",
        "memory/foreshadowing.jsonl",
        "memory/candidates",
        "memory/archives",
    ]
    .into_iter()
    .map(|rel| root.join(rel))
    .collect()
}

fn memory_graph_source_fingerprint(root: &Path) -> MemorySourceFingerprint {
    let mut entries = Vec::new();
    for path in memory_graph_source_paths(root) {
        collect_memory_source_fingerprint(root, &path, 0, &mut entries);
    }
    entries.sort();
    MemorySourceFingerprint { entries }
}

fn collect_memory_source_fingerprint(
    root: &Path,
    current: &Path,
    depth: usize,
    entries: &mut Vec<MemorySourceEntry>,
) {
    let Ok(metadata) = current.metadata() else {
        return;
    };
    let is_dir = metadata.is_dir();
    entries.push(MemorySourceEntry {
        path: display_relative(root, current),
        is_dir,
        modified: metadata.modified().ok(),
    });

    if !is_dir || depth >= MEMORY_SOURCE_FINGERPRINT_DEPTH {
        return;
    }
    let Ok(read_dir) = std::fs::read_dir(current) else {
        return;
    };
    for entry in read_dir.flatten() {
        if entry
            .file_type()
            .ok()
            .is_some_and(|file_type| file_type.is_symlink())
        {
            continue;
        }
        collect_memory_source_fingerprint(root, &entry.path(), depth + 1, entries);
    }
}

fn fingerprint_is_newer_than_graph(
    fingerprint: &MemorySourceFingerprint,
    graph_modified: Option<SystemTime>,
) -> bool {
    let graph_modified = graph_modified.unwrap_or(SystemTime::UNIX_EPOCH);
    fingerprint
        .entries
        .iter()
        .filter_map(|entry| entry.modified)
        .any(|modified| modified >= graph_modified)
}

fn file_modified(path: &Path) -> Option<SystemTime> {
    path.metadata()
        .and_then(|metadata| metadata.modified())
        .ok()
}

pub(crate) fn rebuild_memory_graph(workspace: &Path) -> Result<NovelMemorySnapshot> {
    let root = find_project_root(workspace)?;
    let graph = build_memory_graph(&root)?;
    save_memory_graph(&root, &graph)?;
    memory_snapshot_for_graph(root, graph)
}

pub(crate) fn memory_snapshot(workspace: &Path) -> Result<NovelMemorySnapshot> {
    let root = find_project_root(workspace)?;
    let graph_ready = memory_graph_path(&root).is_file();
    let graph = load_or_build_memory_graph(&root)?;
    let mut snapshot = memory_snapshot_for_graph(root, graph)?;
    snapshot.graph_ready = graph_ready;
    Ok(snapshot)
}

pub(crate) fn memory_context_for_chapter(
    workspace: &Path,
    chapter: u32,
    depth: usize,
    limit: usize,
) -> Result<String> {
    let root = find_project_root(workspace)?;
    let graph = load_or_build_memory_graph(&root)?;
    memory_context_packet(&root, &graph, chapter, depth, limit)
}

pub(crate) fn memory_query_packet(
    workspace: &Path,
    query: &str,
    depth: usize,
    limit: usize,
) -> Result<String> {
    let root = find_project_root(workspace)?;
    let graph = load_or_build_memory_graph(&root)?;
    let seeds = find_memory_seed_nodes(&graph, query);
    if seeds.is_empty() {
        bail!("No memory graph nodes matched query: {query}");
    }
    let neighborhood = memory_neighborhood(&graph, &seeds, depth, limit);
    Ok(format_memory_neighborhood(&neighborhood))
}

pub(crate) fn memory_promises_packet_from_workspace(workspace: &Path) -> Result<String> {
    let root = find_project_root(workspace)?;
    let graph = load_or_build_memory_graph(&root)?;
    Ok(memory_promises_packet(&root, &graph))
}

pub(crate) fn archive_memory_stage_from_workspace(
    workspace: &Path,
    start: u32,
    end: u32,
    label: Option<&str>,
) -> Result<String> {
    let root = find_project_root(workspace)?;
    archive_memory_stage(&root, start, end, label)
}

pub(crate) fn memory_regression_report_from_workspace(
    workspace: &Path,
    window: u32,
    write: bool,
) -> Result<String> {
    let root = find_project_root(workspace)?;
    memory_regression_report(&root, window, write)
}

pub(crate) fn memory_impact_packet(
    workspace: &Path,
    chapter: u32,
    depth: usize,
    limit: usize,
) -> Result<String> {
    let root = find_project_root(workspace)?;
    let graph = load_or_build_memory_graph(&root)?;
    let seed = chapter_node_id(chapter);
    let seeds = if graph.nodes.iter().any(|node| node.id == seed) {
        vec![seed]
    } else {
        find_memory_seed_nodes(&graph, &format!("{chapter:03}"))
    };
    if seeds.is_empty() {
        bail!("No memory graph node found for chapter {chapter:03}");
    }
    let neighborhood = memory_neighborhood(&graph, &seeds, depth, limit);
    let mut out = format!(
        "## Impact surface for chapter {chapter:03}\n\n\
         Use this before rewriting a chapter. Nearby nodes are the memories most likely to need updates.\n\n"
    );
    out.push_str(&format_memory_impact_summary(
        &root,
        &graph,
        &neighborhood,
        chapter,
    )?);
    out.push_str(&format_memory_neighborhood(&neighborhood));
    Ok(out)
}

pub(crate) fn resource_ledger_report_from_workspace(
    workspace: &Path,
    chapter: Option<u32>,
) -> Result<String> {
    let root = find_project_root(workspace)?;
    resource_ledger_report(&root, chapter)
}

fn format_memory_impact_summary(
    root: &Path,
    graph: &MemoryGraph,
    neighborhood: &GraphNeighborhood,
    chapter: u32,
) -> Result<String> {
    let mut out = String::new();
    let affected = affected_nodes_by_kind(neighborhood);
    out.push_str("## Affected story surfaces\n\n");
    for (kind, title) in [
        ("character", "Characters"),
        ("relationship", "Relationships"),
        ("knowledge", "Knowledge boundaries"),
        ("promise", "Promises / foreshadowing"),
        ("event", "Events / timeline"),
        ("location", "Locations"),
        ("object", "Objects / resources"),
        ("world", "World rules"),
    ] {
        let values = affected.get(kind).cloned().unwrap_or_default();
        if values.is_empty() {
            out.push_str(&format!("- {title}: none in graph neighborhood\n"));
        } else {
            out.push_str(&format!("- {title}: {}\n", values.join(", ")));
        }
    }

    let downstream = downstream_chapters(graph, chapter, 8);
    out.push_str("\n## Downstream chapters to re-check\n\n");
    if downstream.is_empty() {
        out.push_str("- none found after this chapter\n");
    } else {
        for chapter in downstream {
            out.push_str(&format!("- Chapter {chapter:03}\n"));
        }
    }

    let candidates = memory_candidates_for_display(root, Some(chapter), false)?;
    out.push_str("\n## Pending candidate memory updates for this chapter\n\n");
    if candidates.is_empty() {
        out.push_str("- none\n");
    } else {
        for candidate in candidates.iter().take(12) {
            out.push_str(&format!(
                "- [{}] {} -> {} (confidence {:.2})\n",
                candidate.kind, candidate.target, candidate.change, candidate.confidence
            ));
            if !candidate.affects.is_empty() {
                out.push_str(&format!("  affects: {}\n", candidate.affects.join(", ")));
            }
        }
    }

    out.push('\n');
    out.push_str(&resource_ledger_section(root, Some(chapter))?);

    out.push_str("\n## Rewrite checklist\n\n");
    out.push_str("- Preserve character knowledge boundaries unless the rewrite explicitly changes what they learn.\n");
    out.push_str("- Re-check downstream chapters that mention affected characters, locations, promises, or events.\n");
    out.push_str("- After rewriting, run `deepseek remember ");
    out.push_str(&chapter.to_string());
    out.push_str("` and review `deepseek memory candidates --chapter ");
    out.push_str(&chapter.to_string());
    out.push_str("` before applying updates.\n\n");
    Ok(out)
}

fn resource_ledger_report(root: &Path, chapter: Option<u32>) -> Result<String> {
    let mut out = String::new();
    out.push_str("# Resource Ledger\n\n");
    out.push_str("- boundary: derived from resource cards, memory graph, and candidate object-state updates.\n");
    if let Some(chapter) = chapter {
        let _ = writeln!(out, "- chapter_filter: {chapter:03}");
    }
    out.push('\n');
    out.push_str(&resource_ledger_section(root, chapter)?);
    Ok(out)
}

fn resource_ledger_section(root: &Path, chapter: Option<u32>) -> Result<String> {
    let graph = load_or_build_memory_graph(root)?;
    let candidates = memory_candidates_for_display(root, chapter, true)?;
    let mut out = String::new();
    out.push_str("## Resource Economy Impact\n\n");
    let resources = graph
        .nodes
        .iter()
        .filter(|node| node.kind == "object" && node.source.starts_with("cards/resources"))
        .collect::<Vec<_>>();
    if resources.is_empty() {
        out.push_str("- resource_cards: none\n");
    } else {
        out.push_str("- resource_cards:\n");
        for resource in resources.iter().take(12) {
            let value = state_string(resource, &["market_value", "ordinary_income_equivalent"])
                .unwrap_or_else(|| "value unknown".to_string());
            let controller = state_string(resource, &["who_controls_it"])
                .unwrap_or_else(|| "controller unknown".to_string());
            let cost = state_string(resource, &["cost_to_use", "debt_or_obligation"])
                .unwrap_or_else(|| "cost/obligation unknown".to_string());
            let _ = writeln!(
                out,
                "  - {} | value: {} | controller: {} | cost: {}",
                resource.label, value, controller, cost
            );
        }
    }

    let resource_candidates = candidates
        .iter()
        .filter(|candidate| candidate_is_resource_related(candidate))
        .collect::<Vec<_>>();
    out.push_str("- resource_changes:\n");
    if resource_candidates.is_empty() {
        out.push_str("  - none\n");
    } else {
        for candidate in resource_candidates.iter().take(16) {
            let chapter = candidate
                .chapter
                .map(|value| format!("{value:03}"))
                .unwrap_or_else(|| "???".to_string());
            let _ = writeln!(
                out,
                "  - chapter {} [{}] {} -> {}",
                chapter, candidate.kind, candidate.target, candidate.change
            );
            let impact = resource_candidate_impact(candidate, &graph);
            if !impact.is_empty() {
                let _ = writeln!(out, "    impact: {}", impact.join(", "));
            }
            if candidate.affects.is_empty() {
                out.push_str("    affects: missing explicit affects list\n");
            } else {
                let _ = writeln!(out, "    affects: {}", candidate.affects.join(", "));
            }
        }
    }
    Ok(out)
}

fn candidate_is_resource_related(candidate: &MemoryUpdateCandidate) -> bool {
    candidate.kind == "object_state"
        || count_terms(
            &format!(
                "{} {} {}",
                candidate.target, candidate.change, candidate.evidence
            ),
            XIANXIA_RESOURCE_TERMS,
        ) > 0
}

fn resource_candidate_impact(
    candidate: &MemoryUpdateCandidate,
    graph: &MemoryGraph,
) -> Vec<String> {
    let haystack = format!("{} {}", candidate.target, candidate.change);
    let mut impacts = Vec::new();
    for node in &graph.nodes {
        if matches!(
            node.kind.as_str(),
            "character" | "promise" | "relationship" | "world" | "location"
        ) && haystack.contains(node.label.trim())
            && node.label.chars().count() >= 2
        {
            impacts.push(format!("{}:{}", node.kind, node.label));
        }
    }
    for affect in &candidate.affects {
        impacts.push(resource_impact_label(affect, graph));
    }
    impacts.sort();
    impacts.dedup();
    impacts.truncate(12);
    impacts
}

fn resource_impact_label(affect: &str, graph: &MemoryGraph) -> String {
    let trimmed = affect.trim();
    if let Some(node) = graph.nodes.iter().find(|node| node.id == trimmed) {
        return format!("{}:{}", node.kind, node.label);
    }
    if let Some((kind, raw)) = trimmed.split_once(':')
        && let Some(node) = graph.nodes.iter().find(|node| {
            node.kind == kind
                && (sanitize_graph_id(&node.label) == raw
                    || Path::new(&node.source)
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .is_some_and(|stem| stem == raw || sanitize_graph_id(stem) == raw))
        })
    {
        return format!("{}:{}", node.kind, node.label);
    }
    trimmed.to_string()
}

fn memory_snapshot_for_graph(root: PathBuf, graph: MemoryGraph) -> Result<NovelMemorySnapshot> {
    let manifest = load_manifest(&root)?;
    let chapters = collect_chapter_dirs(&root)?;
    let current_chapter = manifest.current_chapter.max(
        chapters
            .iter()
            .map(|(chapter, _)| *chapter)
            .max()
            .unwrap_or(0),
    );
    let next_chapter = current_chapter.saturating_add(1).max(1);
    let pending_candidates = collect_memory_update_candidates(&root)
        .map(|candidates| {
            candidates
                .into_iter()
                .filter(|candidate| !candidate_is_applied(candidate))
                .count()
        })
        .unwrap_or_else(|_| graph.candidate_updates.len());
    let schema_status = memory_schema_validation_status(&graph);
    let readiness = workflow_readiness(
        &root,
        &manifest,
        &chapters,
        current_chapter,
        next_chapter,
        memory_graph_path(&root).is_file(),
        &schema_status,
        pending_candidates,
    )?;
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for node in &graph.nodes {
        *kinds.entry(node.kind.clone()).or_insert(0) += 1;
    }
    let mut top_kinds: Vec<(String, usize)> = kinds.into_iter().collect();
    top_kinds.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    top_kinds.truncate(8);

    let mut degree: BTreeMap<&str, usize> = BTreeMap::new();
    for edge in &graph.edges {
        *degree.entry(&edge.source).or_insert(0) += 1;
        *degree.entry(&edge.target).or_insert(0) += 1;
    }
    let labels: BTreeMap<&str, &str> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node.label.as_str()))
        .collect();
    let mut top_hubs: Vec<(String, usize)> = degree
        .into_iter()
        .map(|(id, count)| (labels.get(id).copied().unwrap_or(id).to_string(), count))
        .collect();
    top_hubs.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    top_hubs.truncate(8);

    let health = memory_graph_health(&graph);

    Ok(NovelMemorySnapshot {
        graph_path: memory_graph_path(&root),
        graph_ready: true,
        schema_status,
        updated_at: if graph.updated_at.is_empty() {
            None
        } else {
            Some(graph.updated_at.clone())
        },
        nodes: graph.nodes.len(),
        edges: graph.edges.len(),
        candidate_updates: graph.candidate_updates.len(),
        top_kinds,
        top_hubs,
        summaries: count_files_with_extension(&root.join("memory/summaries"), "md")?,
        reports: count_files_with_extension(&root.join("memory/reports"), "md")?,
        facts: count_jsonl_records(&root.join("memory/facts.jsonl"))?,
        events: count_jsonl_records(&root.join("memory/events.jsonl"))?,
        foreshadowing: count_jsonl_records(&root.join("memory/foreshadowing.jsonl"))?,
        promise_statuses: health.promise_statuses,
        relationship_changes: health.relationship_changes,
        state_changes: health.state_changes,
        relationship_previews: health.relationship_previews,
        state_change_previews: health.state_change_previews,
        readiness,
        root,
    })
}

fn memory_schema_validation_status(graph: &MemoryGraph) -> String {
    let mut issues = Vec::new();
    if graph.schema_version != 2 {
        issues.push(format!("expected v2, got v{}", graph.schema_version));
    }
    if graph.updated_at.trim().is_empty() {
        issues.push("missing updated_at".to_string());
    }

    let node_ids = graph
        .nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect::<BTreeSet<_>>();
    if node_ids.len() != graph.nodes.len() {
        issues.push("duplicate node ids".to_string());
    }
    for node in graph.nodes.iter().take(500) {
        if node.id.trim().is_empty() {
            issues.push("node with empty id".to_string());
            break;
        }
        if node.kind.trim().is_empty() {
            issues.push(format!("node {} has empty kind", node.id));
            break;
        }
        if node.label.trim().is_empty() {
            issues.push(format!("node {} has empty label", node.id));
            break;
        }
    }
    for edge in graph.edges.iter().take(1000) {
        if edge.kind.trim().is_empty() {
            issues.push("edge with empty kind".to_string());
            break;
        }
        if should_validate_memory_endpoint(edge, &edge.source)
            && !node_ids.contains(edge.source.as_str())
        {
            issues.push(format!("edge references missing source {}", edge.source));
            break;
        }
        if should_validate_memory_endpoint(edge, &edge.target)
            && !node_ids.contains(edge.target.as_str())
        {
            issues.push(format!("edge references missing target {}", edge.target));
            break;
        }
        if !(0.0..=1.0).contains(&edge.confidence) {
            issues.push(format!("edge {} confidence out of range", edge.kind));
            break;
        }
    }
    if issues.is_empty() {
        "ok: v2".to_string()
    } else {
        format!("invalid: {}", issues.join("; "))
    }
}

pub(crate) fn memory_schema_validation_report_from_workspace(workspace: &Path) -> Result<String> {
    let root = find_project_root(workspace)?;
    memory_schema_validation_report(&root)
}

pub(crate) fn migrate_memory_workspace_from_workspace(workspace: &Path) -> Result<String> {
    let root = find_project_root(workspace)?;
    migrate_memory_workspace(&root)
}

pub(crate) fn cleanup_memory_workspace_from_workspace(
    workspace: &Path,
    apply: bool,
) -> Result<String> {
    let root = find_project_root(workspace)?;
    cleanup_memory_workspace(&root, apply)
}

#[derive(Debug, Default)]
struct MemoryCleanupReport {
    candidate_files_checked: usize,
    candidate_files_changed: usize,
    candidates_removed: usize,
    summary_files_checked: usize,
    summary_files_changed: usize,
    summary_sections_added: usize,
    ledger_files_checked: usize,
    ledger_files_changed: usize,
    ledger_records_removed: usize,
    backups: Vec<String>,
    changed_files: Vec<String>,
    removed_previews: Vec<String>,
}

fn cleanup_memory_workspace(root: &Path, apply: bool) -> Result<String> {
    let mut report = MemoryCleanupReport::default();
    let backup_root = apply.then(|| {
        root.join("memory/.cleanup")
            .join(chapter_version_timestamp())
    });

    cleanup_candidate_files(root, apply, backup_root.as_deref(), &mut report)?;
    cleanup_summary_files(root, apply, backup_root.as_deref(), &mut report)?;
    cleanup_memory_ledgers(root, apply, backup_root.as_deref(), &mut report)?;

    let graph_status = if apply {
        let graph = build_memory_graph(root)?;
        save_memory_graph(root, &graph)?;
        report
            .changed_files
            .push(display_relative(root, &memory_graph_path(root)));
        Some(memory_schema_validation_status(&graph))
    } else {
        None
    };

    Ok(format_memory_cleanup_report(
        &report,
        apply,
        graph_status.as_deref(),
    ))
}

fn cleanup_candidate_files(
    root: &Path,
    apply: bool,
    backup_root: Option<&Path>,
    report: &mut MemoryCleanupReport,
) -> Result<()> {
    for path in collect_files_with_extensions(&root.join("memory/candidates"), &["json"])? {
        report.candidate_files_checked += 1;
        let raw = read_required(&path)?;
        let value: serde_json::Value = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        let fallback_chapter = value
            .get("chapter")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok());
        let Some(items) = value
            .get("candidates")
            .and_then(serde_json::Value::as_array)
        else {
            continue;
        };

        let mut cleaned = Vec::new();
        let mut removed = 0usize;
        for item in items {
            let parsed = serde_json::from_value::<MemoryUpdateCandidate>(item.clone())
                .ok()
                .or_else(|| memory_candidate_from_json(item, fallback_chapter));
            let Some(mut candidate) = parsed else {
                removed += 1;
                push_cleanup_preview(
                    report,
                    &format!(
                        "{}: removed unparsable candidate {}",
                        display_relative(root, &path),
                        limit_chars(&item.to_string(), 120)
                    ),
                );
                continue;
            };
            if candidate.chapter.is_none() {
                candidate.chapter = fallback_chapter;
            }
            let Some(candidate) = normalize_memory_candidate(candidate) else {
                removed += 1;
                push_cleanup_preview(
                    report,
                    &format!(
                        "{}: removed malformed candidate {}",
                        display_relative(root, &path),
                        limit_chars(&item.to_string(), 120)
                    ),
                );
                continue;
            };
            cleaned.push(serde_json::to_value(candidate).context("failed to encode candidate")?);
        }

        if removed == 0 {
            continue;
        }
        report.candidate_files_changed += 1;
        report.candidates_removed += removed;
        if apply {
            backup_file_for_cleanup(root, &path, backup_root, report)?;
            let mut next = value.clone();
            if let Some(object) = next.as_object_mut() {
                object.insert("schema_version".to_string(), serde_json::json!(1));
                object
                    .entry("status".to_string())
                    .or_insert_with(|| serde_json::Value::String("pending_review".to_string()));
                object.insert("candidates".to_string(), serde_json::Value::Array(cleaned));
            }
            write_text(
                &path,
                &serde_json::to_string_pretty(&next)
                    .context("failed to encode cleaned candidate file")?,
                true,
            )?;
            report.changed_files.push(display_relative(root, &path));
        }
    }
    Ok(())
}

fn cleanup_summary_files(
    root: &Path,
    apply: bool,
    backup_root: Option<&Path>,
    report: &mut MemoryCleanupReport,
) -> Result<()> {
    for path in collect_files_with_extensions(&root.join("memory/summaries"), &["md"])? {
        report.summary_files_checked += 1;
        let raw = read_required(&path)?;
        let cleaned = normalize_memory_summary_output(&raw);
        if cleaned == raw {
            continue;
        }
        report.summary_files_changed += 1;
        report.summary_sections_added += missing_memory_summary_heading_count(&raw);
        if apply {
            backup_file_for_cleanup(root, &path, backup_root, report)?;
            write_text(&path, &cleaned, true)?;
            report.changed_files.push(display_relative(root, &path));
        }
    }
    Ok(())
}

fn cleanup_memory_ledgers(
    root: &Path,
    apply: bool,
    backup_root: Option<&Path>,
    report: &mut MemoryCleanupReport,
) -> Result<()> {
    for rel in [
        "memory/facts.jsonl",
        "memory/events.jsonl",
        "memory/foreshadowing.jsonl",
    ] {
        let path = root.join(rel);
        if !path.is_file() {
            continue;
        }
        report.ledger_files_checked += 1;
        let raw = read_required(&path)?;
        let mut kept = Vec::new();
        let mut removed = 0usize;
        for (index, line) in raw.lines().enumerate() {
            if should_remove_memory_ledger_line(line) {
                removed += 1;
                push_cleanup_preview(
                    report,
                    &format!("{rel}:{} removed {}", index + 1, limit_chars(line, 140)),
                );
            } else {
                kept.push(line.to_string());
            }
        }
        if removed == 0 {
            continue;
        }
        report.ledger_files_changed += 1;
        report.ledger_records_removed += removed;
        if apply {
            backup_file_for_cleanup(root, &path, backup_root, report)?;
            let mut body = kept.join("\n");
            if !body.is_empty() {
                body.push('\n');
            }
            write_text(&path, &body, true)?;
            report.changed_files.push(display_relative(root, &path));
        }
    }
    Ok(())
}

fn missing_memory_summary_heading_count(raw: &str) -> usize {
    REQUIRED_MEMORY_SUMMARY_HEADINGS
        .iter()
        .filter(|heading| !summary_has_heading(raw, heading))
        .count()
}

fn should_remove_memory_ledger_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    if !looks_like_memory_pollution(trimmed) {
        return false;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return true;
    };
    memory_json_value_has_pollution(&value)
}

fn memory_json_value_has_pollution(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(text) => looks_like_memory_pollution(text),
        serde_json::Value::Array(items) => items.iter().any(memory_json_value_has_pollution),
        serde_json::Value::Object(map) => map.iter().any(|(key, value)| {
            if key == "source" || key == "evidence" {
                return false;
            }
            memory_json_value_has_pollution(value)
        }),
        _ => false,
    }
}

fn looks_like_memory_pollution(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    text.contains("候选记忆更新")
        || text.contains("下列变化值得进入候选记忆")
        || text.contains("注意**")
        || text.contains("**注意")
        || text.contains("写法反馈")
        || text.contains("后续风险")
        || lower.contains("candidate_memory_updates")
        || lower.contains("memory_candidate_audit")
        || ((lower.contains("target:") || text.contains("目标："))
            && (lower.contains("change:") || text.contains("变化："))
            && (text.contains("候选") || lower.contains("candidate")))
}

fn backup_file_for_cleanup(
    root: &Path,
    path: &Path,
    backup_root: Option<&Path>,
    report: &mut MemoryCleanupReport,
) -> Result<()> {
    let Some(backup_root) = backup_root else {
        return Ok(());
    };
    let rel = path.strip_prefix(root).unwrap_or(path);
    let target = backup_root.join(rel);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::copy(path, &target).with_context(|| {
        format!(
            "failed to backup {} to {}",
            path.display(),
            target.display()
        )
    })?;
    report.backups.push(display_relative(root, &target));
    Ok(())
}

fn push_cleanup_preview(report: &mut MemoryCleanupReport, line: &str) {
    if report.removed_previews.len() < 12 {
        report.removed_previews.push(line.to_string());
    }
}

fn format_memory_cleanup_report(
    report: &MemoryCleanupReport,
    apply: bool,
    graph_status: Option<&str>,
) -> String {
    let mut out = String::new();
    out.push_str("# Memory Cleanup\n\n");
    let _ = writeln!(out, "- mode: {}", if apply { "apply" } else { "dry-run" });
    let _ = writeln!(
        out,
        "- candidate_files: checked {}, changed {}, removed_candidates {}",
        report.candidate_files_checked, report.candidate_files_changed, report.candidates_removed
    );
    let _ = writeln!(
        out,
        "- summaries: checked {}, changed {}, added_missing_sections {}",
        report.summary_files_checked, report.summary_files_changed, report.summary_sections_added
    );
    let _ = writeln!(
        out,
        "- ledgers: checked {}, changed {}, removed_records {}",
        report.ledger_files_checked, report.ledger_files_changed, report.ledger_records_removed
    );
    if let Some(status) = graph_status {
        let _ = writeln!(out, "- graph_rebuilt: {status}");
    } else {
        out.push_str("- graph_rebuilt: no (dry-run)\n");
    }
    if !report.removed_previews.is_empty() {
        out.push_str("\n## Removed / Rejected Preview\n\n");
        for preview in &report.removed_previews {
            let _ = writeln!(out, "- {preview}");
        }
    }
    if !report.changed_files.is_empty() {
        out.push_str("\n## Changed Files\n\n");
        for path in &report.changed_files {
            let _ = writeln!(out, "- `{path}`");
        }
    }
    if !report.backups.is_empty() {
        out.push_str("\n## Backups\n\n");
        for path in &report.backups {
            let _ = writeln!(out, "- `{path}`");
        }
    }
    if !apply {
        out.push_str("\nRun `deepseek memory cleanup --apply` or `/memory cleanup --apply` to write these changes after reviewing the preview.\n");
    }
    out
}

fn migrate_memory_workspace(root: &Path) -> Result<String> {
    let mut changed = Vec::new();
    let schema_doc = root.join("memory/SCHEMA.md");
    let schema_json = root.join("memory/graph.schema.json");
    write_text(&schema_doc, MEMORY_SCHEMA_DOC, true)?;
    changed.push(display_relative(root, &schema_doc));
    write_text(&schema_json, MEMORY_GRAPH_JSON_SCHEMA, true)?;
    changed.push(display_relative(root, &schema_json));

    let candidate_files =
        collect_files_with_extensions(&root.join("memory/candidates"), &["json"])?;
    let mut migrated_candidates = 0usize;
    for path in candidate_files {
        let raw = read_required(&path)?;
        let mut value: serde_json::Value = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        let mut touched = false;
        if value.get("schema_version").is_none()
            && let Some(object) = value.as_object_mut()
        {
            object.insert("schema_version".to_string(), serde_json::json!(1));
            touched = true;
        }
        if value.get("status").is_none()
            && let Some(object) = value.as_object_mut()
        {
            object.insert(
                "status".to_string(),
                serde_json::Value::String("pending_review".to_string()),
            );
            touched = true;
        }
        if let Some(items) = value
            .get_mut("candidates")
            .and_then(serde_json::Value::as_array_mut)
        {
            for item in items {
                if item.get("status").is_none()
                    && let Some(object) = item.as_object_mut()
                {
                    object.insert(
                        "status".to_string(),
                        serde_json::Value::String("candidate".to_string()),
                    );
                    touched = true;
                }
            }
        }
        if touched {
            write_text(
                &path,
                &serde_json::to_string_pretty(&value)
                    .context("failed to encode migrated candidate file")?,
                true,
            )?;
            migrated_candidates += 1;
        }
    }

    let graph = build_memory_graph(root)?;
    save_memory_graph(root, &graph)?;
    changed.push(display_relative(root, &memory_graph_path(root)));

    let mut out = String::new();
    out.push_str("# Memory Migration\n\n");
    out.push_str("- schema target: v2 graph / v1 candidate files\n");
    let _ = writeln!(out, "- migrated candidate files: {migrated_candidates}");
    out.push_str("- changed files:\n");
    for path in changed {
        let _ = writeln!(out, "  - `{path}`");
    }
    let _ = writeln!(
        out,
        "- validation: {}",
        memory_schema_validation_status(&graph)
    );
    Ok(out)
}

fn memory_schema_validation_report(root: &Path) -> Result<String> {
    let graph = load_or_build_memory_graph(root)?;
    let status = memory_schema_validation_status(&graph);
    let mut out = String::new();
    let _ = writeln!(out, "# Memory Schema Validation\n");
    let _ = writeln!(
        out,
        "- graph: `{}`",
        display_relative(root, &memory_graph_path(root))
    );
    let _ = writeln!(out, "- schema_version: {}", graph.schema_version);
    let _ = writeln!(out, "- status: {status}");
    let _ = writeln!(out, "- nodes: {}", graph.nodes.len());
    let _ = writeln!(out, "- edges: {}", graph.edges.len());
    let _ = writeln!(
        out,
        "- candidate_updates: {}",
        graph.candidate_updates.len()
    );
    let _ = writeln!(
        out,
        "- schema_doc: `{}`",
        display_relative(root, &root.join("memory/SCHEMA.md"))
    );
    let _ = writeln!(
        out,
        "- json_schema: `{}`",
        display_relative(root, &root.join("memory/graph.schema.json"))
    );
    if status.starts_with("ok:") {
        out.push_str("\nThe generated graph satisfies the active v2 contract.\n");
    } else {
        out.push_str("\nRebuild with `deepseek memory build`; if the status remains invalid, inspect the cited graph nodes and edges before applying new candidates.\n");
    }
    Ok(out)
}

fn should_validate_memory_endpoint(edge: &MemoryEdge, value: &str) -> bool {
    if edge.kind == "APPEARS_IN" && value.starts_with("chapter:") {
        return false;
    }
    value == "book"
        || value.starts_with("chapter:")
        || value.starts_with("character:")
        || value.starts_with("secret:")
        || value.starts_with("promise:")
        || value.starts_with("event:")
        || value.starts_with("relationship:")
        || value.starts_with("knowledge:")
        || value.starts_with("character_state:")
        || value.starts_with("location_state:")
        || value.starts_with("object_state:")
}

fn memory_graph_health(graph: &MemoryGraph) -> MemoryGraphHealth {
    MemoryGraphHealth {
        promise_statuses: promise_status_counts(graph),
        relationship_changes: graph
            .nodes
            .iter()
            .filter(|node| {
                node.kind == "relationship"
                    && node
                        .state
                        .get("change")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|value| !value.trim().is_empty())
            })
            .count(),
        state_changes: graph
            .nodes
            .iter()
            .filter(|node| {
                matches!(
                    node.kind.as_str(),
                    "character_state" | "location_state" | "object_state"
                )
            })
            .count(),
        relationship_previews: relationship_change_previews(graph, 4),
        state_change_previews: state_change_previews(graph, 4),
    }
}

#[derive(Debug, Clone)]
struct PromiseLifecycleEntry {
    status: String,
    label: String,
    source: String,
    summary: String,
    first_chapter: Option<u32>,
    payoff_chapter: Option<u32>,
    progress: Option<String>,
    payoff: Option<String>,
    hold_reason: Option<String>,
}

fn memory_promises_packet(root: &Path, graph: &MemoryGraph) -> String {
    let mut entries = promise_lifecycle_entries(graph);
    let counts = promise_status_counts(graph);
    let mut out = String::new();
    out.push_str("# Promise Lifecycle\n\n");
    let _ = writeln!(
        out,
        "- graph: `{}`",
        display_relative(root, &memory_graph_path(root))
    );
    if counts.is_empty() {
        out.push_str("- status_counts: none\n\nNo promise or foreshadowing nodes found.\n");
        return out;
    }
    let _ = writeln!(
        out,
        "- status_counts: {}\n",
        format_promise_status_counts(&counts)
    );

    entries.sort_by(|left, right| {
        promise_status_rank(&left.status)
            .cmp(&promise_status_rank(&right.status))
            .then_with(|| {
                left.first_chapter
                    .unwrap_or(u32::MAX)
                    .cmp(&right.first_chapter.unwrap_or(u32::MAX))
            })
            .then_with(|| left.label.cmp(&right.label))
    });

    let mut grouped: BTreeMap<String, Vec<PromiseLifecycleEntry>> = BTreeMap::new();
    for entry in entries {
        grouped.entry(entry.status.clone()).or_default().push(entry);
    }

    let mut statuses = grouped.keys().cloned().collect::<Vec<_>>();
    statuses.sort_by_key(|status| promise_status_rank(status));
    for status in statuses {
        let Some(group) = grouped.get(&status) else {
            continue;
        };
        let _ = writeln!(
            out,
            "## {} ({})\n",
            promise_status_label(&status),
            group.len()
        );
        for entry in group {
            let first = entry
                .first_chapter
                .map(|chapter| format!("{chapter:03}"))
                .unwrap_or_else(|| "?".to_string());
            let payoff = entry
                .payoff_chapter
                .map(|chapter| format!("{chapter:03}"))
                .unwrap_or_else(|| "?".to_string());
            let _ = writeln!(
                out,
                "- {} | first: {} | payoff: {} | source: `{}`",
                entry.label, first, payoff, entry.source
            );
            if !entry.summary.trim().is_empty() && entry.summary != entry.label {
                let _ = writeln!(out, "  summary: {}", limit_chars(&entry.summary, 220));
            }
            if let Some(progress) = &entry.progress {
                let _ = writeln!(out, "  progress: {}", limit_chars(progress, 220));
            }
            if let Some(payoff) = &entry.payoff {
                let _ = writeln!(out, "  payoff: {}", limit_chars(payoff, 220));
            }
            if let Some(reason) = &entry.hold_reason {
                let _ = writeln!(out, "  hold: {}", limit_chars(reason, 220));
            }
        }
        out.push('\n');
    }

    out.push_str(
        "## Next checks\n\n- Before drafting, carry open/progress promises into `/memory context N`.\n- Before rewriting an early chapter, run `/memory impact N` and verify downstream payoff chapters.\n- After changing promise status, run `deepseek remember N`, review `/memory candidates N`, then `/memory apply N`.\n",
    );
    out
}

fn promise_lifecycle_entries(graph: &MemoryGraph) -> Vec<PromiseLifecycleEntry> {
    graph
        .nodes
        .iter()
        .filter(|node| node.kind == "promise" && !node.id.starts_with("asset:"))
        .map(|node| {
            let status = state_string(node, &["status"])
                .unwrap_or_else(|| "unknown".to_string())
                .to_ascii_lowercase();
            PromiseLifecycleEntry {
                status,
                label: node.label.clone(),
                source: node.source.clone(),
                summary: node.summary.clone(),
                first_chapter: state_chapter(node, &["first_chapter", "chapter"]),
                payoff_chapter: state_chapter(node, &["payoff_chapter"]),
                progress: state_string(node, &["progress", "advance", "advances"]),
                payoff: state_string(node, &["payoff", "payoff_evidence"]),
                hold_reason: state_string(node, &["suspended_reason", "abandoned_reason"]),
            }
        })
        .collect()
}

fn relationship_change_previews(graph: &MemoryGraph, limit: usize) -> Vec<String> {
    let mut values = graph
        .nodes
        .iter()
        .filter(|node| node.kind == "relationship")
        .filter_map(relationship_preview_for_node)
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values.truncate(limit);
    values
}

fn relationship_preview_for_node(node: &MemoryNode) -> Option<String> {
    let change = state_string(node, &["change"])?;
    let chapter = state_chapter(node, &["chapter"])
        .map(|chapter| format!("chapter {chapter:03}: "))
        .unwrap_or_default();
    let target = state_string(node, &["target"])
        .map(|target| format!("{target} -> "))
        .unwrap_or_default();
    Some(format!("{chapter}{target}{}", limit_chars(&change, 180)))
}

fn state_change_previews(graph: &MemoryGraph, limit: usize) -> Vec<String> {
    let mut values = graph
        .nodes
        .iter()
        .filter(|node| {
            matches!(
                node.kind.as_str(),
                "character_state" | "location_state" | "object_state"
            )
        })
        .filter_map(state_change_preview_for_node)
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values.truncate(limit);
    values
}

fn state_change_preview_for_node(node: &MemoryNode) -> Option<String> {
    let change = state_string(node, &["change"])
        .or_else(|| (!node.summary.trim().is_empty()).then(|| node.summary.trim().to_string()))?;
    let chapter = state_chapter(node, &["chapter"])
        .map(|chapter| format!("chapter {chapter:03}: "))
        .unwrap_or_default();
    let target = state_string(node, &["target"])
        .map(|target| format!("{target} -> "))
        .unwrap_or_default();
    Some(format!(
        "{}{}{} [{}]",
        chapter,
        target,
        limit_chars(&change, 180),
        node.kind
    ))
}

fn promise_status_counts(graph: &MemoryGraph) -> Vec<(String, usize)> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for node in graph
        .nodes
        .iter()
        .filter(|node| node.kind == "promise" && !node.id.starts_with("asset:"))
    {
        let status = node
            .state
            .get("status")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("unknown")
            .to_ascii_lowercase();
        *counts.entry(status).or_insert(0) += 1;
    }
    let mut counts = counts.into_iter().collect::<Vec<_>>();
    counts.sort_by_key(|(status, count)| (promise_status_rank(status), std::cmp::Reverse(*count)));
    counts
}

pub(crate) fn format_promise_status_counts(statuses: &[(String, usize)]) -> String {
    statuses
        .iter()
        .map(|(status, count)| format!("{status}={count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn format_audit_layer_counts(counts: &[(String, usize)]) -> String {
    counts
        .iter()
        .map(|(kind, count)| format!("{kind}={count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn promise_status_rank(status: &str) -> u8 {
    match status {
        "new" | "open" => 0,
        "progress" | "advanced" | "advancing" => 1,
        "suspended" | "paused" => 2,
        "payoff" | "paid" | "paid_off" | "resolved" => 3,
        "abandoned" | "dropped" => 4,
        _ => 9,
    }
}

fn archive_manifest_path(root: &Path) -> PathBuf {
    root.join("memory/archives/manifest.json")
}

fn load_archive_manifest(root: &Path) -> Result<MemoryArchiveManifest> {
    let path = archive_manifest_path(root);
    if !path.is_file() {
        return Ok(MemoryArchiveManifest {
            schema_version: 1,
            generated_at: chrono::Utc::now().to_rfc3339(),
            stages: Vec::new(),
        });
    }
    let raw = read_required(&path)?;
    serde_json::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
}

fn save_archive_manifest(root: &Path, manifest: &MemoryArchiveManifest) -> Result<()> {
    write_text(
        &archive_manifest_path(root),
        &serde_json::to_string_pretty(manifest).context("failed to encode archive manifest")?,
        true,
    )
}

fn archived_chapter_stage_map(root: &Path) -> Result<BTreeMap<u32, MemoryArchiveStage>> {
    let mut map = BTreeMap::new();
    for stage in load_archive_manifest(root)?.stages {
        for chapter in stage.start_chapter..=stage.end_chapter {
            map.insert(chapter, stage.clone());
        }
    }
    Ok(map)
}

fn archive_memory_stage(root: &Path, start: u32, end: u32, label: Option<&str>) -> Result<String> {
    if start == 0 || end == 0 || start > end {
        bail!("archive range must be positive and ordered");
    }
    let chapters = collect_chapter_dirs(root)?
        .into_iter()
        .filter(|(chapter, _)| *chapter >= start && *chapter <= end)
        .collect::<Vec<_>>();
    if chapters.is_empty() {
        bail!("no chapters found in range {start:03}-{end:03}");
    }

    let stage_id = format!("stage-{start:03}-{end:03}");
    let stage_label = label
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&stage_id)
        .to_string();
    let summary_rel = format!("memory/archives/{stage_id}.md");
    let summary_path = root.join(&summary_rel);
    let mut body = String::new();
    let _ = writeln!(body, "# Memory Archive: {stage_label}\n");
    let _ = writeln!(body, "- range: chapters {start:03}-{end:03}");
    let _ = writeln!(body, "- generated_at: {}", chrono::Utc::now().to_rfc3339());
    body.push_str("- role: compact stage memory for long-form context control\n\n");
    body.push_str("## Chapter Summaries\n\n");
    for (chapter, dir) in &chapters {
        let summary = root
            .join("memory/summaries")
            .join(format!("{chapter:03}.md"));
        let text = read_optional_limited(&summary, 1_200)
            .or_else(|| read_optional_limited(&dir.join("final.md"), 1_200))
            .or_else(|| read_optional_limited(&dir.join("draft.md"), 1_200))
            .unwrap_or_else(|| "No summary or chapter text found.".to_string());
        let _ = writeln!(body, "### Chapter {chapter:03}\n");
        let _ = writeln!(body, "{}\n", limit_chars(&text, 1_200));
    }

    let graph = load_or_build_memory_graph(root)?;
    let labels = chapters
        .iter()
        .map(|(chapter, _)| *chapter)
        .collect::<BTreeSet<_>>();
    body.push_str("## Stage Story Surfaces\n\n");
    append_archive_surface_section(&mut body, &graph, &labels, "character", "Characters");
    append_archive_surface_section(&mut body, &graph, &labels, "relationship", "Relationships");
    append_archive_surface_section(&mut body, &graph, &labels, "promise", "Promises");
    append_archive_surface_section(&mut body, &graph, &labels, "event", "Events");
    append_archive_surface_section(&mut body, &graph, &labels, "knowledge", "Knowledge");
    append_archive_surface_section(&mut body, &graph, &labels, "secret", "Secrets");

    write_text(&summary_path, &body, true)?;

    let mut manifest = load_archive_manifest(root)?;
    manifest.schema_version = 1;
    manifest.generated_at = chrono::Utc::now().to_rfc3339();
    manifest.stages.retain(|stage| stage.id != stage_id);
    manifest.stages.push(MemoryArchiveStage {
        id: stage_id.clone(),
        label: stage_label.clone(),
        start_chapter: start,
        end_chapter: end,
        summary_path: summary_rel.clone(),
        chapters: chapters.iter().map(|(chapter, _)| *chapter).collect(),
    });
    manifest
        .stages
        .sort_by_key(|stage| (stage.start_chapter, stage.end_chapter));
    save_archive_manifest(root, &manifest)?;

    let graph = build_memory_graph(root)?;
    save_memory_graph(root, &graph)?;

    let mut out = String::new();
    out.push_str("# Memory Archive Created\n\n");
    let _ = writeln!(out, "- stage: `{stage_id}`");
    let _ = writeln!(out, "- label: {stage_label}");
    let _ = writeln!(out, "- range: chapters {start:03}-{end:03}");
    let _ = writeln!(out, "- summary: `{summary_rel}`");
    let _ = writeln!(
        out,
        "- manifest: `{}`",
        display_relative(root, &archive_manifest_path(root))
    );
    let _ = writeln!(
        out,
        "- graph: `{}` rebuilt",
        display_relative(root, &memory_graph_path(root))
    );
    out.push_str("\nArchived chapters remain on disk, but memory graph rebuilds now keep them as compact stage-linked chapter nodes.\n");
    Ok(out)
}

fn append_archive_surface_section(
    out: &mut String,
    graph: &MemoryGraph,
    chapters: &BTreeSet<u32>,
    kind: &str,
    title: &str,
) {
    let mut values = graph
        .nodes
        .iter()
        .filter(|node| node.kind == kind && !node.id.starts_with("asset:"))
        .filter(|node| {
            state_chapter(node, &["chapter", "first_chapter"])
                .is_some_and(|chapter| chapters.contains(&chapter))
        })
        .map(|node| {
            if !node.summary.trim().is_empty() && node.summary != node.label {
                format!("{} — {}", node.label, limit_chars(&node.summary, 180))
            } else {
                node.label.clone()
            }
        })
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values.truncate(24);
    let _ = writeln!(out, "### {title}\n");
    if values.is_empty() {
        out.push_str("- none found in structured memory\n\n");
    } else {
        for value in values {
            let _ = writeln!(out, "- {value}");
        }
        out.push('\n');
    }
}

fn build_memory_graph(root: &Path) -> Result<MemoryGraph> {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut node_ids = BTreeSet::new();
    let archived_chapters = archived_chapter_stage_map(root)?;

    let title = load_manifest(root)
        .map(|manifest| manifest.title)
        .unwrap_or_else(|_| "Novel".to_string());
    push_memory_node(
        &mut nodes,
        &mut node_ids,
        MemoryNode {
            id: "book".to_string(),
            kind: "book".to_string(),
            label: title,
            source: "book.toml".to_string(),
            summary: "Book root".to_string(),
            state: serde_json::Value::Null,
            hash: String::new(),
        },
    );

    for path in [
        "bible/premise.md",
        "bible/world.md",
        "bible/reader_promise.md",
        "bible/style.md",
        "outline/master_plan.md",
        "outline/chapter_index.md",
        "memory/facts.jsonl",
        "memory/events.jsonl",
        "memory/foreshadowing.jsonl",
        "memory/graph.schema.json",
        "memory/archives/manifest.json",
    ] {
        if let Some(text) = read_optional_limited(&root.join(path), CONTEXT_LIMIT) {
            let id = memory_file_node_id(path);
            push_memory_node(
                &mut nodes,
                &mut node_ids,
                MemoryNode {
                    id: id.clone(),
                    kind: memory_kind_for_path(path).to_string(),
                    label: path.to_string(),
                    source: path.to_string(),
                    summary: memory_asset_summary_for_graph(path, &text, 280),
                    state: serde_json::Value::Null,
                    hash: stable_hash(&text),
                },
            );
            push_memory_edge(&mut edges, "CONTAINS", "book", &id, path, 1.0);
        }
    }

    for dir in [
        "cards/characters",
        "cards/world",
        "cards/locations",
        "cards/resources",
    ] {
        for path in collect_asset_files(&root.join(dir))? {
            if is_template_asset(&path) {
                continue;
            }
            if let Some(text) = read_optional_limited(&path, CONTEXT_LIMIT) {
                let rel = display_relative(root, &path);
                let id = memory_file_node_id(&rel);
                let state = card_state_for_graph(&rel, &text);
                push_memory_node(
                    &mut nodes,
                    &mut node_ids,
                    MemoryNode {
                        id: id.clone(),
                        kind: memory_kind_for_path(&rel).to_string(),
                        label: card_label(&rel, &text),
                        source: rel.clone(),
                        summary: summarize_for_graph(&text, 360),
                        state: state.clone(),
                        hash: stable_hash(&text),
                    },
                );
                push_memory_edge(&mut edges, "CONTAINS", "book", &id, &rel, 1.0);
                add_card_semantic_edges(&mut nodes, &mut node_ids, &mut edges, &id, &rel, &state);
            }
        }
    }

    add_jsonl_memory_nodes(root, &mut nodes, &mut node_ids, &mut edges)?;
    add_memory_archive_nodes(root, &mut nodes, &mut node_ids, &mut edges)?;

    for (chapter, dir) in collect_chapter_dirs(root)? {
        let id = chapter_node_id(chapter);
        let mut parts = Vec::new();
        let archived_stage = archived_chapters.get(&chapter);
        if let Some(stage) = archived_stage {
            parts.push(format!(
                "archived: {} ({})",
                stage.label, stage.summary_path
            ));
            if let Some(text) = read_optional_limited(&root.join(&stage.summary_path), 1_200) {
                parts.push(format!("stage memory: {}", summarize_for_graph(&text, 240)));
            }
        } else {
            for file in ["brief.md", "craft_plan.md", "audit.md"] {
                if let Some(text) = read_optional_limited(&dir.join(file), 2_000) {
                    parts.push(format!("{file}: {}", summarize_for_graph(&text, 180)));
                }
            }
            if let Some(text) = read_optional_limited(&dir.join("final.md"), 4_000)
                .or_else(|| read_optional_limited(&dir.join("draft.md"), 4_000))
            {
                parts.push(format!("text: {}", summarize_for_graph(&text, 260)));
            }
            if let Some(text) = read_optional_limited(
                &root
                    .join("memory/summaries")
                    .join(format!("{chapter:03}.md")),
                4_000,
            ) {
                parts.push(format!("memory: {}", summarize_for_graph(&text, 320)));
            }
        }
        let summary = if parts.is_empty() {
            format!("Chapter {chapter:03}")
        } else {
            parts.join(" | ")
        };
        push_memory_node(
            &mut nodes,
            &mut node_ids,
            MemoryNode {
                id: id.clone(),
                kind: "chapter".to_string(),
                label: format!("Chapter {chapter:03}"),
                source: format!("chapters/{chapter:03}"),
                summary,
                state: serde_json::json!({
                    "number": chapter,
                    "has_brief": dir.join("brief.md").is_file(),
                    "has_draft": dir.join("draft.md").is_file(),
                    "has_final": dir.join("final.md").is_file(),
                    "has_audit": dir.join("audit.md").is_file(),
                    "has_summary": root.join("memory/summaries").join(format!("{chapter:03}.md")).is_file(),
                    "archived_stage": archived_stage.map(|stage| stage.id.clone()),
                    "archive_summary": archived_stage.map(|stage| stage.summary_path.clone()),
                }),
                hash: stable_hash(&format!("{}", dir.display())),
            },
        );
        push_memory_edge(
            &mut edges,
            "CONTAINS",
            "book",
            &id,
            &format!("chapters/{chapter:03}"),
            1.0,
        );
        if let Some(stage) = archived_stage {
            push_memory_edge(
                &mut edges,
                "SUMMARIZED_BY",
                &id,
                &memory_file_node_id(&stage.summary_path),
                &stage.summary_path,
                0.95,
            );
        }
        if chapter > 1 {
            push_memory_edge(
                &mut edges,
                "NEXT",
                &chapter_node_id(chapter - 1),
                &id,
                "chapter order",
                1.0,
            );
        }
    }

    add_textual_memory_edges(&nodes, &mut edges);
    dedupe_memory_edges(&mut edges);

    Ok(MemoryGraph {
        schema_version: 2,
        updated_at: chrono::Utc::now().to_rfc3339(),
        nodes,
        edges,
        candidate_updates: collect_memory_update_candidates(root)?
            .into_iter()
            .filter(|candidate| !candidate_is_applied(candidate))
            .collect(),
    })
}

fn push_memory_node(nodes: &mut Vec<MemoryNode>, ids: &mut BTreeSet<String>, node: MemoryNode) {
    if ids.insert(node.id.clone()) {
        nodes.push(node);
    }
}

fn push_memory_edge(
    edges: &mut Vec<MemoryEdge>,
    kind: &str,
    source: &str,
    target: &str,
    evidence: &str,
    confidence: f32,
) {
    if source == target || source.is_empty() || target.is_empty() {
        return;
    }
    edges.push(MemoryEdge {
        kind: kind.to_string(),
        source: source.to_string(),
        target: target.to_string(),
        evidence: evidence.to_string(),
        confidence,
        note: None,
    });
}

fn card_state_for_graph(path: &str, text: &str) -> serde_json::Value {
    if path.starts_with("cards/resources") {
        let mut state = serde_json::Map::new();
        for key in [
            "id",
            "name",
            "alias",
            "aliases",
            "aka",
            "short_name",
            "nicknames",
            "category",
            "rarity",
            "market_value",
            "ordinary_income_equivalent",
            "who_controls_it",
            "cost_to_use",
            "debt_or_obligation",
            "first_seen",
            "last_changed",
            "canon_status",
            "evidence",
        ] {
            if let Some(value) = yaml_scalar_value(text, key) {
                state.insert(key.to_string(), serde_json::Value::String(value));
            }
        }
        let has_value_anchor = resource_card_has_value_anchor(text);
        let has_control_anchor = resource_card_has_control_anchor(text);
        state.insert("resource_card".to_string(), serde_json::Value::Bool(true));
        state.insert(
            "has_value_anchor".to_string(),
            serde_json::Value::Bool(has_value_anchor),
        );
        state.insert(
            "has_control_anchor".to_string(),
            serde_json::Value::Bool(has_control_anchor),
        );
        return serde_json::Value::Object(state);
    }

    if path.starts_with("cards/locations") {
        let mut state = serde_json::Map::new();
        for key in [
            "id",
            "name",
            "title",
            "alias",
            "aliases",
            "aka",
            "short_name",
            "status",
            "state",
            "last_changed",
            "canon_status",
            "evidence",
        ] {
            if let Some(value) = yaml_scalar_value(text, key) {
                state.insert(key.to_string(), serde_json::Value::String(value));
            }
        }
        return if state.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::Value::Object(state)
        };
    }

    if !path.starts_with("cards/characters") {
        return serde_json::Value::Null;
    }

    let mut state = serde_json::Map::new();
    for (source_key, output_key) in [
        ("id", "id"),
        ("name", "name"),
        ("alias", "alias"),
        ("aka", "aka"),
        ("short_name", "short_name"),
        ("title", "title"),
        ("role", "role"),
        ("want", "want"),
        ("need", "need"),
        ("fear", "fear"),
        ("secret", "secret"),
        ("state", "current_state"),
        ("voice", "voice"),
        ("last_seen_chapter", "last_seen_chapter"),
    ] {
        if let Some(value) = yaml_scalar_value(text, source_key) {
            state.insert(output_key.to_string(), serde_json::Value::String(value));
        }
    }
    for (source_key, output_key) in [
        ("aliases", "aliases"),
        ("nicknames", "nicknames"),
        ("knowledge", "knowledge"),
        ("unknown", "unknown"),
        ("does_not_know", "unknown"),
        ("relationships", "relationships"),
    ] {
        let values = yaml_list_or_scalar_values(text, source_key);
        if !values.is_empty() {
            state.insert(
                output_key.to_string(),
                serde_json::Value::Array(
                    values
                        .into_iter()
                        .map(serde_json::Value::String)
                        .collect::<Vec<_>>(),
                ),
            );
        }
    }
    if state.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::Value::Object(state)
    }
}

fn add_card_semantic_edges(
    nodes: &mut Vec<MemoryNode>,
    node_ids: &mut BTreeSet<String>,
    edges: &mut Vec<MemoryEdge>,
    character_id: &str,
    source: &str,
    state: &serde_json::Value,
) {
    let Some(state) = state.as_object() else {
        return;
    };
    for (field, edge_kind, node_kind) in [
        ("knowledge", "KNOWS", "knowledge"),
        ("unknown", "DOES_NOT_KNOW", "knowledge"),
        ("relationships", "AFFECTS", "relationship"),
    ] {
        let Some(values) = state.get(field).and_then(serde_json::Value::as_array) else {
            continue;
        };
        for value in values {
            let Some(label) = value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            let id = format!("{node_kind}:{}", sanitize_graph_id(label));
            push_memory_node(
                nodes,
                node_ids,
                MemoryNode {
                    id: id.clone(),
                    kind: node_kind.to_string(),
                    label: label.to_string(),
                    source: source.to_string(),
                    summary: label.to_string(),
                    state: serde_json::json!({ "from": source }),
                    hash: stable_hash(label),
                },
            );
            push_memory_edge(edges, edge_kind, character_id, &id, source, 0.82);
        }
    }

    for (field, edge_kind, node_kind) in [
        ("want", "WANTS", "desire"),
        ("fear", "FEARS", "fear"),
        ("secret", "KNOWS", "secret"),
        ("who_controls_it", "CONTROLLED_BY", "character"),
        ("debt_or_obligation", "OBLIGATES", "memory"),
    ] {
        let Some(label) = state.get(field).and_then(serde_json::Value::as_str) else {
            continue;
        };
        let label = label.trim();
        if label.is_empty() {
            continue;
        }
        let id = format!("{node_kind}:{}", sanitize_graph_id(label));
        push_memory_node(
            nodes,
            node_ids,
            MemoryNode {
                id: id.clone(),
                kind: node_kind.to_string(),
                label: label.to_string(),
                source: source.to_string(),
                summary: label.to_string(),
                state: serde_json::json!({ "from": source }),
                hash: stable_hash(label),
            },
        );
        push_memory_edge(edges, edge_kind, character_id, &id, source, 0.82);
    }
}

fn add_jsonl_memory_nodes(
    root: &Path,
    nodes: &mut Vec<MemoryNode>,
    node_ids: &mut BTreeSet<String>,
    edges: &mut Vec<MemoryEdge>,
) -> Result<()> {
    for (rel, default_kind) in [
        ("memory/facts.jsonl", "knowledge"),
        ("memory/events.jsonl", "event"),
        ("memory/foreshadowing.jsonl", "promise"),
    ] {
        let path = root.join(rel);
        let Some(raw) = std::fs::read_to_string(&path).ok() else {
            continue;
        };
        let parent_id = memory_file_node_id(rel);
        for (index, line) in raw.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let parsed = serde_json::from_str::<serde_json::Value>(line).ok();
            let kind = parsed
                .as_ref()
                .and_then(|value| value.get("kind"))
                .and_then(serde_json::Value::as_str)
                .map(canonical_memory_kind)
                .unwrap_or_else(|| default_kind.to_string());
            let label = parsed
                .as_ref()
                .and_then(memory_label_from_json)
                .unwrap_or_else(|| summarize_for_graph(line, 80));
            let summary = parsed
                .as_ref()
                .map(memory_summary_from_json)
                .unwrap_or_else(|| summarize_for_graph(line, 360));
            let source = format!("{rel}:{}", index + 1);
            let id = format!(
                "{kind}:{}",
                sanitize_graph_id(&format!("{rel}:{}", index + 1))
            );
            let mut state = parsed.unwrap_or_else(|| serde_json::json!({ "text": line }));
            add_derived_memory_aliases(&mut state);
            push_memory_node(
                nodes,
                node_ids,
                MemoryNode {
                    id: id.clone(),
                    kind: kind.clone(),
                    label: label.clone(),
                    source: source.clone(),
                    summary,
                    state: state.clone(),
                    hash: stable_hash(line),
                },
            );
            push_memory_edge(edges, "CONTAINS", &parent_id, &id, &source, 0.95);
            if kind == "promise" {
                push_memory_edge(edges, "PROMISES", "book", &id, &source, 0.82);
            } else if kind == "event" {
                push_memory_edge(edges, "CAUSES", &parent_id, &id, &source, 0.65);
            }
            add_structured_memory_semantic_edges(
                nodes, node_ids, edges, &id, &kind, &label, &source, &state,
            );
        }
    }
    Ok(())
}

fn add_memory_archive_nodes(
    root: &Path,
    nodes: &mut Vec<MemoryNode>,
    node_ids: &mut BTreeSet<String>,
    edges: &mut Vec<MemoryEdge>,
) -> Result<()> {
    for stage in load_archive_manifest(root)?.stages {
        let path = root.join(&stage.summary_path);
        let text = read_optional_limited(&path, 6_000).unwrap_or_default();
        let id = memory_file_node_id(&stage.summary_path);
        push_memory_node(
            nodes,
            node_ids,
            MemoryNode {
                id: id.clone(),
                kind: "memory_archive".to_string(),
                label: stage.label.clone(),
                source: stage.summary_path.clone(),
                summary: summarize_for_graph(&text, 420),
                state: serde_json::json!({
                    "stage_id": stage.id,
                    "start_chapter": stage.start_chapter,
                    "end_chapter": stage.end_chapter,
                    "chapters": stage.chapters,
                }),
                hash: stable_hash(&text),
            },
        );
        push_memory_edge(edges, "CONTAINS", "book", &id, &stage.summary_path, 1.0);
        for chapter in stage.start_chapter..=stage.end_chapter {
            push_memory_edge(
                edges,
                "SUMMARIZES",
                &id,
                &chapter_node_id(chapter),
                &stage.summary_path,
                0.92,
            );
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn add_structured_memory_semantic_edges(
    nodes: &mut Vec<MemoryNode>,
    node_ids: &mut BTreeSet<String>,
    edges: &mut Vec<MemoryEdge>,
    memory_id: &str,
    kind: &str,
    label: &str,
    source: &str,
    state: &serde_json::Value,
) {
    match kind {
        "knowledge" => {
            if let Some(target_id) =
                ensure_memory_target_node(nodes, node_ids, state, "character", source)
            {
                push_memory_edge(edges, "KNOWS", &target_id, memory_id, source, 0.82);
            }
        }
        "relationship" => {
            for participant in relationship_participants(state, label) {
                let target_id = ensure_labeled_memory_node(
                    nodes,
                    node_ids,
                    "character",
                    &participant,
                    source,
                    0.80,
                );
                push_memory_edge(edges, "AFFECTS", memory_id, &target_id, source, 0.78);
            }
        }
        "character_state" => {
            if let Some(target_id) =
                ensure_memory_target_node(nodes, node_ids, state, "character", source)
            {
                push_memory_edge(edges, "CHANGES", memory_id, &target_id, source, 0.86);
            }
        }
        "location_state" => {
            if let Some(target_id) =
                ensure_memory_target_node(nodes, node_ids, state, "location", source)
            {
                push_memory_edge(edges, "CHANGES", memory_id, &target_id, source, 0.86);
            }
        }
        "object_state" => {
            if let Some(target_id) =
                ensure_memory_target_node(nodes, node_ids, state, "object", source)
            {
                push_memory_edge(edges, "CHANGES", memory_id, &target_id, source, 0.86);
            }
        }
        _ => {}
    }

    if let Some(chapter) = state
        .get("chapter")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
    {
        push_memory_edge(
            edges,
            "APPEARS_IN",
            memory_id,
            &chapter_node_id(chapter),
            source,
            0.78,
        );
    }
}

fn ensure_memory_target_node(
    nodes: &mut Vec<MemoryNode>,
    node_ids: &mut BTreeSet<String>,
    state: &serde_json::Value,
    kind: &str,
    source: &str,
) -> Option<String> {
    let label = state
        .get("target")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    Some(ensure_labeled_memory_node(
        nodes, node_ids, kind, label, source, 0.80,
    ))
}

fn ensure_labeled_memory_node(
    nodes: &mut Vec<MemoryNode>,
    node_ids: &mut BTreeSet<String>,
    kind: &str,
    label: &str,
    source: &str,
    confidence: f32,
) -> String {
    if let Some(existing) = nodes
        .iter()
        .find(|node| node.kind == kind && node.label == label)
    {
        return existing.id.clone();
    }
    let id = format!("{kind}:{}", sanitize_graph_id(label));
    push_memory_node(
        nodes,
        node_ids,
        MemoryNode {
            id: id.clone(),
            kind: kind.to_string(),
            label: label.to_string(),
            source: source.to_string(),
            summary: format!("Inferred {kind} from memory ledger"),
            state: serde_json::json!({ "from": source, "confidence": confidence }),
            hash: stable_hash(&format!("{kind}:{label}")),
        },
    );
    id
}

fn relationship_participants(state: &serde_json::Value, fallback: &str) -> Vec<String> {
    let target = state
        .get("target")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(fallback);
    target
        .split(['/', '／', '&', '、', ',', '，'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .take(4)
        .map(str::to_string)
        .collect()
}

fn collect_memory_update_candidates(root: &Path) -> Result<Vec<MemoryUpdateCandidate>> {
    let mut candidates = Vec::new();
    candidates.extend(read_structured_memory_candidates(root)?);
    for (chapter, dir) in collect_chapter_dirs(root)? {
        for file in ["audit.md", "craft_plan.md"] {
            let path = dir.join(file);
            let Some(text) = read_optional_limited(&path, 24_000) else {
                continue;
            };
            let lines = candidate_update_lines(&text);
            for line in lines {
                let Some(candidate) =
                    memory_candidate_from_line(chapter, &display_relative(root, &path), line)
                else {
                    continue;
                };
                candidates.push(candidate);
            }
        }
    }
    candidates.truncate(200);
    Ok(candidates)
}

fn write_memory_candidate_file(
    root: &Path,
    chapter: u32,
    source_path: &Path,
    summary: &str,
) -> Result<usize> {
    write_memory_candidate_file_with_fallback(root, chapter, source_path, summary, None)
}

fn write_memory_candidate_file_with_fallback(
    root: &Path,
    chapter: u32,
    source_path: &Path,
    summary: &str,
    fallback_chapter_text: Option<&str>,
) -> Result<usize> {
    let rel = display_relative(root, source_path);
    let lines = candidate_update_lines(summary);
    let has_candidate_section = has_candidate_memory_section(summary);
    let mut candidates = lines
        .iter()
        .copied()
        .filter_map(|line| memory_candidate_from_line(chapter, &rel, line))
        .collect::<Vec<_>>();
    if candidates.is_empty()
        && let Some(chapter_text) = fallback_chapter_text
        && !has_candidate_section
    {
        candidates = deterministic_memory_candidates(chapter, &rel, summary, chapter_text);
    }
    let path = root
        .join("memory/candidates")
        .join(format!("{chapter:03}.json"));
    let body = serde_json::to_string_pretty(&serde_json::json!({
        "schema_version": 1,
        "chapter": chapter,
        "source": rel,
        "status": "pending_review",
        "candidates": candidates,
    }))
    .context("failed to encode memory candidates")?;
    if path.exists() {
        let _ = snapshot_memory_artifact_before_write(root, chapter, &path, "pre-candidates")?;
    }
    write_text(&path, &body, true)?;
    Ok(candidates.len())
}

fn deterministic_memory_candidates(
    chapter: u32,
    source: &str,
    _summary: &str,
    chapter_text: &str,
) -> Vec<MemoryUpdateCandidate> {
    let mut candidates = Vec::new();
    let mut seen = BTreeSet::new();
    collect_deterministic_memory_candidates(
        chapter,
        source,
        chapter_text,
        "chapter",
        5,
        &mut seen,
        &mut candidates,
    );
    candidates.truncate(5);
    candidates
}

fn collect_deterministic_memory_candidates(
    chapter: u32,
    source: &str,
    text: &str,
    source_kind: &str,
    limit: usize,
    seen: &mut BTreeSet<String>,
    out: &mut Vec<MemoryUpdateCandidate>,
) {
    let mut heading = String::new();
    for raw in text.lines() {
        let trimmed = raw.trim();
        if trimmed.starts_with('#') {
            heading = trimmed.trim_start_matches('#').trim().to_string();
            continue;
        }
        if out.len() >= limit {
            break;
        }
        let Some(candidate) = deterministic_memory_candidate_from_line(
            chapter,
            source,
            &heading,
            trimmed,
            source_kind,
        ) else {
            continue;
        };
        let key = format!(
            "{}\u{1f}{}\u{1f}{}",
            candidate.kind, candidate.target, candidate.change
        );
        if seen.insert(key) {
            out.push(candidate);
        }
    }
}

fn deterministic_memory_candidate_from_line(
    chapter: u32,
    source: &str,
    heading: &str,
    line: &str,
    source_kind: &str,
) -> Option<MemoryUpdateCandidate> {
    let trimmed = line.trim().trim_start_matches(['-', '*', ' ', '\t']).trim();
    if trimmed.chars().count() < 10 || !is_actionable_audit_line(trimmed) {
        return None;
    }
    if trimmed.eq_ignore_ascii_case("none") || trimmed.contains("CANDIDATE_MEMORY_UPDATES") {
        return None;
    }
    if is_non_durable_memory_line(heading, trimmed) {
        return None;
    }
    if !is_durable_memory_line(heading, trimmed) {
        return None;
    }
    let kind = deterministic_memory_kind(heading, trimmed).to_string();
    let target = deterministic_memory_target(trimmed);
    let change = limit_chars(trimmed, 180);
    normalize_memory_candidate(MemoryUpdateCandidate {
        chapter: Some(chapter),
        kind,
        target: target.clone(),
        change,
        evidence: format!(
            "{source}: deterministic {source_kind} fallback for chapter {chapter:03}"
        ),
        confidence: 0.68,
        affects: deterministic_memory_affects(&target),
        status: Some("candidate".to_string()),
        applied_at: None,
    })
}

fn is_non_durable_memory_line(heading: &str, line: &str) -> bool {
    let heading_has = |needle: &str| heading.contains(needle);
    heading_has("写法反馈")
        || heading_has("CRAFT")
        || line.contains("AI 腔")
        || line.contains("AI腔")
        || line.contains("解释过量")
        || line.contains("写法")
        || line.contains("策略有效")
}

fn is_durable_memory_line(heading: &str, line: &str) -> bool {
    let durable_heading = [
        "章节摘要",
        "人物状态",
        "状态变化",
        "新增事实",
        "事实锁",
        "事件时间线",
        "时间线",
        "伏笔台账",
        "伏笔",
        "人物体验",
        "知识边界",
        "资源",
    ]
    .iter()
    .any(|needle| heading.contains(needle));
    durable_heading
        || [
            "发现", "确认", "知道", "不知", "获得", "失去", "持有", "决定", "选择", "付出", "代价",
            "受伤", "中毒", "关系", "伏笔", "承诺", "资源", "灵石", "账本", "线索", "秘密", "位置",
            "归属",
        ]
        .iter()
        .any(|needle| line.contains(needle))
}

fn deterministic_memory_kind(heading: &str, line: &str) -> &'static str {
    if heading.contains("伏笔") || line.contains("伏笔") || line.contains("承诺") {
        "promise"
    } else if heading.contains("时间线")
        || line.contains("事件")
        || line.contains("发生")
        || line.contains("决定")
        || line.contains("选择")
    {
        "event"
    } else if line.contains("资源")
        || line.contains("灵石")
        || line.contains("账本")
        || line.contains("物件")
        || line.contains("归属")
        || line.contains("持有")
        || line.contains("获得")
        || line.contains("失去")
    {
        "object_state"
    } else if heading.contains("知识") || line.contains("知道") || line.contains("确认") {
        "knowledge"
    } else if line.contains("关系") {
        "relationship"
    } else {
        "memory"
    }
}

fn deterministic_memory_target(line: &str) -> String {
    let marker_target = infer_candidate_target(line);
    if marker_target != "chapter memory" && marker_target.chars().count() <= 24 {
        return clean_memory_target(&marker_target);
    }
    for marker in ["林墨", "账本", "灵石", "资源", "伏笔", "线索"] {
        if line.contains(marker) {
            return marker.to_string();
        }
    }
    let target = line
        .split(['：', ':', '，', ',', '。', '；', ';'])
        .next()
        .unwrap_or(line)
        .split(['的', '在', '从', '已', '将'])
        .next()
        .unwrap_or(line)
        .trim();
    if target.chars().count() >= 2 && target.chars().count() <= 16 {
        clean_memory_target(target)
    } else {
        "chapter memory".to_string()
    }
}

fn clean_memory_target(raw: &str) -> String {
    let cleaned = raw
        .trim()
        .trim_matches(['*', '`', '"', '\'', '“', '”', '‘', '’', '：', ':', '-', ' '])
        .trim()
        .to_string();
    if cleaned.is_empty() {
        "chapter memory".to_string()
    } else {
        cleaned
    }
}

fn deterministic_memory_affects(target: &str) -> Vec<String> {
    if target == "chapter memory" {
        return Vec::new();
    }
    let slug = target
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '-')
        .collect::<String>();
    if slug.is_empty() {
        vec![format!("memory:{target}")]
    } else {
        vec![format!("memory:{slug}")]
    }
}

fn candidate_update_lines(text: &str) -> Vec<&str> {
    let mut saw_section = false;
    let mut in_section = false;
    let mut lines = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            if markdown_heading_matches(
                trimmed.trim_start_matches('#').trim(),
                "CANDIDATE_MEMORY_UPDATES",
            ) {
                saw_section = true;
                in_section = true;
                continue;
            }
            if in_section {
                break;
            }
        }
        if in_section {
            lines.push(line);
        }
    }
    if saw_section { lines } else { Vec::new() }
}

fn has_candidate_memory_section(text: &str) -> bool {
    text.lines()
        .map(str::trim)
        .filter(|line| line.starts_with('#'))
        .any(|line| {
            markdown_heading_matches(
                line.trim_start_matches('#').trim(),
                "CANDIDATE_MEMORY_UPDATES",
            )
        })
}

fn normalize_memory_summary_output(raw: &str) -> String {
    let mut output = raw.trim_end().to_string();
    for heading in REQUIRED_MEMORY_SUMMARY_HEADINGS {
        if summary_has_heading(&output, heading) {
            continue;
        }
        output.push_str("\n\n## ");
        output.push_str(heading);
        output.push('\n');
        if *heading == "CANDIDATE_MEMORY_UPDATES" {
            output.push_str("- none\n");
        } else {
            output.push_str("- missing: generation omitted this required section; review before relying on this summary.\n");
        }
    }
    output
}

fn summary_has_heading(text: &str, token: &str) -> bool {
    text.lines()
        .map(str::trim)
        .filter(|line| line.starts_with('#'))
        .any(|line| markdown_heading_matches(line.trim_start_matches('#').trim(), token))
}

fn markdown_heading_matches(heading: &str, token: &str) -> bool {
    let heading = heading.trim();
    let Some(prefix) = heading.get(..token.len()) else {
        return false;
    };
    if !prefix.eq_ignore_ascii_case(token) {
        return false;
    }
    let rest = heading[token.len()..].trim_start();
    rest.is_empty()
        || rest.chars().next().is_some_and(|ch| {
            matches!(
                ch,
                '(' | '（' | '[' | '【' | ':' | '：' | '-' | '—' | '–' | '/' | '\\'
            )
        })
}

fn collect_audit_risk_lines(
    text: &str,
    chapter: u32,
    preview_limit: usize,
    summary: &mut NovelAuditRiskSummary,
) {
    let mut section: Option<&str> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            let heading = trimmed.trim_start_matches('#').trim();
            section = if heading.eq_ignore_ascii_case("BLOCKER") {
                Some("BLOCKER")
            } else if heading.eq_ignore_ascii_case("MAJOR") {
                Some("MAJOR")
            } else {
                None
            };
            continue;
        }
        let Some(kind) = section else {
            continue;
        };
        if !is_actionable_audit_line(trimmed) {
            continue;
        }
        if kind == "BLOCKER" {
            summary.blockers += 1;
        } else {
            summary.majors += 1;
        }
        if summary.risk_previews.len() < preview_limit {
            summary.risk_previews.push(format!(
                "chapter {chapter:03} {kind}: {}",
                limit_chars(trimmed.trim_start_matches(['-', '*', ' ', '\t']), 120)
            ));
        }
    }
}

fn collect_layered_audit_risks(
    text: &str,
    chapter: u32,
    preview_limit: usize,
    summary: &mut NovelAuditRiskSummary,
) {
    let mut section: Option<AuditTargetSection> = None;
    let mut counts = summary
        .layered_counts
        .iter()
        .cloned()
        .collect::<BTreeMap<_, _>>();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            let parsed = audit_target_section(trimmed.trim_start_matches('#').trim());
            section = if parsed.is_layered() {
                Some(parsed)
            } else {
                None
            };
            continue;
        }
        let Some(section) = section else {
            continue;
        };
        if !is_actionable_audit_line(trimmed) {
            continue;
        }
        let label = section.as_label().to_string();
        *counts.entry(label.clone()).or_default() += 1;
        if summary.layered_previews.len() < preview_limit {
            summary.layered_previews.push(format!(
                "chapter {chapter:03} {label}: {}",
                limit_chars(trimmed.trim_start_matches(['-', '*', ' ', '\t']), 120)
            ));
        }
    }
    summary.layered_counts = counts.into_iter().collect();
}

fn collect_audit_affected_nodes(
    text: &str,
    chapter: u32,
    preview_limit: usize,
    summary: &mut NovelAuditRiskSummary,
) {
    let mut in_section = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            let heading = trimmed.trim_start_matches('#').trim();
            if heading.eq_ignore_ascii_case("AFFECTED_NODES") {
                in_section = true;
                continue;
            }
            if in_section {
                break;
            }
        }
        if !in_section || !is_actionable_audit_line(trimmed) {
            continue;
        }
        for item in affected_node_items(trimmed) {
            summary.affected_nodes += 1;
            if summary.affected_previews.len() < preview_limit {
                summary
                    .affected_previews
                    .push(format!("chapter {chapter:03}: {}", limit_chars(&item, 120)));
            }
        }
    }
}

fn affected_node_items(line: &str) -> Vec<String> {
    let trimmed = line.trim().trim_start_matches(['-', '*', ' ', '\t']).trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if let Some(items) = affected_node_items_from_json(trimmed) {
        return items;
    }
    trimmed
        .split([',', '，', '、'])
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

fn affected_node_items_from_json(raw: &str) -> Option<Vec<String>> {
    let json = if raw.starts_with('{') || raw.starts_with('[') {
        raw
    } else if let Some(start) = raw.find(['{', '[']) {
        &raw[start..]
    } else {
        return None;
    };
    let value = serde_json::from_str::<serde_json::Value>(json).ok()?;
    let mut out = Vec::new();
    collect_affected_json_strings(&value, &mut out);
    if out.is_empty() { None } else { Some(out) }
}

fn collect_affected_json_strings(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(value) => {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                out.push(trimmed.to_string());
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_affected_json_strings(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                if matches!(
                    key.as_str(),
                    "chapter" | "confidence" | "evidence" | "reason"
                ) {
                    continue;
                }
                collect_affected_json_strings(value, out);
            }
        }
        _ => {}
    }
}

fn is_actionable_audit_line(line: &str) -> bool {
    let trimmed = line.trim().trim_start_matches(['-', '*', ' ', '\t']);
    if trimmed.is_empty() {
        return false;
    }
    if is_no_problem_audit_line(trimmed) {
        return false;
    }
    true
}

fn read_structured_memory_candidates(root: &Path) -> Result<Vec<MemoryUpdateCandidate>> {
    let dir = root.join("memory/candidates");
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for path in collect_files_with_extensions(&dir, &["json"])? {
        let raw = read_required(&path)?;
        let value: serde_json::Value = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        let fallback_chapter = value
            .get("chapter")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok());
        let Some(items) = value
            .get("candidates")
            .and_then(serde_json::Value::as_array)
        else {
            continue;
        };
        for item in items {
            let parsed = serde_json::from_value::<MemoryUpdateCandidate>(item.clone())
                .ok()
                .or_else(|| memory_candidate_from_json(item, fallback_chapter));
            let Some(mut candidate) = parsed else {
                continue;
            };
            if candidate.chapter.is_none() {
                candidate.chapter = fallback_chapter;
            }
            let Some(candidate) = normalize_memory_candidate(candidate) else {
                continue;
            };
            out.push(candidate);
        }
    }
    Ok(out)
}

pub(crate) fn import_analysis_report_from_workspace(
    workspace: &Path,
    report: &Path,
) -> Result<String> {
    let root = find_project_root(workspace)?;
    import_analysis_report_from_path(&root, report)
}

fn import_analysis_report_from_path(root: &Path, report: &Path) -> Result<String> {
    let report_path = resolve_project_path(root, report);
    let raw = read_required(&report_path)?;
    let imported = import_analysis_report(root, &report_path, &raw)?;
    Ok(format_import_analysis_report(root, &imported))
}

struct ImportedAnalysisReport {
    report_path: PathBuf,
    candidate_path: PathBuf,
    candidates: usize,
    graph_path: PathBuf,
}

fn import_analysis_report(
    root: &Path,
    report_path: &Path,
    raw: &str,
) -> Result<ImportedAnalysisReport> {
    let now = chrono::Utc::now();
    let stem = report_path
        .file_stem()
        .and_then(OsStr::to_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("analysis");
    let report_id = format!(
        "{}-{}",
        now.format("%Y%m%dT%H%M%SZ"),
        sanitize_file_name(stem)
    );
    let reports_dir = root.join("memory/reports");
    std::fs::create_dir_all(&reports_dir)
        .with_context(|| format!("failed to create {}", reports_dir.display()))?;
    let stored_report_path = reports_dir.join(format!("{report_id}.md"));
    let source_rel = display_relative(root, report_path);
    let report_body = format!(
        "# Imported RLM Manuscript Analysis\n\n- source: `{}`\n- imported_at: `{}`\n- role: continuity diagnosis and reviewable memory extraction\n\n{}\n",
        source_rel,
        now.to_rfc3339(),
        raw.trim()
    );
    write_text(&stored_report_path, &report_body, true)?;

    let candidates = extract_analysis_candidates(raw, &display_relative(root, &stored_report_path));
    let candidate_path = root
        .join("memory/candidates")
        .join(format!("analysis-{report_id}.json"));
    let body = serde_json::to_string_pretty(&serde_json::json!({
        "schema_version": 1,
        "source": display_relative(root, &stored_report_path),
        "status": "pending_review",
        "kind": "rlm_analysis_import",
        "candidates": candidates,
    }))
    .context("failed to encode imported analysis candidates")?;
    write_text(&candidate_path, &body, true)?;

    let graph = build_memory_graph(root)?;
    save_memory_graph(root, &graph)?;

    Ok(ImportedAnalysisReport {
        report_path: stored_report_path,
        candidate_path,
        candidates: candidates.len(),
        graph_path: memory_graph_path(root),
    })
}

fn format_import_analysis_report(root: &Path, imported: &ImportedAnalysisReport) -> String {
    format!(
        "# Imported Manuscript Analysis\n\n- report: `{}`\n- candidate file: `{}`\n- candidate updates: {}\n- graph rebuilt: `{}`\n\nReview with `deepseek memory candidates`, then confirm with `deepseek memory apply`.",
        display_relative(root, &imported.report_path),
        display_relative(root, &imported.candidate_path),
        imported.candidates,
        display_relative(root, &imported.graph_path)
    )
}

fn novel_eval_command(workspace: &Path, command: NovelEvalCommand) -> Result<()> {
    let root = find_project_root(workspace)?;
    match command {
        NovelEvalCommand::CollectFailure {
            kind,
            chapter,
            source,
            expected_signal,
            expected_revision,
            note,
        } => {
            let report = collect_failure_fixture(
                &root,
                kind,
                chapter,
                &source,
                &expected_signal,
                &expected_revision,
                note.as_deref(),
            )?;
            println!("{report}");
            Ok(())
        }
        NovelEvalCommand::CollectNonTrigger {
            signal,
            source,
            note,
        } => {
            let report = collect_non_trigger_fixture(&root, &signal, &source, note.as_deref())?;
            println!("{report}");
            Ok(())
        }
        NovelEvalCommand::Coverage => {
            println!("{}", eval_fixture_coverage_report(&root)?);
            Ok(())
        }
    }
}

fn novel_experiment_command(workspace: &Path, command: NovelExperimentCommand) -> Result<()> {
    let root = find_project_root(workspace)?;
    match command {
        NovelExperimentCommand::Plan {
            name,
            chapters,
            workflow,
            model,
            temperature,
            skill,
            force,
        } => {
            let report = write_experiment_plan(
                &root,
                &name,
                chapters,
                &workflow,
                model.as_deref(),
                temperature,
                skill.as_deref(),
                force,
            )?;
            println!("{report}");
            Ok(())
        }
        NovelExperimentCommand::Snapshot {
            start,
            end,
            run_id,
            force,
        } => {
            let report = write_experiment_snapshot(&root, start, end, run_id.as_deref(), force)?;
            println!("{report}");
            Ok(())
        }
    }
}

fn collect_failure_fixture(
    root: &Path,
    kind: FailureKind,
    chapter: Option<u32>,
    source: &Path,
    expected_signal: &str,
    expected_revision: &str,
    note: Option<&str>,
) -> Result<String> {
    if expected_signal.trim().is_empty() {
        bail!("--expected-signal must not be empty");
    }
    if expected_revision.trim().is_empty() {
        bail!("--expected-revision must not be empty");
    }
    let source_path = resolve_project_path(root, source);
    let text = read_required(&source_path)?;
    let now = chrono::Utc::now();
    let source_stem = source_path
        .file_stem()
        .and_then(OsStr::to_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("sample");
    let chapter_part = chapter
        .map(|value| format!("ch{value:03}-"))
        .unwrap_or_default();
    let fixture_id = format!(
        "{}{}-{}",
        chapter_part,
        now.format("%Y%m%dT%H%M%SZ"),
        sanitize_file_name(source_stem)
    );
    let fixture_dir = root.join("eval/failures").join(kind.as_str());
    std::fs::create_dir_all(&fixture_dir)
        .with_context(|| format!("failed to create {}", fixture_dir.display()))?;
    let fixture_path = fixture_dir.join(format!("{fixture_id}.json"));
    let record = serde_json::json!({
        "schema_version": 1,
        "fixture_id": fixture_id,
        "kind": kind.as_str(),
        "chapter": chapter,
        "source": display_relative(root, &source_path),
        "collected_at": now.to_rfc3339(),
        "expected_signal": expected_signal.trim(),
        "expected_revision": expected_revision.trim(),
        "note": note.unwrap_or("").trim(),
        "text": text,
        "validation_boundary": "Regression fixture only; does not prove real reader quality or million-word stability."
    });
    write_text(
        &fixture_path,
        &serde_json::to_string_pretty(&record).context("failed to encode failure fixture")?,
        true,
    )?;
    Ok(format!(
        "# Failure Fixture Collected\n\n- kind: `{}`\n- fixture: `{}`\n- source: `{}`\n- expected_signal: `{}`\n- boundary: regression fixture only; true validation remains after all development.",
        kind.as_str(),
        display_relative(root, &fixture_path),
        display_relative(root, &source_path),
        expected_signal.trim()
    ))
}

fn collect_non_trigger_fixture(
    root: &Path,
    signal: &str,
    source: &Path,
    note: Option<&str>,
) -> Result<String> {
    if signal.trim().is_empty() {
        bail!("--signal must not be empty");
    }
    let source_path = resolve_project_path(root, source);
    let text = read_required(&source_path)?;
    let now = chrono::Utc::now();
    let source_stem = source_path
        .file_stem()
        .and_then(OsStr::to_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("sample");
    let fixture_id = format!(
        "{}-{}",
        now.format("%Y%m%dT%H%M%SZ"),
        sanitize_file_name(source_stem)
    );
    let fixture_dir = root.join("eval/fixtures/non_trigger");
    std::fs::create_dir_all(&fixture_dir)
        .with_context(|| format!("failed to create {}", fixture_dir.display()))?;
    let fixture_path = fixture_dir.join(format!("{fixture_id}.json"));
    let record = serde_json::json!({
        "schema_version": 1,
        "fixture_id": fixture_id,
        "kind": "non_trigger",
        "signal": signal.trim(),
        "source": display_relative(root, &source_path),
        "collected_at": now.to_rfc3339(),
        "note": note.unwrap_or("").trim(),
        "text": text,
        "validation_boundary": "Negative-control regression fixture only; does not prove real reader quality."
    });
    write_text(
        &fixture_path,
        &serde_json::to_string_pretty(&record).context("failed to encode non-trigger fixture")?,
        true,
    )?;
    Ok(format!(
        "# Non-Trigger Fixture Collected\n\n- signal: `{}`\n- fixture: `{}`\n- source: `{}`\n- boundary: regression fixture only.",
        signal.trim(),
        display_relative(root, &fixture_path),
        display_relative(root, &source_path)
    ))
}

fn eval_fixture_coverage_report(root: &Path) -> Result<String> {
    let mut positives = BTreeMap::<String, usize>::new();
    let failures_dir = root.join("eval/failures");
    for path in collect_files_recursive_with_extensions(&failures_dir, &["json"], 3, 500)? {
        let raw = read_required(&path)?;
        let value = serde_json::from_str::<serde_json::Value>(&raw)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        if let Some(signal) = value
            .get("expected_signal")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            *positives.entry(signal.trim().to_string()).or_default() += 1;
        }
    }

    let mut negatives = BTreeMap::<String, usize>::new();
    let non_trigger_dir = root.join("eval/fixtures/non_trigger");
    for path in collect_files_with_extensions(&non_trigger_dir, &["json"])? {
        let raw = read_required(&path)?;
        let value = serde_json::from_str::<serde_json::Value>(&raw)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        if let Some(signal) = value
            .get("signal")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            *negatives.entry(signal.trim().to_string()).or_default() += 1;
        }
    }

    let mut signals = positives
        .keys()
        .chain(negatives.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for signal in [
        "xianxia_resource_anchor",
        "xianxia_combat_knowledge_loop",
        "xianxia_dialogue_voice",
        "xianxia_worldbuilding_action",
        "xianxia_emotion_texture",
        "anchor_carry",
    ] {
        signals.insert(signal.to_string());
    }

    let mut out = String::new();
    out.push_str("# Eval Fixture Coverage\n\n");
    out.push_str(
        "- boundary: regression fixture coverage only; not a writing capability result.\n",
    );
    out.push_str("- signals:\n");
    for signal in signals {
        let positive = positives.get(&signal).copied().unwrap_or(0);
        let negative = negatives.get(&signal).copied().unwrap_or(0);
        let status = if positive > 0 && negative > 0 {
            "covered"
        } else if positive > 0 {
            "missing_non_trigger"
        } else if negative > 0 {
            "missing_failure"
        } else {
            "missing_both"
        };
        let _ = writeln!(
            out,
            "  - {signal}: failures {positive}, non_triggers {negative}, status {status}"
        );
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn write_experiment_plan(
    root: &Path,
    name: &str,
    chapters: u32,
    workflow: &str,
    model: Option<&str>,
    temperature: Option<f32>,
    skill: Option<&str>,
    force: bool,
) -> Result<String> {
    if name.trim().is_empty() {
        bail!("--name must not be empty");
    }
    if chapters == 0 {
        bail!("--chapters must be greater than 0");
    }
    let manifest = load_manifest(root)?;
    let now = chrono::Utc::now();
    let run_id = format!(
        "{}-{}",
        now.format("%Y%m%dT%H%M%SZ"),
        sanitize_file_name(name)
    );
    let config_path = root
        .join("experiments/configs")
        .join(format!("{run_id}.json"));
    if config_path.exists() && !force {
        bail!(
            "{} already exists. Pass --force to overwrite it.",
            config_path.display()
        );
    }
    let command_chain = experiment_command_chain(chapters, workflow);
    let record = serde_json::json!({
        "schema_version": 1,
        "run_id": run_id,
        "name": name.trim(),
        "created_at": now.to_rfc3339(),
        "book": {
            "title": manifest.title,
            "genre": manifest.genre,
            "target_words": manifest.target_words,
            "current_chapter": manifest.current_chapter
        },
        "planned_chapters": chapters,
        "workflow": workflow.trim(),
        "model": model.unwrap_or("default"),
        "temperature": temperature,
        "skill": skill,
        "memory": {
            "graph": "memory/graph.json",
            "archive_policy": "manual before true long-run validation",
            "candidate_review": "required before durable ledger writes"
        },
        "command_chain": command_chain,
        "expected_artifacts": [
            "ContextQualityReport",
            "ChapterQualityReport",
            "chapters/{NNN}/audit.md",
            "memory/candidates/{NNN}.json",
            "chapter revision diff",
            "memory regression report",
            "experiments/reports/{snapshot}.json"
        ],
        "acceptance_baseline": {
            "path": "experiments/baselines/long_form_acceptance.md",
            "snapshot_command": "deepseek experiment snapshot --start 1 --end N --run-id <run_id>",
            "minimum_sample_chapters": [10, 30, 50],
            "default_words_per_chapter": 3500,
            "required_gates": [
                "no context_quality blockers before drafting unless explicitly waived",
                "every drafted chapter has ChapterQualityReport",
                "every completed chapter has audit.md and memory summary",
                "pending memory candidates are reviewed or justified before the next 10-chapter window",
                "memory regression reports exist for each 10-chapter window"
            ],
            "tracked_metrics": [
                "chapter_success_rate",
                "average_generation_latency_seconds",
                "context_chars_per_chapter",
                "blocker_count_by_code",
                "major_count_by_code",
                "pending_candidate_count",
                "summary_max_chars",
                "canon_sparse_summary_count",
                "revision_voice_loss_count",
                "chapter_bridge_opening_count"
            ]
        },
        "collapse_signals": [
            "character_drift",
            "promise_breakage",
            "resource_without_cost",
            "knowledge_leak",
            "revision_voice_loss",
            "context_growth"
        ],
        "validation_boundary": "Plan only. Do not treat this config as a real writing capability result."
    });
    write_text(
        &config_path,
        &serde_json::to_string_pretty(&record).context("failed to encode experiment config")?,
        true,
    )?;
    Ok(format!(
        "# Experiment Plan Written\n\n- config: `{}`\n- run_id: `{}`\n- planned_chapters: {}\n- workflow: `{}`\n- boundary: scaffold only; no chapters were generated and no capability claim was made.",
        display_relative(root, &config_path),
        run_id,
        chapters,
        workflow.trim()
    ))
}

fn experiment_command_chain(chapters: u32, workflow: &str) -> Vec<String> {
    let mut chain = Vec::new();
    chain.push("deepseek memory build".to_string());
    for chapter in 1..=chapters.min(100) {
        chain.push(format!("deepseek brief {chapter}"));
        chain.push(format!("deepseek write {chapter}"));
        chain.push(format!("deepseek audit {chapter}"));
        if workflow.contains("targeted") || workflow.contains("revise") {
            chain.push(format!("deepseek revise {chapter}"));
        }
        chain.push(format!("deepseek remember {chapter}"));
        if chapter % 10 == 0 {
            chain.push("deepseek memory regression 10 --write".to_string());
        }
        if chapter % 20 == 0 {
            chain.push("deepseek memory promises".to_string());
        }
    }
    if workflow.contains("archive") && chapters >= 25 {
        chain.push("deepseek memory archive 1 25 --label stage-001-025".to_string());
    }
    chain
}

fn write_experiment_snapshot(
    root: &Path,
    start: u32,
    end: Option<u32>,
    run_id: Option<&str>,
    force: bool,
) -> Result<String> {
    if start == 0 {
        bail!("--start must be greater than 0");
    }
    let manifest = load_manifest(root)?;
    let discovered_end = collect_chapter_dirs(root)?
        .into_iter()
        .map(|(chapter, _)| chapter)
        .max()
        .unwrap_or(manifest.current_chapter)
        .max(start);
    let end = end.unwrap_or(discovered_end);
    if end < start {
        bail!("--end must be greater than or equal to --start");
    }
    let now = chrono::Utc::now();
    let snapshot_id = format!(
        "{}-{}-{:03}-{:03}",
        now.format("%Y%m%dT%H%M%SZ"),
        sanitize_file_name(run_id.unwrap_or("snapshot")),
        start,
        end
    );
    let report_path = root
        .join("experiments/reports")
        .join(format!("{snapshot_id}.json"));
    if report_path.exists() && !force {
        bail!(
            "{} already exists. Pass --force to overwrite it.",
            report_path.display()
        );
    }

    let mut chapter_reports = Vec::new();
    let mut collapse_counts = BTreeMap::<String, usize>::new();
    for chapter in start..=end {
        let dir = chapter_dir(root, chapter);
        let has_draft = dir.join("draft.md").is_file();
        let has_final = dir.join("final.md").is_file();
        let has_audit = dir.join("audit.md").is_file();
        let has_candidates = root
            .join("memory/candidates")
            .join(format!("{chapter:03}.json"))
            .is_file();
        let context_report = context_quality_report(root, chapter)
            .unwrap_or_else(|err| format!("# ContextQualityReport\n\n- error: {}", err));
        let chapter_text = read_optional_limited(&dir.join("final.md"), CONTEXT_LIMIT)
            .or_else(|| read_optional_limited(&dir.join("draft.md"), CONTEXT_LIMIT))
            .unwrap_or_default();
        let quality_report = if chapter_text.trim().is_empty() {
            "# ChapterQualityReport\n\n- error: missing chapter text\n".to_string()
        } else {
            chapter_quality_report(root, chapter, &chapter_text)
                .unwrap_or_else(|err| format!("# ChapterQualityReport\n\n- error: {}", err))
        };
        let context_gaps = extract_report_items(&context_report, "- gaps:");
        let quality_issues = extract_report_items(&quality_report, "- detected_issues:");
        let mut collapse_candidates = collapse_candidates_from_reports(
            &context_gaps,
            &quality_issues,
            has_audit,
            has_candidates,
        );
        collapse_candidates.sort();
        collapse_candidates.dedup();
        for item in &collapse_candidates {
            *collapse_counts.entry(item.clone()).or_default() += 1;
        }
        chapter_reports.push(serde_json::json!({
            "chapter": chapter,
            "has_draft": has_draft,
            "has_final": has_final,
            "has_audit": has_audit,
            "has_memory_candidates": has_candidates,
            "context_gaps": context_gaps,
            "quality_issues": quality_issues,
            "collapse_candidates": collapse_candidates,
            "context_quality_report": context_report,
            "chapter_quality_report": quality_report
        }));
    }

    let record = serde_json::json!({
        "schema_version": 1,
        "snapshot_id": snapshot_id,
        "run_id": run_id.unwrap_or("unassigned"),
        "created_at": now.to_rfc3339(),
        "chapter_range": { "start": start, "end": end },
        "book": {
            "title": manifest.title,
            "genre": manifest.genre,
            "target_words": manifest.target_words
        },
        "chapters": chapter_reports,
        "collapse_counts": collapse_counts,
        "validation_boundary": "Deterministic snapshot of existing artifacts only; no chapters generated and no capability claim made."
    });
    write_text(
        &report_path,
        &serde_json::to_string_pretty(&record).context("failed to encode experiment snapshot")?,
        true,
    )?;
    Ok(format!(
        "# Experiment Snapshot Written\n\n- report: `{}`\n- chapters: {:03}-{:03}\n- boundary: existing artifacts only; no real long-run validation was executed.",
        display_relative(root, &report_path),
        start,
        end
    ))
}

fn extract_report_items(report: &str, marker: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut in_section = false;
    for line in report.lines() {
        let trimmed = line.trim();
        if trimmed == marker {
            in_section = true;
            continue;
        }
        if in_section {
            if let Some(item) = trimmed.strip_prefix("- ") {
                if item != "none" {
                    items.push(item.to_string());
                }
            } else if !trimmed.is_empty() {
                break;
            }
        }
    }
    items
}

fn collapse_candidates_from_reports(
    context_gaps: &[String],
    quality_issues: &[String],
    has_audit: bool,
    has_candidates: bool,
) -> Vec<String> {
    let mut out = Vec::new();
    for item in context_gaps.iter().chain(quality_issues.iter()) {
        if item.contains("knowledge") || item.contains("知识") {
            out.push("knowledge_leak".to_string());
        }
        if item.contains("promise") || item.contains("伏笔") || item.contains("承诺") {
            out.push("promise_breakage".to_string());
        }
        if item.contains("resource") || item.contains("资源") {
            out.push("resource_without_cost".to_string());
        }
        if item.contains("dialogue") || item.contains("voice") || item.contains("声口") {
            out.push("character_drift".to_string());
        }
        if item.contains("anchor_carry") {
            out.push("anchor_carry_loss".to_string());
        }
    }
    if !has_audit {
        out.push("missing_audit".to_string());
    }
    if !has_candidates {
        out.push("missing_memory_candidates".to_string());
    }
    out
}

#[derive(Debug, Clone)]
struct MemoryReportSummary {
    report_path: PathBuf,
    source: Option<String>,
    imported_at: Option<String>,
    candidate_file: Option<PathBuf>,
    candidates: usize,
    pending: usize,
    applied: usize,
}

pub(crate) fn memory_reports_packet_from_workspace(workspace: &Path) -> Result<String> {
    let root = find_project_root(workspace)?;
    memory_reports_packet(&root)
}

fn memory_reports_packet(root: &Path) -> Result<String> {
    let reports = collect_memory_report_summaries(root)?;
    let mut out = String::new();
    out.push_str("# Imported Manuscript Analysis Reports\n\n");
    if reports.is_empty() {
        out.push_str("- none\n");
        return Ok(out);
    }
    let total_candidates = reports
        .iter()
        .map(|report| report.candidates)
        .sum::<usize>();
    let pending = reports.iter().map(|report| report.pending).sum::<usize>();
    let applied = reports.iter().map(|report| report.applied).sum::<usize>();
    let _ = writeln!(out, "- reports: {}", reports.len());
    let _ = writeln!(out, "- candidates: {total_candidates}");
    let _ = writeln!(out, "- pending: {pending}");
    let _ = writeln!(out, "- applied: {applied}\n");
    for report in reports {
        let _ = writeln!(out, "## `{}`", display_relative(root, &report.report_path));
        if let Some(source) = &report.source {
            let _ = writeln!(out, "- source: `{source}`");
        }
        if let Some(imported_at) = &report.imported_at {
            let _ = writeln!(out, "- imported_at: `{imported_at}`");
        }
        if let Some(candidate_file) = &report.candidate_file {
            let _ = writeln!(
                out,
                "- candidate_file: `{}`",
                display_relative(root, candidate_file)
            );
        } else {
            out.push_str("- candidate_file: missing\n");
        }
        let _ = writeln!(
            out,
            "- candidates: {} | pending: {} | applied: {}\n",
            report.candidates, report.pending, report.applied
        );
    }
    Ok(out)
}

fn collect_memory_report_summaries(root: &Path) -> Result<Vec<MemoryReportSummary>> {
    let reports = collect_files_with_extensions(&root.join("memory/reports"), &["md"])?;
    let candidate_files =
        collect_files_with_extensions(&root.join("memory/candidates"), &["json"])?;
    let mut summaries = Vec::new();
    for report_path in reports {
        let text = read_required(&report_path)?;
        let report_rel = display_relative(root, &report_path);
        let candidate_file = candidate_files.iter().find_map(|path| {
            let raw = read_required(path).ok()?;
            let value = serde_json::from_str::<serde_json::Value>(&raw).ok()?;
            let source = value.get("source").and_then(serde_json::Value::as_str)?;
            (source == report_rel).then(|| path.clone())
        });
        let (candidates, pending, applied) = candidate_file
            .as_ref()
            .map(|path| candidate_file_counts(path))
            .transpose()?
            .unwrap_or((0, 0, 0));
        summaries.push(MemoryReportSummary {
            report_path,
            source: front_matter_value(&text, "source"),
            imported_at: front_matter_value(&text, "imported_at"),
            candidate_file,
            candidates,
            pending,
            applied,
        });
    }
    summaries.sort_by(|a, b| b.report_path.cmp(&a.report_path));
    Ok(summaries)
}

fn candidate_file_counts(path: &Path) -> Result<(usize, usize, usize)> {
    let raw = read_required(path)?;
    let value: serde_json::Value = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let Some(items) = value
        .get("candidates")
        .and_then(serde_json::Value::as_array)
    else {
        return Ok((0, 0, 0));
    };
    let mut pending = 0;
    let mut applied = 0;
    for item in items {
        let status = item
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("candidate");
        if status.eq_ignore_ascii_case("applied") {
            applied += 1;
        } else {
            pending += 1;
        }
    }
    Ok((items.len(), pending, applied))
}

fn front_matter_value(text: &str, key: &str) -> Option<String> {
    let prefix = format!("- {key}:");
    text.lines().find_map(|line| {
        let value = line.trim().strip_prefix(&prefix)?;
        let value = value.trim().trim_matches('`').trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

fn memory_regression_report(root: &Path, window: u32, write: bool) -> Result<String> {
    let window = window.clamp(1, 100);
    let graph = load_or_build_memory_graph(root)?;
    let chapters = collect_chapter_dirs(root)?;
    if chapters.is_empty() {
        bail!(
            "no chapters found under {}",
            root.join("chapters").display()
        );
    }
    let chapter_numbers = chapters
        .iter()
        .map(|(chapter, _)| *chapter)
        .collect::<Vec<_>>();
    let mut out = String::new();
    out.push_str("# Continuity Regression\n\n");
    let _ = writeln!(out, "- window: {window}");
    let _ = writeln!(out, "- chapters: {}", chapter_numbers.len());
    let _ = writeln!(
        out,
        "- graph_schema: {}",
        memory_schema_validation_status(&graph)
    );
    let _ = writeln!(
        out,
        "- promise_status_counts: {}",
        format_promise_status_counts(&promise_status_counts(&graph))
    );
    let _ = writeln!(
        out,
        "- candidate_updates: {}\n",
        graph.candidate_updates.len()
    );
    out.push_str("This deterministic report checks coverage and continuity surfaces before deeper RLM review.\n\n");

    let mut start_index = 0usize;
    while start_index < chapter_numbers.len() {
        let end_index = (start_index + window as usize).min(chapter_numbers.len());
        let slice = &chapter_numbers[start_index..end_index];
        let start = slice.first().copied().unwrap_or_default();
        let end = slice.last().copied().unwrap_or_default();
        let _ = writeln!(out, "## Chapters {start:03}-{end:03}\n");
        append_regression_window(&mut out, root, &graph, slice)?;
        start_index = end_index;
    }

    out.push_str("## Recommended Cadence\n\n");
    out.push_str(
        "- Every 10 chapters: inspect missing summaries, candidates, and new promise states.\n",
    );
    out.push_str(
        "- Every 20 chapters: run `/memory promises` and resolve stale open/progress items.\n",
    );
    out.push_str("- Every 50 chapters: run `/analyze 3 continuity regression` for RLM-backed character, timeline, promise, and knowledge-boundary review.\n");

    if write {
        let path = root.join("memory/reports").join(format!(
            "regression-{}-w{window}.md",
            chrono::Utc::now().format("%Y%m%d%H%M%S")
        ));
        write_text(&path, &out, true)?;
        let mut saved = String::new();
        let _ = writeln!(
            saved,
            "# Continuity Regression Saved\n\n- report: `{}`\n",
            display_relative(root, &path)
        );
        saved.push_str(&out);
        Ok(saved)
    } else {
        Ok(out)
    }
}

fn append_regression_window(
    out: &mut String,
    root: &Path,
    graph: &MemoryGraph,
    chapters: &[u32],
) -> Result<()> {
    let chapter_set = chapters.iter().copied().collect::<BTreeSet<_>>();
    let missing_summaries = chapters
        .iter()
        .copied()
        .filter(|chapter| {
            !root
                .join("memory/summaries")
                .join(format!("{chapter:03}.md"))
                .is_file()
        })
        .collect::<Vec<_>>();
    let missing_audits = chapters
        .iter()
        .copied()
        .filter(|chapter| !chapter_dir(root, *chapter).join("audit.md").is_file())
        .collect::<Vec<_>>();
    let candidates = graph
        .candidate_updates
        .iter()
        .filter(|candidate| {
            candidate
                .chapter
                .is_some_and(|chapter| chapter_set.contains(&chapter))
        })
        .collect::<Vec<_>>();
    let active_promises = graph
        .nodes
        .iter()
        .filter(|node| node.kind == "promise" && !node.id.starts_with("asset:"))
        .filter(|node| {
            state_chapter(node, &["chapter", "first_chapter"])
                .is_some_and(|chapter| chapter_set.contains(&chapter))
        })
        .filter(|node| {
            let status = state_string(node, &["status"])
                .unwrap_or_else(|| "unknown".to_string())
                .to_ascii_lowercase();
            !matches!(
                status.as_str(),
                "payoff" | "paid" | "paid_off" | "resolved" | "abandoned" | "dropped"
            )
        })
        .collect::<Vec<_>>();
    let knowledge_nodes = graph
        .nodes
        .iter()
        .filter(|node| node.kind == "knowledge")
        .filter(|node| {
            state_chapter(node, &["chapter", "first_chapter"])
                .is_some_and(|chapter| chapter_set.contains(&chapter))
        })
        .count();
    let relationship_changes = graph
        .nodes
        .iter()
        .filter(|node| node.kind == "relationship")
        .filter(|node| {
            state_chapter(node, &["chapter", "first_chapter"])
                .is_some_and(|chapter| chapter_set.contains(&chapter))
        })
        .count();
    let event_nodes = graph
        .nodes
        .iter()
        .filter(|node| node.kind == "event")
        .filter(|node| {
            state_chapter(node, &["chapter", "first_chapter"])
                .is_some_and(|chapter| chapter_set.contains(&chapter))
        })
        .count();
    let anchor_candidates = regression_anchor_candidates(graph, &chapter_set);
    let carry = score_anchor_carry_for_chapters(root, chapters, &anchor_candidates)?;

    let _ = writeln!(
        out,
        "- coverage: summaries missing {}, audits missing {}, pending candidates {}",
        missing_summaries.len(),
        missing_audits.len(),
        candidates.len()
    );
    let _ = writeln!(
        out,
        "- surfaces: active_promises {}, knowledge {}, relationships {}, events {}",
        active_promises.len(),
        knowledge_nodes,
        relationship_changes,
        event_nodes
    );
    let _ = writeln!(
        out,
        "- anchor_carry: anchors {}, mentioned {}, carried {}, rate {:.2}",
        carry.anchor_count, carry.mentioned_count, carry.carried_count, carry.carry_rate
    );

    if !missing_summaries.is_empty() {
        let _ = writeln!(
            out,
            "- missing summaries: {}",
            format_chapter_list(&missing_summaries)
        );
    }
    if !missing_audits.is_empty() {
        let _ = writeln!(
            out,
            "- missing audits: {}",
            format_chapter_list(&missing_audits)
        );
    }
    if !active_promises.is_empty() {
        out.push_str("- active promises:\n");
        for node in active_promises.into_iter().take(8) {
            let status = state_string(node, &["status"]).unwrap_or_else(|| "unknown".to_string());
            let _ = writeln!(
                out,
                "  - {} [{}] {}",
                node.label,
                promise_status_label(&status),
                limit_chars(&node.summary, 160)
            );
        }
    }
    if !candidates.is_empty() {
        out.push_str("- pending candidates:\n");
        for candidate in candidates.into_iter().take(8) {
            let _ = writeln!(
                out,
                "  - chapter {} [{}] {} -> {}",
                candidate
                    .chapter
                    .map(|chapter| format!("{chapter:03}"))
                    .unwrap_or_else(|| "?".to_string()),
                candidate.kind,
                candidate.target,
                limit_chars(&candidate.change, 140)
            );
        }
    }
    if !carry.weak_items.is_empty() {
        out.push_str("- weak anchors:\n");
        for item in carry.weak_items.iter().take(8) {
            let mode = if item.modes.is_empty() {
                "mentioned_only".to_string()
            } else {
                item.modes.join("+")
            };
            let terms = if item.terms.is_empty() {
                String::new()
            } else {
                format!(" ({})", item.terms.join(", "))
            };
            let _ = writeln!(out, "  - {}: {mode}{terms}", item.anchor);
        }
    }
    out.push_str("- gates: knowledge-boundary scan, promise carry/payoff scan, event timeline order, relationship/state deltas\n\n");
    Ok(())
}

#[derive(Debug, Clone, Default)]
struct AnchorCarryWindowReport {
    anchor_count: usize,
    mentioned_count: usize,
    carried_count: usize,
    carry_rate: f32,
    weak_items: Vec<AnchorCarryWindowItem>,
}

#[derive(Debug, Clone)]
struct AnchorCarryWindowItem {
    anchor: String,
    modes: Vec<String>,
    terms: Vec<String>,
}

fn regression_anchor_candidates(graph: &MemoryGraph, chapters: &BTreeSet<u32>) -> Vec<String> {
    let mut anchors = graph
        .nodes
        .iter()
        .filter(|node| {
            matches!(
                node.kind.as_str(),
                "character" | "relationship" | "promise" | "event" | "knowledge" | "secret"
            ) && !node.id.starts_with("asset:")
        })
        .filter(|node| {
            state_chapter(node, &["chapter", "first_chapter"])
                .is_none_or(|chapter| chapters.contains(&chapter))
        })
        .map(|node| node.label.trim().to_string())
        .filter(|label| !label.is_empty())
        .collect::<Vec<_>>();
    anchors.sort();
    anchors.dedup();
    anchors.truncate(32);
    anchors
}

fn score_anchor_carry_for_chapters(
    root: &Path,
    chapters: &[u32],
    anchors: &[String],
) -> Result<AnchorCarryWindowReport> {
    if anchors.is_empty() {
        return Ok(AnchorCarryWindowReport::default());
    }
    let mut text = String::new();
    for chapter in chapters {
        let dir = chapter_dir(root, *chapter);
        if let Some(chapter_text) = read_optional_limited(&dir.join("final.md"), 8_000)
            .or_else(|| read_optional_limited(&dir.join("draft.md"), 8_000))
        {
            text.push_str(&chapter_text);
            text.push('\n');
        }
    }
    let mut mentioned = 0usize;
    let mut carried = 0usize;
    let mut weak_items = Vec::new();
    let sentences = split_anchor_sentences(&text);
    for anchor in anchors {
        let item = score_anchor_carry_item(anchor, &sentences);
        if item.mentioned {
            mentioned += 1;
        }
        if item.carried {
            carried += 1;
        }
        if item.mentioned && !item.carried {
            weak_items.push(AnchorCarryWindowItem {
                anchor: anchor.clone(),
                modes: Vec::new(),
                terms: Vec::new(),
            });
        }
    }
    weak_items.sort_by(|left, right| left.anchor.cmp(&right.anchor));
    weak_items.truncate(12);
    let anchor_count = anchors.len();
    Ok(AnchorCarryWindowReport {
        anchor_count,
        mentioned_count: mentioned,
        carried_count: carried,
        carry_rate: if anchor_count == 0 {
            0.0
        } else {
            carried as f32 / anchor_count as f32
        },
        weak_items,
    })
}

#[derive(Debug, Clone, Default)]
struct AnchorCarryItemScore {
    mentioned: bool,
    carried: bool,
    modes: Vec<String>,
    terms: Vec<String>,
}

fn score_anchor_carry_item(anchor: &str, sentences: &[String]) -> AnchorCarryItemScore {
    let mut score = AnchorCarryItemScore::default();
    for sentence in sentences
        .iter()
        .filter(|sentence| sentence.contains(anchor))
    {
        score.mentioned = true;
        collect_anchor_mode(sentence, "action", ANCHOR_ACTION_TERMS, &mut score);
        collect_anchor_mode(sentence, "dialogue", ANCHOR_DIALOGUE_TERMS, &mut score);
        collect_anchor_mode(
            sentence,
            "consequence",
            ANCHOR_CONSEQUENCE_TERMS,
            &mut score,
        );
        collect_anchor_mode(sentence, "payoff_pressure", ANCHOR_PAYOFF_TERMS, &mut score);
    }
    score.modes.sort();
    score.modes.dedup();
    score.terms.sort();
    score.terms.dedup();
    score.carried = !score.modes.is_empty();
    score
}

fn collect_anchor_mode(
    sentence: &str,
    mode: &str,
    terms: &[&str],
    score: &mut AnchorCarryItemScore,
) {
    let matched = terms
        .iter()
        .copied()
        .filter(|term| sentence.contains(term))
        .take(3)
        .collect::<Vec<_>>();
    if matched.is_empty() {
        return;
    }
    score.modes.push(mode.to_string());
    score.terms.extend(matched.into_iter().map(str::to_string));
}

fn split_anchor_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        current.push(ch);
        if matches!(ch, '。' | '！' | '？' | '!' | '?' | '；' | ';' | '\n') {
            push_anchor_sentence(&mut sentences, &mut current);
        }
    }
    push_anchor_sentence(&mut sentences, &mut current);
    sentences
}

fn push_anchor_sentence(sentences: &mut Vec<String>, current: &mut String) {
    let sentence = current.trim();
    if !sentence.is_empty() {
        sentences.push(sentence.to_string());
    }
    current.clear();
}

const ANCHOR_ACTION_TERMS: &[&str] = &[
    "拔", "握", "递", "交", "救", "追", "挡", "打开", "藏", "拿", "看", "盯", "亮出", "压", "斩",
    "换", "抢", "护", "逼问", "承认", "选择", "翻", "读", "逼", "追问",
];
const ANCHOR_DIALOGUE_TERMS: &[&str] = &[
    "\"", "“", "”", "说", "问", "喊", "答", "承认", "道", "低声", "开口", "接话",
];
const ANCHOR_CONSEQUENCE_TERMS: &[&str] = &[
    "因此",
    "于是",
    "导致",
    "逼得",
    "只好",
    "选择",
    "决定",
    "代价",
    "后果",
    "失去",
    "换来",
    "发现",
    "意识到",
    "确认",
    "暴露",
    "牵出",
    "重新",
    "不敢",
    "被迫",
];
const ANCHOR_PAYOFF_TERMS: &[&str] = &[
    "要还", "还债", "偿还", "清算", "兑现", "伏笔", "真相", "账册", "代价", "承诺", "线索", "缺页",
    "入口", "交易", "选择", "信任", "背叛", "秘密", "谜底",
];
const SCENE_GOAL_TERMS: &[&str] = &[
    "要", "想", "必须", "得", "不能", "不敢", "避开", "躲", "藏", "隐瞒", "确认", "查", "找",
    "拿到", "守住", "救", "逃", "拦", "换", "还", "问清", "试探",
];
const SCENE_CHANGE_TERMS: &[&str] = &[
    "改口", "停住", "停下", "让开", "退", "退后", "转身", "闭嘴", "沉默", "点头", "摇头", "交出",
    "收回", "放下", "藏起", "拿走", "答应", "拒绝", "承认", "怀疑", "相信", "知道", "明白", "暴露",
    "改变", "决定", "选择", "逼", "威胁", "欠",
];
const CHAPTER_BRIDGE_KEY_TERMS: &[&str] = &[
    "账册",
    "账本",
    "残页",
    "古经",
    "草环",
    "水袋",
    "干饼",
    "外套",
    "匕首",
    "碎片",
    "锁",
    "锁扣",
    "牢门",
    "禁厄牢",
    "祭坛",
    "山神庙",
    "镇厄司",
    "照命坊",
    "月关祭",
    "占灾台",
    "追兵",
    "信号",
    "血",
    "伤",
    "右手",
    "左手",
    "膝",
    "丹田",
    "命线",
    "灾劫",
    "痴厄",
    "九儿",
    "苏棠",
    "薛蛮",
    "江鹤",
    "沈落霞",
    "霍连城",
    "韩玉郎",
    "林墨",
    "逃",
    "追",
    "藏",
    "等",
    "问",
    "答应",
    "拒绝",
    "承诺",
    "真相",
    "秘密",
    "代价",
    "后果",
];
const CHAPTER_BRIDGE_TRANSITION_TERMS: &[&str] = &[
    "那之后",
    "第二日",
    "次日",
    "天亮",
    "天没亮",
    "一夜",
    "半夜",
    "当晚",
    "回到",
    "离开",
    "醒来",
    "再睁眼",
    "刚才",
    "昨夜",
    "昨日",
    "三日后",
    "后天",
    "仍",
    "还",
    "没",
    "未",
    "继续",
    "接着",
    "身上",
    "手里",
    "怀里",
    "口袋",
    "袖",
    "鞋底",
];

const RESOURCE_CARD_TEMPLATE: &str = r#"id: resource_id
name: 资源名
category: currency|pill|artifact|manual|territory|favor
rarity: 常见/稀有/宗门管制/禁物
market_value: 公开价格或坊市估值
ordinary_income_equivalent: 折合普通修士多久收入
who_controls_it: 控制方/所有者/发放方
cost_to_use: 使用代价、损耗、反噬或规则限制
debt_or_obligation: 获得后欠下的人情、债务、门规或阵营义务
first_seen: chapters/000
last_changed: chapters/000
canon_status: draft|canon|retired
evidence: 证据路径或章节句柄
"#;

const XIANXIA_GENRE_TERMS: &[&str] = &[
    "玄幻", "仙侠", "修仙", "修真", "修炼", "宗门", "灵气", "法宝", "丹药", "飞剑",
];
const XIANXIA_REALM_RULE_TERMS: &[&str] = &[
    "境界", "修为", "炼气", "筑基", "金丹", "元婴", "化神", "渡劫", "瓶颈", "破境", "灵根", "功法",
    "心法", "禁制",
];
const XIANXIA_FACTION_TERMS: &[&str] = &[
    "宗门", "师门", "门规", "长老", "掌门", "内门", "外门", "家族", "世家", "王朝", "盟约", "供奉",
    "香火", "山门",
];
const XIANXIA_ARTIFACT_TERMS: &[&str] = &[
    "法宝", "法器", "灵器", "飞剑", "阵盘", "符箓", "丹炉", "剑丸", "灵舟", "禁制", "阵法", "剑诀",
    "术法",
];
const XIANXIA_ABSTRACT_EMOTION_TERMS: &[&str] = &[
    "感到",
    "觉得",
    "无比",
    "极其",
    "十分",
    "非常",
    "震惊",
    "愤怒",
    "悲伤",
    "痛苦",
    "恐惧",
    "绝望",
    "激动",
    "复杂",
    "难以言说",
];
const XIANXIA_CONCRETE_REACTION_TERMS: &[&str] = &[
    "指节", "掌心", "喉咙", "牙", "血", "汗", "袖", "衣角", "膝", "肩", "背", "腕", "咽", "攥",
    "捏", "抖", "退", "停", "低头", "闭眼", "吐息", "咳",
];
const XIANXIA_RESOURCE_TERMS: &[&str] = &[
    "灵石", "丹药", "灵丹", "妖丹", "符箓", "法宝", "灵器", "功法", "秘籍", "灵脉", "洞府", "药田",
    "矿脉", "供奉", "俸禄", "赏赐", "资源",
];
const XIANXIA_RESOURCE_ANCHOR_TERMS: &[&str] = &[
    "值", "价格", "市价", "折合", "一年", "三年", "十年", "收入", "俸禄", "供奉", "代价", "换来",
    "抵", "欠", "债", "账", "买", "卖", "租", "押",
];
const XIANXIA_RESOURCE_OBLIGATION_TERMS: &[&str] = &[
    "欠",
    "债",
    "账",
    "人情",
    "义务",
    "归还",
    "偿还",
    "抵押",
    "控制",
    "掌控",
    "归属",
    "所有者",
    "宗门义务",
    "师门债",
    "门规处罚",
    "反噬",
    "损耗",
];
const XIANXIA_CONTEXT_ECONOMY_TERMS: &[&str] = &[
    "价格", "市价", "收入", "俸禄", "供奉", "灵石", "钱", "货币", "坊市", "买", "卖", "债", "账",
];
const XIANXIA_COMBAT_TERMS: &[&str] = &[
    "剑", "刀", "拳", "掌", "斩", "刺", "劈", "轰", "杀", "阵", "符", "术", "神通", "飞剑", "灵力",
    "剑气", "拳罡", "血",
];
const XIANXIA_COMBAT_OBSERVATION_TERMS: &[&str] = &[
    "看出",
    "听见",
    "察觉",
    "发现",
    "盯住",
    "破绽",
    "气息",
    "脚步",
    "手腕",
    "剑路",
    "阵眼",
    "灵力流向",
    "呼吸",
    "血气",
];
const XIANXIA_COMBAT_REVERSAL_TERMS: &[&str] = &[
    "反手",
    "反杀",
    "趁",
    "借势",
    "避开",
    "换招",
    "破阵",
    "破开",
    "忽然",
    "下一刻",
    "原来",
    "不是",
    "早已",
    "只等",
];
const XIANXIA_WORLDBUILDING_TERMS: &[&str] = &[
    "境界", "宗门", "门规", "灵石", "灵脉", "洞府", "坊市", "山门", "禁地", "阵法", "法宝", "丹药",
    "功法", "师承", "香火", "因果", "天劫",
];

const STYLE_ZERO_TOLERANCE_TERMS: &[&str] = &[
    "不禁",
    "不由得",
    "下意识地",
    "情不自禁地",
    "莫名地",
    "忍不住",
    "一股",
    "一抹",
    "一丝",
    "一缕",
    "前所未有的",
    "难以言喻的",
    "莫名的",
    "深吸一口气",
    "若有所思",
    "意味深长",
    "心头一跳",
    "浑身一震",
    "嘴角微微上扬",
];

const STYLE_AI_SUMMARY_TERMS: &[&str] = &[
    "这一刻他终于明白了",
    "这一刻她终于明白了",
    "命运的齿轮开始转动",
    "故事才刚刚开始",
    "这不仅仅是",
    "这不只是",
    "真正重要的是",
    "更深层的意义",
    "某种意义上",
    "从某种意义上说",
];

const STYLE_AI_STRUCTURE_TERMS: &[&str] = &[
    "不是因为",
    "而是因为",
    "不仅仅是",
    "不只是",
    "真正的",
    "更重要的是",
    "换句话说",
    "归根结底",
    "某种程度上",
    "值得注意的是",
    "这意味着",
    "这说明",
    "这代表",
    "这象征",
];

const STYLE_RULE_OF_THREE_TERMS: &[&str] = &[
    "恐惧", "愤怒", "痛苦", "震惊", "绝望", "犹豫", "沉默", "冰冷", "锋利", "古老", "沉重", "复杂",
    "真实", "清晰", "明确", "权力", "信息", "关系", "选择", "代价", "后果", "命运", "真相", "秘密",
];

const STYLE_AI_OPENING_ATMOSPHERE_TERMS: &[&str] = &[
    "废墟", "冒烟", "晨风", "山风", "夜色", "暮色", "天色", "清晨", "黄昏", "月光", "雨声", "风声",
    "雾气", "烟尘", "寒意", "寂静", "坊市", "广场", "街", "巷",
];

const STYLE_OPENING_ACTION_TERMS: &[&str] = &[
    "攥", "掂", "递", "推", "按", "藏", "扯", "跪", "拦", "挡", "逃", "跑", "摔", "砸", "割", "咬",
    "吐", "捡", "塞", "换", "卖", "买", "问", "盯", "看着", "低头",
];

const STYLE_OPENING_CHARACTER_TERMS: &[&str] = &[
    "他", "她", "沈", "林", "陈", "秦", "陆", "顾", "师兄", "师妹", "执事", "小吏", "卖家", "弟子",
    "掌柜",
];

const STYLE_BUDGET_TERMS: &[&str] = &[
    "随即",
    "接着",
    "而后",
    "非常",
    "极其",
    "稍稍",
    "略微",
    "颇为",
    "淡淡地说",
    "冷冷说道",
    "轻声说道",
];

const VIEWPOINT_INNER_STATE_PATTERNS: &[&str] = &[
    "他心想",
    "她心想",
    "他心里",
    "她心里",
    "他意识到",
    "她意识到",
    "他明白",
    "她明白",
    "他知道",
    "她知道",
    "他觉得",
    "她觉得",
];

fn format_chapter_list(chapters: &[u32]) -> String {
    chapters
        .iter()
        .take(20)
        .map(|chapter| format!("{chapter:03}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn extract_analysis_candidates(raw: &str, source: &str) -> Vec<MemoryUpdateCandidate> {
    let mut candidates = Vec::new();
    collect_candidates_from_json_values(raw, &mut candidates, false);
    for line in candidate_update_lines(raw) {
        if let Some(candidate) = memory_candidate_from_line(0, source, line) {
            candidates.push(candidate);
        }
    }
    for candidate in &mut candidates {
        if candidate.chapter == Some(0) {
            candidate.chapter = None;
        }
        if candidate.evidence.trim().is_empty() {
            candidate.evidence = source.to_string();
        }
        candidate.kind = canonical_memory_kind(&candidate.kind);
        candidate.status = Some("candidate".to_string());
    }
    dedupe_memory_candidates(&mut candidates);
    candidates.truncate(200);
    candidates
}

fn collect_candidates_from_json_values(
    raw: &str,
    out: &mut Vec<MemoryUpdateCandidate>,
    require_container: bool,
) {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) {
        collect_candidates_from_json_value(&value, out, require_container);
    }
    for line in raw.lines() {
        let trimmed = line.trim().trim_start_matches(['-', '*', ' ', '\t']);
        let json = if trimmed.starts_with('{') {
            Some(trimmed)
        } else {
            trimmed.find('{').map(|start| &trimmed[start..])
        };
        let Some(json) = json else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
            continue;
        };
        collect_candidates_from_json_value(&value, out, true);
    }
}

fn collect_candidates_from_json_value(
    value: &serde_json::Value,
    out: &mut Vec<MemoryUpdateCandidate>,
    require_container: bool,
) {
    if let Some(items) = value
        .get("candidate_memory_updates")
        .or_else(|| value.get("candidates"))
        .and_then(serde_json::Value::as_array)
    {
        let fallback_chapter = value
            .get("chapter")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok());
        for item in items {
            if let Some(candidate) = memory_candidate_from_json(item, fallback_chapter) {
                out.push(candidate);
            }
        }
        return;
    }
    if require_container {
        return;
    }
    if (value.get("change").is_some() || value.get("summary").is_some())
        && let Some(candidate) = memory_candidate_from_json(value, None)
    {
        out.push(candidate);
    }
}

fn dedupe_memory_candidates(candidates: &mut Vec<MemoryUpdateCandidate>) {
    let mut seen = BTreeSet::new();
    candidates.retain(|candidate| {
        seen.insert(format!(
            "{}\n{}\n{}\n{:?}",
            candidate.kind, candidate.target, candidate.change, candidate.chapter
        ))
    });
}

pub(crate) fn cite_material_from_workspace(
    workspace: &Path,
    source: &Path,
    chapter: Option<u32>,
    card: Option<&Path>,
) -> Result<String> {
    let root = find_project_root(workspace)?;
    cite_material_from_path(&root, source, chapter, card)
}

fn cite_material_from_path(
    root: &Path,
    source: &Path,
    chapter: Option<u32>,
    card: Option<&Path>,
) -> Result<String> {
    if chapter.is_none() && card.is_none() {
        bail!("choose a destination with --chapter N or --card cards/.../name.md");
    }
    if chapter.is_some() && card.is_some() {
        bail!("choose only one destination: --chapter N or --card PATH");
    }

    let source_path = resolve_project_path(root, source);
    validate_material_source(root, &source_path)?;
    let raw = read_required(&source_path)?;
    let citation = build_material_citation(root, &source_path, &raw);

    if let Some(chapter) = chapter {
        let dir = chapter_dir(root, chapter);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
        let brief_path = dir.join("brief.md");
        append_section(&brief_path, &citation)?;
        return Ok(format!(
            "# Material Citation Added\n\n- source: `{}`\n- target: `{}`\n\nThe material remains reference-only. Promote durable facts through cards, bible, or memory candidates before treating them as canon.",
            display_relative(root, &source_path),
            display_relative(root, &brief_path)
        ));
    }

    let card = card.expect("checked card destination");
    let card_path = resolve_project_path(root, card);
    validate_material_card_target(root, &card_path)?;
    append_section(&card_path, &citation)?;
    Ok(format!(
        "# Material Citation Added\n\n- source: `{}`\n- target: `{}`\n\nReview the card draft before treating any sourced detail as canon.",
        display_relative(root, &source_path),
        display_relative(root, &card_path)
    ))
}

fn validate_material_source(root: &Path, source: &Path) -> Result<()> {
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let canonical_source = source
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", source.display()))?;
    let materials_root = canonical_root.join("materials");
    if !canonical_source.starts_with(&materials_root) {
        bail!(
            "material source must be under {}",
            root.join("materials").display()
        );
    }
    if !canonical_source.is_file() {
        bail!("material source is not a file: {}", source.display());
    }
    Ok(())
}

fn validate_material_card_target(root: &Path, target: &Path) -> Result<()> {
    let rel = target.strip_prefix(root).unwrap_or(target);
    let starts_allowed = ["cards", "bible", "outline"]
        .iter()
        .any(|dir| rel.starts_with(dir));
    if !starts_allowed {
        bail!("card target must be under cards/, bible/, or outline/");
    }
    let extension = target.extension().and_then(OsStr::to_str).unwrap_or("");
    if !matches!(extension, "md" | "yaml" | "yml") {
        bail!("card target must be .md, .yaml, or .yml");
    }
    Ok(())
}

fn build_material_citation(root: &Path, source: &Path, raw: &str) -> String {
    let rel = display_relative(root, source);
    let digest = stable_hash(raw);
    let excerpt = material_excerpt(raw, 900);
    format!(
        "## Reference Material: {rel}\n\n- source: `{rel}`\n- imported_at: `{}`\n- source_hash: `{digest}`\n- canon_status: reference_only\n- rule: This material does not override book.toml, bible, cards, outline, chapters, or memory ledgers.\n\n### Usable Notes\n\n{}\n\n### Promotion Checklist\n\n- [ ] If this changes canon, update the relevant bible/card/memory candidate with source notes.\n- [ ] If this only supports atmosphere or procedure, keep it as reference and do not create durable facts.\n",
        chrono::Utc::now().to_rfc3339(),
        excerpt
    )
}

fn material_excerpt(raw: &str, limit: usize) -> String {
    let mut lines = raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !line.to_ascii_lowercase().starts_with("ignore previous"))
        .take(20)
        .collect::<Vec<_>>()
        .join("\n");
    if lines.is_empty() {
        lines = "(empty material file)".to_string();
    }
    limit_chars(&lines, limit)
}

fn append_section(path: &Path, section: &str) -> Result<()> {
    let mut body = String::new();
    if path.is_file() {
        body = read_required(path)?;
        if !body.ends_with('\n') {
            body.push('\n');
        }
        body.push('\n');
    } else if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    body.push_str(section.trim());
    body.push('\n');
    std::fs::write(path, body).with_context(|| format!("failed to write {}", path.display()))
}

#[derive(Debug, Clone)]
struct MaterialReference {
    target_path: PathBuf,
    source: String,
    source_hash: Option<String>,
    canon_status: Option<String>,
    health: MaterialReferenceHealth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MaterialReferenceHealth {
    Ok,
    StaleSource,
    MissingSource,
    Unchecked,
}

impl MaterialReferenceHealth {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::StaleSource => "stale_source",
            Self::MissingSource => "missing_source",
            Self::Unchecked => "unchecked",
        }
    }
}

pub(crate) fn material_references_packet_from_workspace(workspace: &Path) -> Result<String> {
    let root = find_project_root(workspace)?;
    material_references_packet(&root)
}

fn material_references_packet(root: &Path) -> Result<String> {
    let references = collect_material_references(root)?;
    let mut out = String::new();
    out.push_str("# Material References\n\n");
    out.push_str(
        "Local materials are reference-only unless promoted into bible/cards/memory candidates with source notes.\n\n",
    );
    if references.is_empty() {
        out.push_str("- none\n");
        return Ok(out);
    }

    let mut by_source: BTreeMap<String, Vec<&MaterialReference>> = BTreeMap::new();
    for reference in &references {
        by_source
            .entry(reference.source.clone())
            .or_default()
            .push(reference);
    }
    out.push_str(&format!("- citations: {}\n", references.len()));
    out.push_str(&format!("- unique sources: {}\n\n", by_source.len()));
    let mut health_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for reference in &references {
        *health_counts.entry(reference.health.as_str()).or_insert(0) += 1;
    }
    if !health_counts.is_empty() {
        out.push_str("- health:");
        for (health, count) in health_counts {
            let _ = write!(out, " {health}={count}");
        }
        out.push_str("\n\n");
    }
    for (source, items) in by_source {
        let _ = writeln!(out, "## `{source}`\n");
        for item in items {
            let status = item.canon_status.as_deref().unwrap_or("reference_only");
            let hash = item.source_hash.as_deref().unwrap_or("missing");
            let _ = writeln!(
                out,
                "- target: `{}` | status: {status} | source_health: {} | hash: {hash}",
                display_relative(root, &item.target_path),
                item.health.as_str()
            );
        }
        out.push('\n');
    }
    Ok(out)
}

fn collect_material_references(root: &Path) -> Result<Vec<MaterialReference>> {
    let mut targets = Vec::new();
    for dir in ["chapters", "cards", "bible", "outline"] {
        targets.extend(collect_files_recursive_with_extensions(
            &root.join(dir),
            &["md", "yaml", "yml"],
            4,
            512,
        )?);
    }
    targets.sort();

    let mut references = Vec::new();
    for path in targets {
        let Some(text) = read_optional_limited(&path, 200_000) else {
            continue;
        };
        references.extend(
            material_references_from_text(&path, &text)
                .into_iter()
                .map(|reference| material_reference_with_health(root, reference)),
        );
    }
    references.sort_by(|a, b| {
        (
            a.source.as_str(),
            a.health,
            display_relative(root, &a.target_path),
            a.source_hash.as_deref().unwrap_or(""),
        )
            .cmp(&(
                b.source.as_str(),
                b.health,
                display_relative(root, &b.target_path),
                b.source_hash.as_deref().unwrap_or(""),
            ))
    });
    Ok(references)
}

fn material_references_from_text(path: &Path, text: &str) -> Vec<MaterialReference> {
    let mut references = Vec::new();
    let mut in_reference = false;
    let mut source: Option<String> = None;
    let mut source_hash: Option<String> = None;
    let mut canon_status: Option<String> = None;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("## Reference Material:") {
            if let Some(source_value) = source.take() {
                references.push(MaterialReference {
                    target_path: path.to_path_buf(),
                    source: source_value,
                    source_hash: source_hash.take(),
                    canon_status: canon_status.take(),
                    health: MaterialReferenceHealth::Unchecked,
                });
            }
            in_reference = true;
            source = Some(
                trimmed
                    .trim_start_matches("## Reference Material:")
                    .trim()
                    .trim_matches('`')
                    .to_string(),
            );
            continue;
        }
        if in_reference && trimmed.starts_with("## ") {
            if let Some(source_value) = source.take() {
                references.push(MaterialReference {
                    target_path: path.to_path_buf(),
                    source: source_value,
                    source_hash: source_hash.take(),
                    canon_status: canon_status.take(),
                    health: MaterialReferenceHealth::Unchecked,
                });
            }
            in_reference = false;
            continue;
        }
        if !in_reference {
            continue;
        }
        if let Some(value) = material_reference_field(trimmed, "source") {
            source = Some(value);
        } else if let Some(value) = material_reference_field(trimmed, "source_hash") {
            source_hash = Some(value);
        } else if let Some(value) = material_reference_field(trimmed, "canon_status") {
            canon_status = Some(value);
        }
    }
    if let Some(source_value) = source {
        references.push(MaterialReference {
            target_path: path.to_path_buf(),
            source: source_value,
            source_hash,
            canon_status,
            health: MaterialReferenceHealth::Unchecked,
        });
    }
    references
}

fn material_reference_with_health(
    root: &Path,
    mut reference: MaterialReference,
) -> MaterialReference {
    let source_path = resolve_project_path(root, Path::new(&reference.source));
    let Some(text) = std::fs::read_to_string(&source_path).ok() else {
        reference.health = MaterialReferenceHealth::MissingSource;
        return reference;
    };
    let Some(expected_hash) = reference.source_hash.as_deref() else {
        reference.health = MaterialReferenceHealth::Unchecked;
        return reference;
    };
    let actual_hash = stable_hash(&text);
    reference.health = if actual_hash == expected_hash {
        MaterialReferenceHealth::Ok
    } else {
        MaterialReferenceHealth::StaleSource
    };
    reference
}

fn material_reference_field(line: &str, key: &str) -> Option<String> {
    let rest = line.strip_prefix("- ")?;
    let (field, value) = rest.split_once(':')?;
    if field.trim() != key {
        return None;
    }
    let value = value.trim().trim_matches('`').trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn resolve_project_path(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

pub(crate) fn memory_candidates_packet(
    workspace: &Path,
    chapter: Option<u32>,
    include_applied: bool,
) -> Result<String> {
    let root = find_project_root(workspace)?;
    let candidates = memory_candidates_for_display(&root, chapter, include_applied)?;
    if candidates.is_empty() {
        Ok("No candidate memory updates found.".to_string())
    } else {
        Ok(format_memory_candidates(&candidates))
    }
}

pub(crate) fn apply_memory_candidates_from_workspace(
    workspace: &Path,
    chapter: Option<u32>,
    dry_run: bool,
) -> Result<String> {
    let root = find_project_root(workspace)?;
    apply_memory_candidates(&root, chapter, dry_run)
}

fn memory_candidates_for_display(
    root: &Path,
    chapter: Option<u32>,
    include_applied: bool,
) -> Result<Vec<MemoryUpdateCandidate>> {
    let mut candidates = collect_memory_update_candidates(root)?;
    candidates.retain(|candidate| {
        chapter.is_none_or(|chapter| candidate.chapter == Some(chapter))
            && (include_applied || !candidate_is_applied(candidate))
    });
    candidates.sort_by_key(|candidate| {
        (
            candidate.chapter.unwrap_or(u32::MAX),
            candidate.kind.clone(),
            candidate.target.clone(),
        )
    });
    Ok(candidates)
}

fn format_memory_candidates(candidates: &[MemoryUpdateCandidate]) -> String {
    let mut out = String::new();
    out.push_str("# Candidate Memory Updates\n\n");
    for (index, candidate) in candidates.iter().enumerate() {
        out.push_str(&format!(
            "{}. [{}] {} -> {}\n",
            index + 1,
            candidate.kind,
            candidate.target,
            candidate.change
        ));
        if let Some(chapter) = candidate.chapter {
            out.push_str(&format!("   chapter: {chapter:03}\n"));
        }
        out.push_str(&format!(
            "   confidence: {:.2} | status: {}\n",
            candidate.confidence,
            candidate.status.as_deref().unwrap_or("candidate")
        ));
        if !candidate.evidence.trim().is_empty() {
            out.push_str(&format!("   evidence: {}\n", candidate.evidence));
        }
        if !candidate.affects.is_empty() {
            out.push_str(&format!("   affects: {}\n", candidate.affects.join(", ")));
        }
    }
    out.push_str("\nUse `deepseek memory apply --chapter N` after reviewing candidates.\n");
    out
}

fn apply_memory_candidates(root: &Path, chapter: Option<u32>, dry_run: bool) -> Result<String> {
    let candidates = memory_candidates_for_display(root, chapter, false)?;
    if candidates.is_empty() {
        return Ok("No pending candidate memory updates to apply.".to_string());
    }

    let mut ledger_lines: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
    let mut behavior_lines = Vec::new();
    for candidate in &candidates {
        let path = root.join(ledger_path_for_candidate(candidate));
        let record = serde_json::to_string(&candidate_ledger_record(candidate))
            .context("failed to encode memory ledger record")?;
        ledger_lines.entry(path).or_default().push(record);
        for behavior in behavior_records_from_candidate(candidate) {
            behavior_lines.push(
                serde_json::to_string(&behavior).context("failed to encode behavior record")?,
            );
        }
    }

    let mut out = String::new();
    out.push_str("# Apply Candidate Memory Updates\n\n");
    out.push_str(&format!(
        "Mode: {}\n",
        if dry_run { "dry-run" } else { "apply" }
    ));
    out.push_str(&format!("Candidates: {}\n\n", candidates.len()));

    for (path, lines) in &ledger_lines {
        out.push_str(&format!(
            "- {}: {} record(s)\n",
            display_relative(root, path),
            lines.len()
        ));
        if !dry_run {
            append_jsonl_records(path, lines)?;
        }
    }
    if !behavior_lines.is_empty() {
        let path = root.join("memory/behavior.jsonl");
        out.push_str(&format!(
            "- {}: {} record(s)\n",
            display_relative(root, &path),
            behavior_lines.len()
        ));
        if !dry_run {
            append_jsonl_records(&path, &behavior_lines)?;
        }
    }

    if !dry_run {
        mark_candidate_files_applied(root, chapter)?;
        let graph = build_memory_graph(root)?;
        save_memory_graph(root, &graph)?;
        out.push_str("\nApplied candidates, marked source files, and rebuilt memory/graph.json.\n");
    } else {
        out.push_str("\nNo files changed.\n");
    }
    Ok(out)
}

fn candidate_is_applied(candidate: &MemoryUpdateCandidate) -> bool {
    candidate
        .status
        .as_deref()
        .is_some_and(|status| status.eq_ignore_ascii_case("applied"))
        || candidate.applied_at.is_some()
}

fn ledger_path_for_candidate(candidate: &MemoryUpdateCandidate) -> &'static str {
    match candidate.kind.as_str() {
        "event" | "timeline" => "memory/events.jsonl",
        "promise" | "foreshadowing" => "memory/foreshadowing.jsonl",
        "relationship" | "character_state" | "location_state" | "object_state" => {
            "memory/facts.jsonl"
        }
        _ => "memory/facts.jsonl",
    }
}

fn candidate_ledger_record(candidate: &MemoryUpdateCandidate) -> serde_json::Value {
    let kind = canonical_memory_kind(&candidate.kind);
    let target_key = if kind.as_str() == "promise" {
        "promise"
    } else if kind.as_str() == "event" {
        "event"
    } else {
        "target"
    };
    let mut record = serde_json::json!({
        "kind": kind,
        "chapter": candidate.chapter,
        "target": candidate.target,
        "change": candidate.change,
        "evidence": candidate.evidence,
        "confidence": candidate.confidence,
        "affects": candidate.affects,
        "source": "memory_candidate",
        "applied_at": chrono::Utc::now().to_rfc3339(),
    });
    if let Some(object) = record.as_object_mut() {
        object.insert(
            target_key.to_string(),
            serde_json::Value::String(candidate.target.clone()),
        );
        if kind.as_str() == "promise" {
            object.entry("status".to_string()).or_insert_with(|| {
                serde_json::Value::String(infer_promise_status(&candidate.change).to_string())
            });
            if let Some(chapter) = candidate.chapter {
                object
                    .entry("first_chapter".to_string())
                    .or_insert_with(|| serde_json::json!(chapter));
            }
            object
                .entry("progress".to_string())
                .or_insert_with(|| serde_json::Value::String(candidate.change.clone()));
        }
    }
    record
}

fn canonical_memory_kind(kind: &str) -> String {
    match kind.trim().to_ascii_lowercase().as_str() {
        "foreshadowing" | "foreshadow" | "伏笔" => "promise".to_string(),
        "timeline" | "时间线" => "event".to_string(),
        "character_state" | "location_state" | "object_state" | "knowledge" | "relationship"
        | "promise" | "event" | "memory" => kind.trim().to_ascii_lowercase(),
        _ => kind.trim().to_string(),
    }
}

fn infer_promise_status(change: &str) -> &'static str {
    let lower = change.to_lowercase();
    if change.contains("回收")
        || change.contains("兑现")
        || lower.contains("payoff")
        || lower.contains("paid")
        || lower.contains("resolved")
    {
        "payoff"
    } else if change.contains("悬置") || change.contains("暂停") || lower.contains("suspend") {
        "suspended"
    } else if change.contains("废弃") || change.contains("放弃") || lower.contains("abandon") {
        "abandoned"
    } else if change.contains("推进") || change.contains("加深") || lower.contains("progress") {
        "progress"
    } else {
        "new"
    }
}

fn append_jsonl_records(path: &Path, lines: &[String]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let mut body = String::new();
    if path.is_file() {
        body = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        if !body.is_empty() && !body.ends_with('\n') {
            body.push('\n');
        }
    }
    for line in lines {
        body.push_str(line);
        body.push('\n');
    }
    std::fs::write(path, body).with_context(|| format!("failed to write {}", path.display()))
}

fn mark_candidate_files_applied(root: &Path, chapter: Option<u32>) -> Result<()> {
    let dir = root.join("memory/candidates");
    if !dir.is_dir() {
        return Ok(());
    }
    let now = chrono::Utc::now().to_rfc3339();
    for path in collect_files_with_extensions(&dir, &["json"])? {
        let raw = read_required(&path)?;
        let mut value: serde_json::Value = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        let file_chapter = value
            .get("chapter")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok());
        if chapter.is_some() && file_chapter != chapter {
            continue;
        }
        let Some(items) = value
            .get_mut("candidates")
            .and_then(serde_json::Value::as_array_mut)
        else {
            continue;
        };
        for item in items {
            let status = item
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("candidate");
            if status.eq_ignore_ascii_case("applied") {
                continue;
            }
            if let Some(obj) = item.as_object_mut() {
                obj.insert(
                    "status".to_string(),
                    serde_json::Value::String("applied".to_string()),
                );
                obj.insert(
                    "applied_at".to_string(),
                    serde_json::Value::String(now.clone()),
                );
            }
        }
        write_text(
            &path,
            &serde_json::to_string_pretty(&value).context("failed to encode candidate file")?,
            true,
        )?;
    }
    Ok(())
}

fn memory_candidate_from_json(
    value: &serde_json::Value,
    fallback_chapter: Option<u32>,
) -> Option<MemoryUpdateCandidate> {
    let kind = value
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("memory")
        .to_string();
    let target = value
        .get("target")
        .or_else(|| value.get("promise"))
        .or_else(|| value.get("event"))
        .or_else(|| value.get("label"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)?;
    let change = value
        .get("change")
        .or_else(|| value.get("summary"))
        .or_else(|| value.get("progress"))
        .or_else(|| value.get("payoff"))
        .or_else(|| value.get("fact"))
        .and_then(serde_json::Value::as_str)?
        .to_string();
    let evidence = value
        .get("evidence")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    let confidence = value
        .get("confidence")
        .and_then(serde_json::Value::as_f64)
        .map(|value| value.clamp(0.0, 1.0) as f32)
        .unwrap_or(0.7);
    let affects = value
        .get("affects")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    normalize_memory_candidate(MemoryUpdateCandidate {
        chapter: value
            .get("chapter")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .or(fallback_chapter),
        kind,
        target,
        change,
        evidence,
        confidence,
        affects,
        status: value
            .get("status")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .or_else(|| Some("candidate".to_string())),
        applied_at: value
            .get("applied_at")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
    })
}

fn memory_candidate_from_line(
    chapter: u32,
    source: &str,
    line: &str,
) -> Option<MemoryUpdateCandidate> {
    let trimmed = line.trim().trim_start_matches(['-', '*', ' ', '\t']);
    if trimmed.starts_with('#') || trimmed.eq_ignore_ascii_case("none") {
        return None;
    }
    if trimmed.chars().count() < 8 {
        return None;
    }
    if let Some(candidate) = memory_candidate_from_json_line(chapter, trimmed) {
        return Some(candidate);
    }
    if !looks_like_explicit_text_candidate(trimmed) {
        return None;
    }
    let target = candidate_text_field(trimmed, &["target:", "目标：", "人物：", "对象："])?;
    let change = candidate_text_field(trimmed, &["change:", "变化：", "变更："])?;
    let evidence = candidate_text_field(trimmed, &["evidence:", "证据："])
        .unwrap_or_else(|| format!("{source}: chapter {chapter:03}"));

    normalize_memory_candidate(MemoryUpdateCandidate {
        chapter: Some(chapter),
        kind: infer_candidate_kind(trimmed).to_string(),
        target,
        change,
        evidence,
        confidence: infer_confidence(trimmed),
        affects: infer_affects(trimmed),
        status: Some("candidate".to_string()),
        applied_at: None,
    })
}

fn memory_candidate_from_json_line(chapter: u32, line: &str) -> Option<MemoryUpdateCandidate> {
    let json = if line.starts_with('{') {
        line
    } else if let Some(start) = line.find('{') {
        &line[start..]
    } else {
        return None;
    };
    let value = serde_json::from_str::<serde_json::Value>(json).ok()?;
    memory_candidate_from_json(&value, Some(chapter)).map(|mut candidate| {
        if candidate.chapter.is_none() {
            candidate.chapter = Some(chapter);
        }
        candidate
    })
}

fn normalize_memory_candidate(
    mut candidate: MemoryUpdateCandidate,
) -> Option<MemoryUpdateCandidate> {
    candidate.kind = canonical_memory_kind(&candidate.kind);
    candidate.target = clean_memory_target(&candidate.target);
    candidate.change = candidate.change.trim().to_string();
    candidate.evidence = candidate.evidence.trim().to_string();
    candidate.confidence = candidate.confidence.clamp(0.0, 1.0);
    candidate
        .affects
        .retain(|value| is_valid_candidate_affect(value));
    candidate.affects.sort();
    candidate.affects.dedup();
    if candidate.status.is_none() {
        candidate.status = Some("candidate".to_string());
    }
    validate_memory_candidate_fields(&candidate).then_some(candidate)
}

fn validate_memory_candidate_fields(candidate: &MemoryUpdateCandidate) -> bool {
    is_supported_memory_candidate_kind(&candidate.kind)
        && is_valid_candidate_text_field(&candidate.target, CandidateField::Target)
        && is_valid_candidate_text_field(&candidate.change, CandidateField::Change)
        && candidate.evidence.chars().count() <= 260
}

fn is_supported_memory_candidate_kind(kind: &str) -> bool {
    matches!(
        canonical_memory_kind(kind).as_str(),
        "knowledge"
            | "relationship"
            | "promise"
            | "event"
            | "location_state"
            | "object_state"
            | "character_state"
            | "memory"
    )
}

#[derive(Debug, Clone, Copy)]
enum CandidateField {
    Target,
    Change,
}

fn is_valid_candidate_text_field(value: &str, field: CandidateField) -> bool {
    let trimmed = value.trim();
    let chars = trimmed.chars().count();
    let max = match field {
        CandidateField::Target => 48,
        CandidateField::Change => 220,
    };
    if chars == 0 || chars > max {
        return false;
    }
    if trimmed.starts_with('#') || trimmed.contains('\n') || trimmed.contains('|') {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    let forbidden = [
        "候选记忆更新",
        "candidate_memory_updates",
        "memory_candidate_audit",
        "target:",
        "change:",
        "evidence:",
        "confidence",
        "affects:",
        "注意**",
        "**注意",
        "下列变化值得进入候选记忆",
        "写法反馈",
        "后续风险",
        "audit",
    ];
    if forbidden
        .iter()
        .any(|marker| trimmed.contains(marker) || lower.contains(marker))
    {
        return false;
    }
    if matches!(field, CandidateField::Target)
        && [
            "注意",
            "建议",
            "问题",
            "风险",
            "本章",
            "章节",
            "chapter memory",
        ]
        .contains(&trimmed)
    {
        return false;
    }
    true
}

fn is_valid_candidate_affect(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && trimmed.chars().count() <= 80
        && !trimmed.contains('\n')
        && !trimmed.contains("target:")
        && !trimmed.contains("change:")
}

fn looks_like_explicit_text_candidate(text: &str) -> bool {
    (text.contains("候选")
        || text.contains("待确认")
        || text.contains("应写入")
        || text.contains("需要登记")
        || text.to_lowercase().contains("candidate_memory"))
        && has_any_marker(text, &["target:", "目标：", "人物：", "对象："])
        && has_any_marker(text, &["change:", "变化：", "变更："])
}

fn has_any_marker(text: &str, markers: &[&str]) -> bool {
    markers.iter().any(|marker| text.contains(marker))
}

fn candidate_text_field(text: &str, markers: &[&str]) -> Option<String> {
    let (start, marker_len) = markers
        .iter()
        .filter_map(|marker| text.find(marker).map(|index| (index, marker.len())))
        .min_by_key(|(index, _)| *index)?;
    let rest = &text[start + marker_len..];
    let stop = candidate_field_stop(rest).unwrap_or(rest.len());
    let value = rest[..stop]
        .trim()
        .trim_start_matches([':', '：', '=', ' ', '\t'])
        .trim()
        .trim_matches(['`', '"', '\'', '“', '”', '‘', '’'])
        .trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn candidate_field_stop(text: &str) -> Option<usize> {
    [
        " target:",
        " 目标：",
        " 人物：",
        " 对象：",
        " change:",
        " 变化：",
        " 变更：",
        " evidence:",
        " 证据：",
        " confidence",
        " 置信度",
        " affects:",
        " 影响：",
        " 影响:",
    ]
    .iter()
    .filter_map(|marker| text.find(marker))
    .min()
}

fn infer_candidate_kind(text: &str) -> &'static str {
    if text.contains("知道")
        || text.contains("不知")
        || text.contains("确认")
        || text.contains("信息边界")
    {
        "knowledge"
    } else if text.contains("关系") {
        "relationship"
    } else if text.contains("伏笔") || text.to_lowercase().contains("promise") {
        "promise"
    } else if text.contains("时间") || text.contains("事件") || text.contains("发生") {
        "event"
    } else if text.contains("地点") || text.contains("位置") {
        "location_state"
    } else if text.contains("物件") || text.contains("资源") || text.contains("归属") {
        "object_state"
    } else {
        "memory"
    }
}

fn infer_candidate_target(text: &str) -> String {
    for marker in ["target:", "目标：", "人物：", "对象："] {
        if let Some((_, rest)) = text.split_once(marker) {
            let target = rest
                .split(['，', ',', '。', ';', '；'])
                .next()
                .unwrap_or(rest)
                .trim();
            if !target.is_empty() {
                return target.to_string();
            }
        }
    }
    text.split(['：', ':', '，', ',', '。'])
        .find(|part| part.trim().chars().count() >= 2)
        .map(|part| part.trim().to_string())
        .unwrap_or_else(|| "chapter memory".to_string())
}

fn infer_confidence(text: &str) -> f32 {
    for marker in ["confidence", "置信度"] {
        if let Some((_, rest)) = text.split_once(marker) {
            let numeric = rest
                .chars()
                .skip_while(|ch| !ch.is_ascii_digit())
                .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
                .collect::<String>();
            if let Ok(value) = numeric.parse::<f32>() {
                return value.clamp(0.0, 1.0);
            }
        }
    }
    0.7
}

fn infer_affects(text: &str) -> Vec<String> {
    let Some((_, rest)) = text
        .split_once("affects:")
        .or_else(|| text.split_once("影响："))
        .or_else(|| text.split_once("影响:"))
    else {
        return Vec::new();
    };
    rest.split(['，', ',', ';', '；'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .take(8)
        .map(str::to_string)
        .collect()
}

fn memory_label_from_json(value: &serde_json::Value) -> Option<String> {
    for key in [
        "label", "title", "target", "change", "event", "fact", "promise",
    ] {
        if let Some(label) = value.get(key).and_then(serde_json::Value::as_str) {
            let label = label.trim();
            if !label.is_empty() {
                return Some(label.to_string());
            }
        }
    }
    None
}

fn add_derived_memory_aliases(value: &mut serde_json::Value) {
    let serde_json::Value::Object(map) = value else {
        return;
    };
    let mut aliases = map
        .get("aliases")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    for key in ["id", "promise_id", "foreshadowing_id"] {
        if let Some(text) = map
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            && !aliases.iter().any(|alias| alias.as_str() == Some(text))
        {
            aliases.push(serde_json::Value::String(text.to_string()));
        }
    }
    if !aliases.is_empty() {
        map.insert("aliases".to_string(), serde_json::Value::Array(aliases));
    }
}

fn memory_summary_from_json(value: &serde_json::Value) -> String {
    let kind = value
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .map(canonical_memory_kind)
        .unwrap_or_else(|| "memory".to_string());
    let mut parts = Vec::new();
    parts.push(format!("[{kind}]"));
    if let Some(chapter) = value
        .get("chapter")
        .or_else(|| value.get("first_chapter"))
        .and_then(serde_json::Value::as_u64)
    {
        parts.push(format!("chapter {chapter:03}"));
    }
    if let Some(status) = value.get("status").and_then(serde_json::Value::as_str) {
        parts.push(format!("status: {status}"));
    }
    if let Some(label) = memory_label_from_json(value) {
        parts.push(label);
    }
    if let Some(change) = value
        .get("change")
        .or_else(|| value.get("progress"))
        .or_else(|| value.get("payoff"))
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        && !parts.iter().any(|part| part == change)
    {
        parts.push(format!("change: {change}"));
    }
    if let Some(evidence) = value
        .get("evidence")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        parts.push(format!("evidence: {evidence}"));
    }
    limit_chars(&parts.join(" | "), 360)
}

fn yaml_scalar_value(text: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    let line = text
        .lines()
        .find(|line| line.trim_start().starts_with(&prefix))?;
    let (_, value) = line.split_once(':')?;
    let value = value.trim().trim_matches('"').trim_matches('\'');
    (!value.is_empty() && value != "[]").then(|| value.to_string())
}

fn yaml_list_or_scalar_values(text: &str, key: &str) -> Vec<String> {
    let prefix = format!("{key}:");
    let lines = text.lines().collect::<Vec<_>>();
    let Some(index) = lines
        .iter()
        .position(|line| line.trim_start().starts_with(&prefix))
    else {
        return Vec::new();
    };
    let Some((_, inline)) = lines[index].split_once(':') else {
        return Vec::new();
    };
    let inline = inline.trim();
    if inline.starts_with('[') && inline.ends_with(']') {
        return inline
            .trim_matches(['[', ']'])
            .split(',')
            .map(|value| value.trim().trim_matches('"').trim_matches('\''))
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect();
    }
    if !inline.is_empty() {
        return vec![inline.trim_matches('"').trim_matches('\'').to_string()];
    }

    let mut values = Vec::new();
    for line in lines.iter().skip(index + 1) {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        if !trimmed.starts_with('-') {
            break;
        }
        let value = trimmed
            .trim_start_matches('-')
            .trim()
            .trim_matches('"')
            .trim_matches('\'');
        if !value.is_empty() {
            values.push(value.to_string());
        }
    }
    values
}

fn add_textual_memory_edges(nodes: &[MemoryNode], edges: &mut Vec<MemoryEdge>) {
    let entity_nodes: Vec<&MemoryNode> = nodes
        .iter()
        .filter(|node| memory_node_is_mention_target(node))
        .collect();
    for source in nodes {
        if source.kind == "book" {
            continue;
        }
        let haystack = format!("{} {}", source.label, source.summary);
        let mut mentions = entity_nodes
            .iter()
            .filter_map(|target| {
                if source.id == target.id {
                    return None;
                }
                let matched_alias = memory_mention_match(&haystack, target)?;
                let score = memory_mention_target_score(source, target, &matched_alias);
                Some((*target, matched_alias, score))
            })
            .collect::<Vec<_>>();
        mentions.sort_by(|left, right| {
            right
                .2
                .total_cmp(&left.2)
                .then_with(|| left.0.id.cmp(&right.0.id))
        });
        let mention_limit = memory_mention_limit_for_source(source);
        for (target, matched_alias, score) in mentions.into_iter().take(mention_limit) {
            push_memory_edge(
                edges,
                "MENTIONS",
                &source.id,
                &target.id,
                &source.source,
                memory_mention_confidence(score, &matched_alias, target),
            );
        }
    }
}

fn memory_node_is_mention_target(node: &MemoryNode) -> bool {
    matches!(
        node.kind.as_str(),
        "character" | "world" | "location" | "promise" | "object"
    )
}

fn memory_mention_limit_for_source(source: &MemoryNode) -> usize {
    match source.kind.as_str() {
        "chapter" => 16,
        "memory_archive" => 14,
        "memory" | "outline" | "bible" | "world" => 10,
        "character" | "location" | "object" | "promise" => 8,
        _ => MEMORY_MENTION_DEFAULT_LIMIT,
    }
}

fn memory_mention_match(haystack: &str, target: &MemoryNode) -> Option<String> {
    let mut aliases = vec![target.label.trim().to_string()];
    aliases.extend(memory_node_aliases(target));
    aliases.sort_by_key(|alias| std::cmp::Reverse(alias.chars().count()));
    aliases.dedup();
    aliases
        .into_iter()
        .filter(|alias| memory_alias_is_searchable(alias))
        .take(MEMORY_MENTION_ALIAS_LIMIT)
        .find(|alias| haystack.contains(alias))
}

fn memory_alias_is_searchable(alias: &str) -> bool {
    let chars = alias.chars().count();
    (2..=48).contains(&chars)
}

fn memory_mention_target_score(
    source: &MemoryNode,
    target: &MemoryNode,
    matched_alias: &str,
) -> f32 {
    let mut score = memory_mention_kind_weight(target.kind.as_str());
    score += (matched_alias.chars().count().min(12) as f32) * 0.2;
    if source.kind == "chapter" {
        score += 2.0;
    }
    if source.source.starts_with("memory/summaries/") {
        score += 1.5;
    }
    if let (Some(source_chapter), Some(target_chapter)) = (
        chapter_number_from_source(&source.source)
            .or_else(|| chapter_number_from_node_id(&source.id))
            .or_else(|| state_chapter(source, &["chapter", "first_chapter"])),
        chapter_number_from_source(&target.source)
            .or_else(|| chapter_number_from_node_id(&target.id))
            .or_else(|| state_chapter(target, &["chapter", "first_chapter"])),
    ) {
        let distance = source_chapter.abs_diff(target_chapter);
        score += (3.0 / (distance as f32 + 1.0)).max(0.2);
    }
    score
}

fn memory_mention_kind_weight(kind: &str) -> f32 {
    match kind {
        "character" => 8.0,
        "promise" => 7.0,
        "location" => 6.0,
        "object" => 5.0,
        "world" => 3.0,
        _ => 1.0,
    }
}

fn memory_mention_confidence(score: f32, matched_alias: &str, target: &MemoryNode) -> f32 {
    let mut confidence: f32 = 0.58 + (score / 40.0);
    if matched_alias == target.label {
        confidence += 0.08;
    }
    confidence.clamp(0.58, 0.86)
}

fn dedupe_memory_edges(edges: &mut Vec<MemoryEdge>) {
    let mut seen = BTreeSet::new();
    edges.retain(|edge| {
        seen.insert(format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}",
            edge.kind, edge.source, edge.target, edge.evidence
        ))
    });
}

fn memory_file_node_id(path: &str) -> String {
    format!("asset:{}", sanitize_graph_id(path))
}

fn chapter_node_id(chapter: u32) -> String {
    format!("chapter:{chapter:03}")
}

fn memory_kind_for_path(path: &str) -> &'static str {
    if path.starts_with("cards/characters") {
        "character"
    } else if path.starts_with("cards/resources") {
        "object"
    } else if path.starts_with("cards/locations") {
        "location"
    } else if path.starts_with("cards/world") || path.starts_with("bible/world") {
        "world"
    } else if path.starts_with("outline") {
        "outline"
    } else if path.contains("foreshadow") {
        "promise"
    } else if path.starts_with("memory") {
        "memory"
    } else if path.starts_with("bible") {
        "bible"
    } else {
        "asset"
    }
}

fn card_label(path: &str, text: &str) -> String {
    for key in ["name:", "title:", "id:"] {
        if let Some(line) = text.lines().find(|line| line.trim_start().starts_with(key))
            && let Some((_, value)) = line.split_once(':')
        {
            let value = value.trim().trim_matches('"').trim_matches('\'');
            if !value.is_empty() {
                return value.to_string();
            }
        }
    }
    if let Some(heading) = text
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("# ").map(str::trim))
        .filter(|value| !value.is_empty())
    {
        return heading.to_string();
    }
    Path::new(path)
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or(path)
        .to_string()
}

fn sanitize_graph_id(input: &str) -> String {
    input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ('\u{4e00}'..='\u{9fff}').contains(&ch) {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn stable_hash(input: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in input.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn summarize_for_graph(text: &str, limit: usize) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(limit)
        .collect()
}

fn memory_asset_summary_for_graph(path: &str, text: &str, limit: usize) -> String {
    if !path.starts_with("memory/") || !path.ends_with(".jsonl") {
        return summarize_for_graph(text, limit);
    }
    let mut records = Vec::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if records.len() >= 6 {
            records.push("...".to_string());
            break;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
            records.push(memory_summary_from_json(&value));
        } else {
            records.push(summarize_for_graph(line, 120));
        }
    }
    if records.is_empty() {
        String::new()
    } else {
        limit_chars(&records.join(" | "), limit)
    }
}

fn print_memory_graph_status(graph: &MemoryGraph) {
    let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
    for node in &graph.nodes {
        *kinds.entry(&node.kind).or_insert(0) += 1;
    }
    println!("Memory graph updated: {}", graph.updated_at);
    println!("Schema version: {}", graph.schema_version);
    println!("Nodes: {}", graph.nodes.len());
    println!("Edges: {}", graph.edges.len());
    println!(
        "Candidate memory updates: {}",
        graph.candidate_updates.len()
    );
    let promise_statuses = promise_status_counts(graph);
    if !promise_statuses.is_empty() {
        println!("Promise lifecycle:");
        for (status, count) in promise_statuses {
            println!("- {status}: {count}");
        }
    }
    let relationship_changes = graph
        .nodes
        .iter()
        .filter(|node| {
            node.kind == "relationship"
                && node
                    .state
                    .get("change")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|value| !value.trim().is_empty())
        })
        .count();
    let state_changes = graph
        .nodes
        .iter()
        .filter(|node| {
            matches!(
                node.kind.as_str(),
                "character_state" | "location_state" | "object_state"
            )
        })
        .count();
    println!("Relationship changes: {relationship_changes}");
    println!("State changes: {state_changes}");
    for (kind, count) in kinds {
        println!("{kind}: {count}");
    }
    let mut degree: BTreeMap<&str, usize> = BTreeMap::new();
    for edge in &graph.edges {
        *degree.entry(&edge.source).or_insert(0) += 1;
        *degree.entry(&edge.target).or_insert(0) += 1;
    }
    let labels: BTreeMap<&str, &str> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node.label.as_str()))
        .collect();
    let mut ranked: Vec<(&str, usize)> = degree.into_iter().collect();
    ranked.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    if !ranked.is_empty() {
        println!("Top memory hubs:");
        for (id, count) in ranked.into_iter().take(8) {
            println!("- {} ({count})", labels.get(id).copied().unwrap_or(id));
        }
    }
}

fn find_memory_seed_nodes(graph: &MemoryGraph, query: &str) -> Vec<String> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }
    let query_lower = query.to_lowercase();
    let mut seed_scores: BTreeMap<String, f32> = BTreeMap::new();
    for node in &graph.nodes {
        if let Some(score) = memory_seed_node_score(node, query, &query_lower) {
            add_memory_seed_score(&mut seed_scores, node.id.clone(), score);
        }
    }
    for (alias, ids) in memory_alias_index(graph) {
        if let Some(score) = memory_seed_text_score(&alias, query, &query_lower) {
            for id in ids {
                add_memory_seed_score(&mut seed_scores, id, score + 2.0);
            }
        }
    }
    let mut ranked = seed_scores.into_iter().collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    ranked
        .into_iter()
        .take(MEMORY_QUERY_SEED_LIMIT)
        .map(|(id, _)| id)
        .collect()
}

fn add_memory_seed_score(scores: &mut BTreeMap<String, f32>, id: String, score: f32) {
    scores
        .entry(id)
        .and_modify(|existing| *existing = (*existing).max(score))
        .or_insert(score);
}

fn memory_seed_node_score(node: &MemoryNode, query: &str, query_lower: &str) -> Option<f32> {
    if node.id.eq_ignore_ascii_case(query) {
        return Some(100.0);
    }
    let mut best: Option<f32> = None;
    for (text, base) in [
        (node.label.as_str(), 80.0),
        (node.source.as_str(), 45.0),
        (node.summary.as_str(), 20.0),
    ] {
        if let Some(score) = memory_seed_text_score(text, query, query_lower) {
            best = Some(best.map_or(score + base, |value| value.max(score + base)));
        }
    }
    for alias in memory_node_aliases(node) {
        if let Some(score) = memory_seed_text_score(&alias, query, query_lower) {
            best = Some(best.map_or(score + 70.0, |value| value.max(score + 70.0)));
        }
    }
    best
}

fn memory_seed_text_score(text: &str, query: &str, query_lower: &str) -> Option<f32> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if text.eq_ignore_ascii_case(query) {
        return Some(30.0);
    }
    let text_lower = text.to_lowercase();
    if text_lower == query_lower {
        return Some(30.0);
    }
    if text_lower.starts_with(query_lower) {
        return Some(22.0);
    }
    if text_lower.contains(query_lower) {
        return Some(12.0);
    }
    if query_lower.contains(&text_lower) && text.chars().count() >= 2 {
        return Some(8.0);
    }
    None
}

fn memory_alias_index(graph: &MemoryGraph) -> BTreeMap<String, Vec<String>> {
    let mut index: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for node in &graph.nodes {
        for alias in memory_node_aliases(node) {
            index.entry(alias).or_default().push(node.id.clone());
        }
    }
    for ids in index.values_mut() {
        ids.sort();
        ids.dedup();
    }
    index
}

fn memory_node_aliases(node: &MemoryNode) -> Vec<String> {
    let mut aliases = Vec::new();
    collect_state_aliases(
        &node.state,
        &[
            "id",
            "name",
            "alias",
            "aliases",
            "aka",
            "also_known_as",
            "short_name",
            "nicknames",
            "title",
            "promise_id",
            "foreshadowing_id",
            "target",
            "item",
            "object",
            "location",
            "place",
            "slug",
            "别名",
            "短名",
            "物品别名",
            "地点别名",
            "角色别名",
            "伏笔id",
        ],
        &mut aliases,
    );
    aliases.extend(extract_alias_markers(&node.summary));
    aliases.sort();
    aliases.dedup();
    aliases
}

fn collect_state_aliases(value: &serde_json::Value, keys: &[&str], out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                if keys.iter().any(|wanted| key.eq_ignore_ascii_case(wanted)) {
                    collect_alias_value(value, out);
                }
                collect_state_aliases(value, keys, out);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect_state_aliases(value, keys, out);
            }
        }
        _ => {}
    }
}

fn collect_alias_value(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(text) => push_alias_text(text, out),
        serde_json::Value::Array(values) => {
            for value in values {
                collect_alias_value(value, out);
            }
        }
        serde_json::Value::Object(map) => {
            for value in map.values() {
                collect_alias_value(value, out);
            }
        }
        _ => {}
    }
}

fn push_alias_text(text: &str, out: &mut Vec<String>) {
    let text = text.trim();
    if !text.is_empty() {
        out.push(text.to_string());
    }
}

fn extract_alias_markers(text: &str) -> Vec<String> {
    let mut aliases = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        let Some((key, rest)) = trimmed.split_once(':').or_else(|| trimmed.split_once('：'))
        else {
            continue;
        };
        let key = key.trim().to_lowercase();
        if !matches!(
            key.as_str(),
            "alias"
                | "aliases"
                | "aka"
                | "别名"
                | "伏笔id"
                | "promise_id"
                | "foreshadowing_id"
                | "角色别名"
                | "物品别名"
                | "地点别名"
                | "短名"
        ) {
            continue;
        }
        for part in rest.split([',', '，', '/', '|', '、']) {
            push_alias_text(part, &mut aliases);
        }
    }
    aliases
}

fn memory_neighborhood(
    graph: &MemoryGraph,
    seeds: &[String],
    depth: usize,
    limit: usize,
) -> GraphNeighborhood {
    let node_by_id: BTreeMap<&str, &MemoryNode> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let mut adjacency: BTreeMap<&str, Vec<&MemoryEdge>> = BTreeMap::new();
    for edge in &graph.edges {
        adjacency.entry(&edge.source).or_default().push(edge);
        adjacency.entry(&edge.target).or_default().push(edge);
    }

    let mut selected = BTreeSet::new();
    let mut best_scores = BTreeMap::<String, f32>::new();
    let mut queue = BinaryHeap::new();
    for seed in seeds {
        if node_by_id.contains_key(seed.as_str()) {
            best_scores.insert(seed.clone(), 10_000.0);
            queue.push(MemoryFrontier {
                node_id: seed.clone(),
                depth: 0,
                score: 10_000.0,
            });
        }
    }

    while let Some(frontier) = queue.pop() {
        if selected.len() >= limit {
            break;
        }
        if best_scores
            .get(&frontier.node_id)
            .is_some_and(|score| frontier.score < *score)
        {
            continue;
        }
        let current = frontier.node_id.clone();
        selected.insert(current.clone());
        if frontier.depth >= depth {
            continue;
        }
        if let Some(edges) = adjacency.get(current.as_str()) {
            let mut weighted_edges = edges
                .iter()
                .filter_map(|edge| {
                    if edge.kind == "CONTAINS" && (edge.source == "book" || edge.target == "book") {
                        return None;
                    }
                    let next = if edge.source == current {
                        edge.target.as_str()
                    } else {
                        edge.source.as_str()
                    };
                    let next_node = *node_by_id.get(next)?;
                    let score = memory_edge_score(edge, next_node);
                    Some((*edge, next.to_string(), score))
                })
                .collect::<Vec<_>>();
            weighted_edges.sort_by(|left, right| {
                right
                    .2
                    .total_cmp(&left.2)
                    .then_with(|| left.0.kind.cmp(&right.0.kind))
                    .then_with(|| left.1.cmp(&right.1))
            });
            for (_, next, edge_score) in
                weighted_edges.into_iter().take(MEMORY_FRONTIER_EDGE_FANOUT)
            {
                if selected.contains(&next) {
                    continue;
                }
                let next_score = frontier.score + edge_score - ((frontier.depth + 1) as f32);
                if next_score > best_scores.get(&next).copied().unwrap_or(f32::NEG_INFINITY) {
                    best_scores.insert(next.clone(), next_score);
                    queue.push(MemoryFrontier {
                        node_id: next,
                        depth: frontier.depth + 1,
                        score: next_score,
                    });
                }
            }
        }
    }

    let mut nodes: Vec<MemoryNode> = graph
        .nodes
        .iter()
        .filter(|node| selected.contains(&node.id))
        .cloned()
        .collect();
    let seed_set = seeds.iter().map(String::as_str).collect::<BTreeSet<_>>();
    nodes.sort_by(|left, right| {
        let left_score = best_scores.get(&left.id).copied().unwrap_or_default();
        let right_score = best_scores.get(&right.id).copied().unwrap_or_default();
        let left_seed = seed_set.contains(left.id.as_str());
        let right_seed = seed_set.contains(right.id.as_str());
        right_seed
            .cmp(&left_seed)
            .then_with(|| right_score.total_cmp(&left_score))
            .then_with(|| left.source.cmp(&right.source))
            .then_with(|| left.id.cmp(&right.id))
    });
    let mut edges: Vec<MemoryEdge> = graph
        .edges
        .iter()
        .filter(|edge| selected.contains(&edge.source) && selected.contains(&edge.target))
        .cloned()
        .collect();
    edges.sort_by(|left, right| {
        let left_target = node_by_id
            .get(left.target.as_str())
            .or_else(|| node_by_id.get(left.source.as_str()))
            .copied();
        let right_target = node_by_id
            .get(right.target.as_str())
            .or_else(|| node_by_id.get(right.source.as_str()))
            .copied();
        let left_score = left_target.map_or(0.0, |node| memory_edge_score(left, node));
        let right_score = right_target.map_or(0.0, |node| memory_edge_score(right, node));
        right_score
            .total_cmp(&left_score)
            .then_with(|| left.kind.cmp(&right.kind))
            .then_with(|| left.source.cmp(&right.source))
            .then_with(|| left.target.cmp(&right.target))
    });
    edges.truncate(MEMORY_NEIGHBORHOOD_EDGE_LIMIT);
    GraphNeighborhood { nodes, edges }
}

#[derive(Debug, Clone)]
struct MemoryFrontier {
    node_id: String,
    depth: usize,
    score: f32,
}

impl Eq for MemoryFrontier {}

impl PartialEq for MemoryFrontier {
    fn eq(&self, other: &Self) -> bool {
        self.score.to_bits() == other.score.to_bits() && self.node_id == other.node_id
    }
}

impl Ord for MemoryFrontier {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.score
            .total_cmp(&other.score)
            .then_with(|| other.depth.cmp(&self.depth))
            .then_with(|| other.node_id.cmp(&self.node_id))
    }
}

impl PartialOrd for MemoryFrontier {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

fn memory_edge_score(edge: &MemoryEdge, next_node: &MemoryNode) -> f32 {
    edge.confidence * 10.0
        + memory_edge_kind_weight(&edge.kind)
        + memory_node_recency_weight(next_node)
        + chapter_distance_weight(edge, next_node)
}

fn memory_edge_kind_weight(kind: &str) -> f32 {
    match kind {
        "CHANGES" | "KNOWS" | "DOES_NOT_KNOW" => 11.0,
        "AFFECTS" | "PROMISES" | "FORESHADOWS" | "PAYS_OFF" => 9.0,
        "SUMMARIZED_BY" | "SUMMARIZES" | "CANDIDATE_UPDATE" => 7.0,
        "MENTIONS" | "INVOLVES" => 5.0,
        "LOCATED_AT" | "HAS_RESOURCE" | "RELATES_TO" => 5.0,
        "NEXT" => 3.0,
        "CONTAINS" => 1.0,
        _ => 4.0,
    }
}

fn memory_node_recency_weight(node: &MemoryNode) -> f32 {
    let source = node.source.as_str();
    if source.starts_with("memory/summaries/") {
        return 4.0;
    }
    if source.starts_with("memory/candidates/") || node.kind.contains("candidate") {
        return 3.0;
    }
    if source.starts_with("chapters/") {
        return 2.0;
    }
    0.0
}

fn chapter_distance_weight(edge: &MemoryEdge, next_node: &MemoryNode) -> f32 {
    let Some(current_chapter) = chapter_number_from_node_id(&edge.source)
        .or_else(|| chapter_number_from_node_id(&edge.target))
        .or_else(|| chapter_number_from_source(&edge.evidence))
    else {
        return 0.0;
    };
    let Some(next_chapter) = chapter_number_from_node_id(&next_node.id)
        .or_else(|| chapter_number_from_source(&next_node.source))
    else {
        return 0.0;
    };
    let distance = current_chapter.abs_diff(next_chapter);
    if distance == 0 {
        4.0
    } else {
        (4.0 / (distance as f32 + 1.0)).max(0.25)
    }
}

fn chapter_number_from_node_id(value: &str) -> Option<u32> {
    value
        .strip_prefix("chapter:")
        .or_else(|| value.strip_prefix("chapter_"))
        .and_then(|rest| {
            rest.chars()
                .take_while(|ch| ch.is_ascii_digit())
                .collect::<String>()
                .parse::<u32>()
                .ok()
        })
}

fn chapter_number_from_source(value: &str) -> Option<u32> {
    let marker = "chapters/";
    let start = value.find(marker)? + marker.len();
    value
        .get(start..)?
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>()
        .parse::<u32>()
        .ok()
}

fn memory_context_packet(
    root: &Path,
    graph: &MemoryGraph,
    chapter: u32,
    depth: usize,
    limit: usize,
) -> Result<String> {
    let mut seeds = vec![chapter_node_id(chapter)];
    if chapter > 1 {
        seeds.push(chapter_node_id(chapter - 1));
    }
    if let Some(text) = read_optional_limited(&chapter_dir(root, chapter).join("brief.md"), 4_000) {
        for node in &graph.nodes {
            if matches!(node.kind.as_str(), "character" | "world" | "location")
                && text.contains(&node.label)
            {
                seeds.push(node.id.clone());
            }
        }
    }
    seeds.retain(|seed| graph.nodes.iter().any(|node| node.id == *seed));
    seeds.sort();
    seeds.dedup();
    if seeds.is_empty() {
        return Ok("# Memory Graph Context\n\nNo graph seeds found for this chapter.".to_string());
    }
    let neighborhood = memory_neighborhood(graph, &seeds, depth, limit);
    Ok(format!(
        "# Memory Graph Context\n\nSeed chapter: {chapter:03}\nDepth: {depth}\nLimit: {limit}\n\n{}",
        format_memory_neighborhood(&neighborhood)
    ))
}

fn format_memory_neighborhood(neighborhood: &GraphNeighborhood) -> String {
    let mut out = String::new();
    if let Some(semantics) = format_story_semantics(neighborhood) {
        out.push_str(&semantics);
        out.push('\n');
    }
    out.push_str("## Nodes\n\n");
    for node in &neighborhood.nodes {
        out.push_str(&format!(
            "- [{}] {} — {} ({})\n",
            node.kind, node.label, node.source, node.id
        ));
        if !node.summary.trim().is_empty() {
            out.push_str(&format!("  {}\n", limit_chars(&node.summary, 300)));
        }
    }
    out.push_str("\n## Edges\n\n");
    if neighborhood.edges.is_empty() {
        out.push_str("- none\n");
    } else {
        let labels: BTreeMap<&str, &str> = neighborhood
            .nodes
            .iter()
            .map(|node| (node.id.as_str(), node.label.as_str()))
            .collect();
        for edge in &neighborhood.edges {
            let source = labels
                .get(edge.source.as_str())
                .copied()
                .unwrap_or(&edge.source);
            let target = labels
                .get(edge.target.as_str())
                .copied()
                .unwrap_or(&edge.target);
            out.push_str(&format!(
                "- {} --{}--> {} | evidence: {} | confidence: {:.2}\n",
                source, edge.kind, target, edge.evidence, edge.confidence
            ));
        }
    }
    out
}

fn format_story_semantics(neighborhood: &GraphNeighborhood) -> Option<String> {
    let mut sections = Vec::new();
    sections.extend(format_character_semantics(neighborhood));
    sections.extend(format_promise_semantics(neighborhood));
    sections.extend(format_event_semantics(neighborhood));
    sections.extend(format_relationship_semantics(neighborhood));
    sections.extend(format_state_change_semantics(neighborhood));
    if sections.is_empty() {
        None
    } else {
        Some(format!("## Story Semantics\n\n{}", sections.join("\n")))
    }
}

fn format_character_semantics(neighborhood: &GraphNeighborhood) -> Vec<String> {
    let labels = memory_labels(neighborhood);
    let mut sections = Vec::new();
    for node in neighborhood
        .nodes
        .iter()
        .filter(|node| node.kind == "character")
    {
        let mut lines = Vec::new();
        if let Some(value) = state_string(node, &["current_state", "state"]) {
            lines.push(format!("- Current state: {}", limit_chars(&value, 220)));
        }
        if let Some(value) = state_string(node, &["role"]) {
            lines.push(format!("- Role: {}", limit_chars(&value, 160)));
        }
        if let Some(value) = state_string(node, &["want"]) {
            lines.push(format!("- Wants: {}", limit_chars(&value, 180)));
        }
        if let Some(value) = state_string(node, &["fear"]) {
            lines.push(format!("- Fears: {}", limit_chars(&value, 180)));
        }

        let knows = semantic_values_for_node(
            neighborhood,
            &labels,
            node,
            &["knowledge"],
            &["KNOWS"],
            &["knowledge"],
        );
        let unknowns = semantic_values_for_node(
            neighborhood,
            &labels,
            node,
            &["unknown"],
            &["DOES_NOT_KNOW"],
            &["knowledge"],
        );
        if !knows.is_empty() || !unknowns.is_empty() {
            lines.push("- Knowledge boundaries:".to_string());
            if !knows.is_empty() {
                lines.push(format!("  - Knows: {}", knows.join("; ")));
            }
            if !unknowns.is_empty() {
                lines.push(format!("  - Does not know: {}", unknowns.join("; ")));
            }
        }

        let relationships = semantic_values_for_node(
            neighborhood,
            &labels,
            node,
            &["relationships"],
            &["AFFECTS"],
            &["relationship"],
        );
        if !relationships.is_empty() {
            lines.push(format!("- Relationships: {}", relationships.join("; ")));
        }

        let secrets = semantic_values_for_node(
            neighborhood,
            &labels,
            node,
            &["secret"],
            &["KNOWS"],
            &["secret"],
        );
        if !secrets.is_empty() {
            lines.push(format!("- Secrets: {}", secrets.join("; ")));
        }

        let appearances = character_appearances(neighborhood, &labels, &node.id);
        if !appearances.is_empty() {
            lines.push(format!("- Appearances: {}", appearances.join(", ")));
        }

        if !lines.is_empty() {
            sections.push(format!(
                "### Character: {}\n{}",
                node.label,
                lines.join("\n")
            ));
        }
    }
    sections
}

fn format_promise_semantics(neighborhood: &GraphNeighborhood) -> Vec<String> {
    let mut sections = Vec::new();
    for node in neighborhood
        .nodes
        .iter()
        .filter(|node| node.kind == "promise" && !node.id.starts_with("asset:"))
    {
        let mut lines = Vec::new();
        if let Some(value) = state_string(node, &["status"]) {
            lines.push(format!(
                "- Lifecycle status: {}",
                promise_status_label(&value)
            ));
        }
        if let Some(value) = state_chapter(node, &["first_chapter", "chapter"]) {
            lines.push(format!("- First appearance: chapter {value:03}"));
        }
        if let Some(value) = state_string(node, &["progress", "advance", "advances"]) {
            lines.push(format!("- Progress: {}", limit_chars(&value, 220)));
        }
        if let Some(value) = state_string(node, &["payoff", "payoff_evidence"]) {
            lines.push(format!("- Payoff: {}", limit_chars(&value, 220)));
        }
        if let Some(value) = state_chapter(node, &["payoff_chapter"]) {
            lines.push(format!("- Payoff chapter: {value:03}"));
        }
        if let Some(value) = state_string(node, &["suspended_reason", "abandoned_reason"]) {
            lines.push(format!("- Hold reason: {}", limit_chars(&value, 220)));
        }
        if lines.is_empty() && !node.summary.trim().is_empty() {
            lines.push(format!("- Summary: {}", limit_chars(&node.summary, 220)));
        }
        if !lines.is_empty() {
            sections.push(format!("### Promise: {}\n{}", node.label, lines.join("\n")));
        }
    }
    sections
}

fn format_event_semantics(neighborhood: &GraphNeighborhood) -> Vec<String> {
    let mut events = neighborhood
        .nodes
        .iter()
        .filter(|node| node.kind == "event" && !node.id.starts_with("asset:"))
        .filter_map(|node| {
            let chapter = state_chapter(node, &["chapter", "first_chapter"]);
            let label = if let Some(chapter) = chapter {
                format!("chapter {chapter:03}: {}", node.label)
            } else {
                node.label.clone()
            };
            (!label.trim().is_empty()).then_some((chapter.unwrap_or(u32::MAX), label))
        })
        .collect::<Vec<_>>();
    events.sort_by_key(|(chapter, label)| (*chapter, label.clone()));
    events.dedup_by(|left, right| left.1 == right.1);
    if events.is_empty() {
        Vec::new()
    } else {
        let lines = events
            .into_iter()
            .take(12)
            .map(|(_, label)| format!("- {}", limit_chars(&label, 220)))
            .collect::<Vec<_>>()
            .join("\n");
        vec![format!("### Event Timeline\n{lines}")]
    }
}

fn format_relationship_semantics(neighborhood: &GraphNeighborhood) -> Vec<String> {
    let mut relationships = neighborhood
        .nodes
        .iter()
        .filter(|node| node.kind == "relationship")
        .map(|node| {
            if let Some(change) = state_string(node, &["change"]) {
                let chapter = state_chapter(node, &["chapter"])
                    .map(|chapter| format!("chapter {chapter:03}: "))
                    .unwrap_or_default();
                let target = state_string(node, &["target"])
                    .map(|target| format!("{target} -> "))
                    .unwrap_or_default();
                format!("{chapter}{target}{}", limit_chars(&change, 220))
            } else if !node.summary.trim().is_empty() && node.summary != node.label {
                format!("{}: {}", node.label, limit_chars(&node.summary, 180))
            } else {
                node.label.clone()
            }
        })
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>();
    relationships.sort();
    relationships.dedup();
    if relationships.is_empty() {
        Vec::new()
    } else {
        let lines = relationships
            .into_iter()
            .take(12)
            .map(|value| format!("- {}", limit_chars(&value, 220)))
            .collect::<Vec<_>>()
            .join("\n");
        vec![format!("### Relationship States\n{lines}")]
    }
}

fn format_state_change_semantics(neighborhood: &GraphNeighborhood) -> Vec<String> {
    let mut states = neighborhood
        .nodes
        .iter()
        .filter(|node| {
            matches!(
                node.kind.as_str(),
                "character_state" | "location_state" | "object_state"
            )
        })
        .map(|node| {
            let chapter = state_chapter(node, &["chapter"])
                .map(|chapter| format!("chapter {chapter:03}: "))
                .unwrap_or_default();
            let target = state_string(node, &["target"])
                .map(|target| format!("{target} -> "))
                .unwrap_or_default();
            let change = state_string(node, &["change"])
                .unwrap_or_else(|| node.summary.clone())
                .trim()
                .to_string();
            format!(
                "{}{}{} [{}]",
                chapter,
                target,
                limit_chars(&change, 220),
                node.kind
            )
        })
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>();
    states.sort();
    states.dedup();
    if states.is_empty() {
        Vec::new()
    } else {
        let lines = states
            .into_iter()
            .take(12)
            .map(|value| format!("- {value}"))
            .collect::<Vec<_>>()
            .join("\n");
        vec![format!("### State Changes\n{lines}")]
    }
}

fn semantic_values_for_node(
    neighborhood: &GraphNeighborhood,
    labels: &BTreeMap<&str, &str>,
    node: &MemoryNode,
    state_keys: &[&str],
    edge_kinds: &[&str],
    target_kinds: &[&str],
) -> Vec<String> {
    let mut values = Vec::new();
    for key in state_keys {
        values.extend(state_strings(node, key));
    }
    for edge in neighborhood.edges.iter().filter(|edge| {
        edge.source == node.id
            && edge_kinds.contains(&edge.kind.as_str())
            && neighborhood
                .nodes
                .iter()
                .find(|candidate| candidate.id == edge.target)
                .is_some_and(|candidate| target_kinds.contains(&candidate.kind.as_str()))
    }) {
        if let Some(target_node) = neighborhood
            .nodes
            .iter()
            .find(|candidate| candidate.id == edge.target)
        {
            values.push(semantic_node_display_value(target_node, node));
        } else {
            values.push(
                labels
                    .get(edge.target.as_str())
                    .copied()
                    .unwrap_or(edge.target.as_str())
                    .to_string(),
            );
        }
    }
    clean_semantic_values_for_node(values, node)
}

fn semantic_node_display_value(target: &MemoryNode, source: &MemoryNode) -> String {
    state_string(target, &["change", "progress", "payoff", "fact", "event"])
        .or_else(|| {
            if !target.summary.trim().is_empty()
                && target.summary != target.label
                && !target.summary.starts_with("Inferred ")
            {
                Some(target.summary.clone())
            } else {
                None
            }
        })
        .unwrap_or_else(|| {
            if target.label == source.label {
                String::new()
            } else {
                target.label.clone()
            }
        })
}

fn character_appearances(
    neighborhood: &GraphNeighborhood,
    labels: &BTreeMap<&str, &str>,
    character_id: &str,
) -> Vec<String> {
    let mut chapters = neighborhood
        .edges
        .iter()
        .filter(|edge| edge.kind == "MENTIONS")
        .filter_map(|edge| {
            let chapter_id = if edge.source.starts_with("chapter:") && edge.target == character_id {
                Some(edge.source.as_str())
            } else if edge.target.starts_with("chapter:") && edge.source == character_id {
                Some(edge.target.as_str())
            } else {
                None
            }?;
            Some(
                labels
                    .get(chapter_id)
                    .copied()
                    .unwrap_or(chapter_id)
                    .to_string(),
            )
        })
        .collect::<Vec<_>>();
    chapters.sort();
    chapters.dedup();
    chapters.truncate(12);
    chapters
}

fn memory_labels(neighborhood: &GraphNeighborhood) -> BTreeMap<&str, &str> {
    neighborhood
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node.label.as_str()))
        .collect()
}

fn state_string(node: &MemoryNode, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| {
            node.state
                .get(*key)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
        .or_else(|| {
            keys.iter()
                .find_map(|key| state_strings(node, key).into_iter().next())
        })
}

fn state_strings(node: &MemoryNode, key: &str) -> Vec<String> {
    match node.state.get(key) {
        Some(serde_json::Value::String(value)) => vec![value.clone()],
        Some(serde_json::Value::Array(values)) => values
            .iter()
            .filter_map(|value| {
                value
                    .as_str()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            })
            .collect(),
        Some(value) if value.is_object() || value.is_array() => vec![value.to_string()],
        _ => Vec::new(),
    }
}

fn state_chapter(node: &MemoryNode, keys: &[&str]) -> Option<u32> {
    keys.iter().find_map(|key| {
        let value = node.state.get(*key)?;
        value
            .as_u64()
            .and_then(|chapter| u32::try_from(chapter).ok())
            .or_else(|| {
                value
                    .as_str()
                    .and_then(|raw| raw.trim().parse::<u32>().ok())
            })
    })
}

fn clean_semantic_values_for_node(values: Vec<String>, source: &MemoryNode) -> Vec<String> {
    let mut values = values
        .into_iter()
        .map(|value| limit_chars(value.trim(), 180))
        .filter(|value| !value.is_empty() && value != &source.label)
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values.truncate(12);
    values
}

fn promise_status_label(status: &str) -> String {
    match status.trim().to_lowercase().as_str() {
        "new" | "open" => format!("{status} (new/open)"),
        "progress" | "advanced" | "advancing" => format!("{status} (progress)"),
        "payoff" | "paid" | "paid_off" | "resolved" => format!("{status} (payoff)"),
        "suspended" | "paused" => format!("{status} (suspended)"),
        "abandoned" | "dropped" => format!("{status} (abandoned)"),
        _ => status.to_string(),
    }
}

fn affected_nodes_by_kind(neighborhood: &GraphNeighborhood) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for node in &neighborhood.nodes {
        let non_story_asset = node.id.starts_with("asset:") && !node.source.starts_with("cards/");
        if non_story_asset || matches!(node.kind.as_str(), "book" | "chapter" | "asset" | "memory")
        {
            continue;
        }
        out.entry(node.kind.clone())
            .or_default()
            .push(node.label.clone());
    }
    for values in out.values_mut() {
        values.sort();
        values.dedup();
        values.truncate(8);
    }
    out
}

fn downstream_chapters(graph: &MemoryGraph, chapter: u32, limit: usize) -> Vec<u32> {
    let mut downstream = Vec::new();
    let mut current = chapter_node_id(chapter);
    let mut visited = BTreeSet::new();
    while downstream.len() < limit && visited.insert(current.clone()) {
        let Some(next) = graph
            .edges
            .iter()
            .find(|edge| edge.kind == "NEXT" && edge.source == current)
            .map(|edge| edge.target.clone())
        else {
            break;
        };
        if let Some(number) = next
            .strip_prefix("chapter:")
            .and_then(|raw| raw.parse().ok())
        {
            downstream.push(number);
        }
        current = next;
    }
    downstream
}

fn collect_asset_files(dir: &Path) -> Result<Vec<PathBuf>> {
    collect_files_with_extensions(dir, &["md", "yaml", "yml", "toml", "json"])
}

fn collect_material_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = collect_files_recursive_with_extensions(
        &root.join("materials"),
        &["md", "txt", "json", "jsonl", "toml", "yaml", "yml", "csv"],
        3,
        64,
    )?;
    files.retain(|path| {
        !path
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| name.eq_ignore_ascii_case("README.md"))
    });
    Ok(files)
}

fn non_template_files(files: Vec<PathBuf>) -> Vec<PathBuf> {
    files
        .into_iter()
        .filter(|path| !is_template_asset(path))
        .collect()
}

fn append_file_list(out: &mut String, root: &Path, label: &str, files: &[PathBuf]) {
    let _ = writeln!(out, "- {label}: {}", files.len());
    for path in files {
        let _ = writeln!(out, "  - {}", display_relative(root, path));
    }
}

fn append_recent_file_list(
    out: &mut String,
    root: &Path,
    label: &str,
    files: &[PathBuf],
    limit: usize,
) {
    let _ = writeln!(out, "- {label}: {} total", files.len());
    let start = files.len().saturating_sub(limit);
    for path in &files[start..] {
        let _ = writeln!(out, "  - {}", display_relative(root, path));
    }
    if start > 0 {
        let _ = writeln!(out, "  - ... {} older omitted; use --full", start);
    }
}

fn chapter_markers(root: &Path, chapter: u32, dir: &Path) -> Result<Vec<String>> {
    let mut markers = Vec::new();
    for (file, label) in [
        ("brief.md", "brief"),
        ("craft_plan.md", "craft_plan"),
        ("draft.md", "draft"),
        ("audit.md", "audit"),
        ("final.md", "final"),
    ] {
        if dir.join(file).is_file() {
            markers.push(label.to_string());
        }
    }
    if root
        .join("memory/summaries")
        .join(format!("{chapter:03}.md"))
        .is_file()
    {
        markers.push("summary".to_string());
    }
    let versions = count_directory_files(&dir.join(".versions"))?;
    if versions > 0 {
        markers.push(format!("versions:{versions}"));
    }
    if markers.is_empty() {
        markers.push("empty".to_string());
    }
    Ok(markers)
}

fn character_state_lines(graph: &MemoryGraph) -> Vec<String> {
    let mut lines = Vec::new();
    for node in graph.nodes.iter().filter(|node| node.kind == "character") {
        let state = node
            .state
            .get("current_state")
            .and_then(serde_json::Value::as_str)
            .or_else(|| node.state.get("state").and_then(serde_json::Value::as_str))
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(node.summary.as_str());
        lines.push(format!("{}: {}", node.label, limit_chars(state, 180)));
    }
    lines.sort();
    lines
}

fn count_directory_files(dir: &Path) -> Result<usize> {
    if !dir.is_dir() {
        return Ok(0);
    }
    let mut count = 0_usize;
    for entry in
        std::fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))?
    {
        if entry?.file_type()?.is_file() {
            count += 1;
        }
    }
    Ok(count)
}

fn foreshadowing_preview(root: &Path, limit: usize) -> Result<Vec<String>> {
    let path = root.join("memory/foreshadowing.jsonl");
    let Some(text) = std::fs::read_to_string(&path).ok() else {
        return Ok(Vec::new());
    };
    let mut preview = Vec::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if preview.len() >= limit {
            break;
        }
        let value = serde_json::from_str::<serde_json::Value>(line).ok();
        if let Some(value) = value {
            let label = value
                .get("promise")
                .or_else(|| value.get("foreshadowing"))
                .or_else(|| value.get("event"))
                .or_else(|| value.get("target"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unnamed promise");
            let status = value
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            let chapter = value
                .get("chapter")
                .or_else(|| value.get("first_chapter"))
                .and_then(serde_json::Value::as_u64)
                .map(|chapter| format!("chapter {chapter:03}"))
                .unwrap_or_else(|| "chapter unknown".to_string());
            preview.push(format!("{label} | {status} | {chapter}"));
        } else {
            preview.push(limit_chars(line, 160));
        }
    }
    Ok(preview)
}

fn is_template_asset(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .map(|name| name.starts_with('_') || name.eq_ignore_ascii_case("template.yaml"))
        .unwrap_or(false)
}

fn collect_files_with_extensions(dir: &Path, extensions: &[&str]) -> Result<Vec<PathBuf>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in
        std::fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let extension = path.extension().and_then(OsStr::to_str).unwrap_or("");
        if entry.file_type()?.is_file() && extensions.contains(&extension) {
            files.push(entry.path());
        }
    }
    files.sort();
    Ok(files)
}

fn collect_files_recursive_with_extensions(
    dir: &Path,
    extensions: &[&str],
    max_depth: usize,
    max_files: usize,
) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_files_recursive_inner(dir, extensions, max_depth, max_files, &mut files)?;
    files.sort();
    files.truncate(max_files);
    Ok(files)
}

fn collect_files_recursive_inner(
    dir: &Path,
    extensions: &[&str],
    depth: usize,
    max_files: usize,
    out: &mut Vec<PathBuf>,
) -> Result<()> {
    if depth == 0 || out.len() >= max_files || !dir.is_dir() {
        return Ok(());
    }
    for entry in
        std::fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))?
    {
        if out.len() >= max_files {
            break;
        }
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_files_recursive_inner(&path, extensions, depth - 1, max_files, out)?;
        } else if file_type.is_file() {
            let extension = path
                .extension()
                .and_then(OsStr::to_str)
                .unwrap_or("")
                .to_ascii_lowercase();
            if extensions.contains(&extension.as_str()) {
                out.push(path);
            }
        }
    }
    Ok(())
}

fn recent_summary_paths(root: &Path, chapter: u32, limit: usize) -> Result<Vec<PathBuf>> {
    let summaries = root.join("memory/summaries");
    if !summaries.is_dir() || chapter <= 1 {
        return Ok(Vec::new());
    }
    let start = chapter.saturating_sub(limit as u32).max(1);
    let mut paths = Vec::new();
    for number in start..chapter {
        let path = summaries.join(format!("{number:03}.md"));
        if path.is_file() {
            paths.push(path);
        }
    }
    Ok(paths)
}

fn display_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
        .replace('\\', "/")
}

fn limit_chars(raw: &str, limit: usize) -> String {
    if raw.len() <= limit {
        return raw.to_string();
    }
    let mut end = limit.min(raw.len());
    while end > 0 && !raw.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n\n[...truncated...]", &raw[..end])
}

fn write_text(path: &Path, contents: &str, force: bool) -> Result<()> {
    if path.exists() && !force {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(path, contents).with_context(|| format!("failed to write {}", path.display()))
}

fn chapter_dir(root: &Path, chapter: u32) -> PathBuf {
    root.join("chapters").join(format!("{chapter:03}"))
}

fn collect_chapter_dirs(root: &Path) -> Result<Vec<(u32, PathBuf)>> {
    let mut chapters = Vec::new();
    let chapters_dir = root.join("chapters");
    if !chapters_dir.is_dir() {
        return Ok(chapters);
    }
    for entry in std::fs::read_dir(&chapters_dir)
        .with_context(|| format!("failed to read {}", chapters_dir.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if let Ok(number) = name.parse::<u32>() {
            chapters.push((number, entry.path()));
        }
    }
    chapters.sort_by_key(|(number, _)| *number);
    Ok(chapters)
}

pub(crate) fn chapter_diff_packet(workspace: &Path, chapter: u32) -> Result<String> {
    let root = find_project_root(workspace)?;
    let chapter_dir = chapter_dir(&root, chapter);
    let final_path = chapter_dir.join("final.md");
    let draft_path = chapter_dir.join("draft.md");
    let (old_label, old_text, new_label, new_text) = if final_path.is_file() {
        if let Some(version) = latest_chapter_version(&root, chapter, Some("final.md"))? {
            (
                display_relative(&root, &version.path),
                read_required(&version.path)?,
                display_relative(&root, &final_path),
                read_required(&final_path)?,
            )
        } else if draft_path.is_file() {
            (
                display_relative(&root, &draft_path),
                read_required(&draft_path)?,
                display_relative(&root, &final_path),
                read_required(&final_path)?,
            )
        } else {
            bail!("chapter {chapter:03} has final.md but no draft.md or saved final version");
        }
    } else {
        let Some(version) = latest_chapter_version(&root, chapter, None)? else {
            bail!("chapter {chapter:03} has no final.md and no saved versions");
        };
        (
            display_relative(&root, &version.path),
            read_required(&version.path)?,
            display_relative(&root, &draft_path),
            read_required(&draft_path)?,
        )
    };
    Ok(format_chapter_diff(
        chapter, &old_label, &old_text, &new_label, &new_text,
    ))
}

pub(crate) fn chapter_undo_from_workspace(workspace: &Path, chapter: u32) -> Result<String> {
    let root = find_project_root(workspace)?;
    chapter_undo(&root, chapter)
}

fn chapter_undo(root: &Path, chapter: u32) -> Result<String> {
    let version = latest_chapter_version(root, chapter, None)?
        .ok_or_else(|| anyhow!("chapter {chapter:03} has no saved versions to restore"))?;
    let chapter_dir = chapter_dir(root, chapter);
    let target = chapter_dir.join(&version.target);
    let current_backup = snapshot_chapter_file_before_write(root, chapter, &target, "before-undo")?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::copy(&version.path, &target).with_context(|| {
        format!(
            "failed to restore {} to {}",
            version.path.display(),
            target.display()
        )
    })?;
    let mut out = format!(
        "# Chapter Undo: {chapter:03}\n\nRestored `{}` from `{}`.",
        display_relative(root, &target),
        display_relative(root, &version.path)
    );
    if let Some(current_backup) = current_backup {
        out.push_str(&format!(
            "\nCurrent version was preserved at `{}`.",
            display_relative(root, &current_backup)
        ));
    }
    out.push('\n');
    Ok(out)
}

fn snapshot_chapter_file_before_write(
    root: &Path,
    chapter: u32,
    path: &Path,
    reason: &str,
) -> Result<Option<PathBuf>> {
    let Some(file_name) = path.file_name().and_then(OsStr::to_str) else {
        return Ok(None);
    };
    snapshot_chapter_source_for_target(root, chapter, path, file_name, reason)
}

fn snapshot_memory_summary_before_write(
    root: &Path,
    chapter: u32,
    path: &Path,
    reason: &str,
) -> Result<Option<PathBuf>> {
    snapshot_memory_artifact_before_write(root, chapter, path, reason)
}

fn snapshot_memory_artifact_before_write(
    root: &Path,
    chapter: u32,
    source: &Path,
    reason: &str,
) -> Result<Option<PathBuf>> {
    if !source.is_file() {
        return Ok(None);
    }
    let Some(file_name) = source.file_name().and_then(OsStr::to_str) else {
        return Ok(None);
    };
    let versions_dir = root.join("memory/.versions").join(format!("{chapter:03}"));
    std::fs::create_dir_all(&versions_dir)
        .with_context(|| format!("failed to create {}", versions_dir.display()))?;
    let timestamp = chapter_version_timestamp();
    let mut dest = versions_dir.join(format!(
        "{timestamp}-{reason}-{}",
        sanitize_version_target(file_name)
    ));
    let mut suffix = 1;
    while dest.exists() {
        dest = versions_dir.join(format!(
            "{timestamp}-{reason}-{suffix}-{}",
            sanitize_version_target(file_name)
        ));
        suffix += 1;
    }
    std::fs::copy(source, &dest).with_context(|| {
        format!(
            "failed to save memory artifact version {} -> {}",
            source.display(),
            dest.display()
        )
    })?;
    Ok(Some(dest))
}

fn snapshot_chapter_source_for_target(
    root: &Path,
    chapter: u32,
    source: &Path,
    target_file_name: &str,
    reason: &str,
) -> Result<Option<PathBuf>> {
    if !source.is_file() {
        return Ok(None);
    }
    let chapter_dir = chapter_dir(root, chapter);
    let versions_dir = chapter_dir.join(".versions");
    std::fs::create_dir_all(&versions_dir)
        .with_context(|| format!("failed to create {}", versions_dir.display()))?;
    let timestamp = chapter_version_timestamp();
    let mut dest = versions_dir.join(format!(
        "{timestamp}-{reason}-{}",
        sanitize_version_target(target_file_name)
    ));
    let mut suffix = 1;
    while dest.exists() {
        dest = versions_dir.join(format!(
            "{timestamp}-{reason}-{suffix}-{}",
            sanitize_version_target(target_file_name)
        ));
        suffix += 1;
    }
    std::fs::copy(source, &dest).with_context(|| {
        format!(
            "failed to save chapter version {} -> {}",
            source.display(),
            dest.display()
        )
    })?;
    Ok(Some(dest))
}

#[derive(Debug, Clone)]
struct ChapterVersion {
    path: PathBuf,
    target: String,
    modified: std::time::SystemTime,
}

fn latest_chapter_version(
    root: &Path,
    chapter: u32,
    target: Option<&str>,
) -> Result<Option<ChapterVersion>> {
    let versions_dir = chapter_dir(root, chapter).join(".versions");
    if !versions_dir.is_dir() {
        return Ok(None);
    }
    let mut versions = Vec::new();
    for entry in std::fs::read_dir(&versions_dir)
        .with_context(|| format!("failed to read {}", versions_dir.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(OsStr::to_str) else {
            continue;
        };
        let Some(version_target) = chapter_version_target(file_name).map(ToString::to_string)
        else {
            continue;
        };
        if target.is_some_and(|target| target != version_target) {
            continue;
        }
        versions.push(ChapterVersion {
            modified: entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
            path,
            target: version_target,
        });
    }
    versions.sort_by(|left, right| {
        let left_name = left
            .path
            .file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_default();
        let right_name = right
            .path
            .file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_default();
        left.modified
            .cmp(&right.modified)
            .then_with(|| left_name.cmp(&right_name))
    });
    Ok(versions.pop())
}

fn chapter_version_target(file_name: &str) -> Option<&str> {
    if file_name.ends_with("-final.md") {
        Some("final.md")
    } else if file_name.ends_with("-draft.md") {
        Some("draft.md")
    } else {
        None
    }
}

fn chapter_version_timestamp() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}

fn sanitize_version_target(target: &str) -> String {
    match target {
        "draft.md" | "final.md" => target.to_string(),
        other => sanitize_file_name(other),
    }
}

fn format_chapter_diff(
    chapter: u32,
    old_label: &str,
    old_text: &str,
    new_label: &str,
    new_text: &str,
) -> String {
    let diff = TextDiff::from_lines(old_text, new_text);
    let mut inserted = 0usize;
    let mut deleted = 0usize;
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Insert => inserted += 1,
            ChangeTag::Delete => deleted += 1,
            ChangeTag::Equal => {}
        }
    }
    let mut out = format!(
        "# Chapter Diff: {chapter:03}\n\nComparing `{old_label}` -> `{new_label}`\n\nLines: +{inserted} -{deleted}\n\nVerification:\n- Read this diff for accidental prose loss, continuity regressions, and missing payoff/foreshadowing carry-through.\n- If this revision should stand, run `deepseek remember {chapter}` to refresh chapter memory before continuing.\n- If it is wrong, run `deepseek undo {chapter}` to restore the latest saved chapter version.\n\n```diff\n--- {old_label}\n+++ {new_label}\n"
    );
    for change in diff.iter_all_changes() {
        let sign = match change.tag() {
            ChangeTag::Delete => "-",
            ChangeTag::Insert => "+",
            ChangeTag::Equal => " ",
        };
        out.push_str(sign);
        out.push_str(change.value());
        if !change.value().ends_with('\n') {
            out.push('\n');
        }
    }
    out.push_str("```\n");
    out
}

fn sanitize_file_name(input: &str) -> String {
    let mut out = String::new();
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric()
            || matches!(ch, '-' | '_' | '.')
            || ('\u{4e00}'..='\u{9fff}').contains(&ch)
        {
            out.push(ch);
        } else if ch.is_whitespace() {
            out.push('_');
        }
    }
    if out.is_empty() {
        "novel".to_string()
    } else {
        out
    }
}

#[allow(clippy::items_after_test_module)]
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn init_project_creates_long_form_assets() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("测试长篇".to_string()),
            Some("都市重生".to_string()),
            Some("失败者回到十年前".to_string()),
            500_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");

        for path in [
            "book.toml",
            "bible/premise.md",
            "bible/reader_promise.md",
            "craft/human_texture.md",
            "craft/anti_ai_patterns.md",
            "cards/characters/_template.yaml",
            "cards/resources/_template.yaml",
            "materials/README.md",
            "outline/chapter_index.md",
            "memory/SCHEMA.md",
            "memory/graph.schema.json",
            "memory/facts.jsonl",
            "memory/events.jsonl",
            "memory/foreshadowing.jsonl",
            "memory/behavior.jsonl",
            "eval/README.md",
            "eval/rubrics/quality_signals.md",
            "eval/failures/resource_without_cost",
            "experiments/README.md",
            "experiments/baselines/long_form_acceptance.md",
            "experiments/configs",
            "experiments/runs",
            "leaderboard/README.md",
        ] {
            assert!(tmp.path().join(path).exists(), "missing {path}");
        }

        let manifest = load_manifest(tmp.path()).expect("manifest");
        assert_eq!(manifest.title, "测试长篇");
        assert_eq!(manifest.genre, "都市重生");
        assert_eq!(manifest.target_words, 500_000);
        let materials_readme =
            std::fs::read_to_string(tmp.path().join("materials/README.md")).expect("materials");
        assert!(materials_readme.contains("素材是参考来源，不是作品 canon"));
        let schema = std::fs::read_to_string(tmp.path().join("memory/SCHEMA.md")).expect("schema");
        assert!(schema.contains("schema_version"));
        assert!(schema.contains("DOES_NOT_KNOW"));
        assert!(schema.contains("Character Behavior Ledger"));
        let json_schema =
            std::fs::read_to_string(tmp.path().join("memory/graph.schema.json")).expect("schema");
        assert!(json_schema.contains("\"$schema\""));
        assert!(json_schema.contains("\"schema_version\""));
        let resource_template =
            std::fs::read_to_string(tmp.path().join("cards/resources/_template.yaml"))
                .expect("resource template");
        assert!(resource_template.contains("ordinary_income_equivalent"));
        assert!(resource_template.contains("debt_or_obligation"));
        let eval_readme =
            std::fs::read_to_string(tmp.path().join("eval/README.md")).expect("eval readme");
        assert!(eval_readme.contains("does not prove real reader quality"));
        let experiments_readme = std::fs::read_to_string(tmp.path().join("experiments/README.md"))
            .expect("experiments readme");
        assert!(experiments_readme.contains("after development is complete"));
        let baseline = std::fs::read_to_string(
            tmp.path()
                .join("experiments/baselines/long_form_acceptance.md"),
        )
        .expect("long-form baseline");
        assert!(baseline.contains("50 chapters"));
        assert!(baseline.contains("3500 target words"));
    }

    #[test]
    fn export_book_orders_chapters_and_prefers_finals() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("导出测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("chapters/002/draft.md"),
            "# 第 002 章\n\n草稿二",
            true,
        )
        .expect("write draft");
        write_text(
            &tmp.path().join("chapters/001/draft.md"),
            "# 第 001 章\n\n草稿一",
            true,
        )
        .expect("write draft");
        write_text(
            &tmp.path().join("chapters/001/final.md"),
            "# 第 001 章\n\n终稿一",
            true,
        )
        .expect("write final");

        let output = tmp.path().join("exports/book.txt");
        export_book(tmp.path(), Some(output.clone()), ExportFormat::Txt).expect("export");
        let exported = std::fs::read_to_string(output).expect("export text");

        let first = exported.find("终稿一").expect("chapter 1");
        let second = exported.find("草稿二").expect("chapter 2");
        assert!(first < second, "chapters should be ordered: {exported}");
        assert!(
            !exported.contains("草稿一"),
            "final should replace draft: {exported}"
        );
    }

    #[test]
    fn memory_summary_and_candidate_overwrites_are_versioned() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("记忆备份测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        let summary_path = tmp.path().join("memory/summaries/001.md");
        write_text(&summary_path, "# 第 001 章记忆\n\n旧摘要", true).expect("write summary");

        let summary_backup =
            snapshot_memory_summary_before_write(tmp.path(), 1, &summary_path, "pre-remember")
                .expect("backup")
                .expect("summary backup");
        write_text(&summary_path, "# 第 001 章记忆\n\n新摘要", true).expect("rewrite summary");
        let saved_summary = std::fs::read_to_string(summary_backup).expect("saved summary");
        assert!(saved_summary.contains("旧摘要"));

        let source = tmp.path().join("chapters/001/audit.md");
        write_text(
            &source,
            "## CANDIDATE_MEMORY_UPDATES\n{\"chapter\":1,\"kind\":\"knowledge\",\"target\":\"林墨\",\"change\":\"知道旧案\",\"evidence\":\"audit\",\"confidence\":0.8}\n",
            true,
        )
        .expect("write audit");
        let audit = read_required(&source).expect("audit");
        write_memory_candidate_file(tmp.path(), 1, &source, &audit).expect("write candidates");
        let candidate_path = tmp.path().join("memory/candidates/001.json");
        write_text(&candidate_path, "{\"old\":true}", true).expect("old candidate");
        write_memory_candidate_file(tmp.path(), 1, &source, &audit).expect("rewrite candidates");
        let backups = std::fs::read_dir(tmp.path().join("memory/.versions/001"))
            .expect("versions")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert!(
            backups.iter().any(|name| name.contains("pre-candidates")),
            "candidate overwrite should be versioned: {backups:?}"
        );
    }

    #[test]
    fn writing_context_includes_recent_summaries_before_chapters() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("上下文测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("memory/summaries/001.md"),
            "第一章摘要",
            true,
        )
        .expect("write summary");
        write_text(
            &tmp.path().join("chapters/002/craft_plan.md"),
            "第二章人味计划",
            true,
        )
        .expect("write craft plan");
        write_text(
            &tmp.path().join("chapters/001/final.md"),
            "# 第 001 章\n\n正文",
            true,
        )
        .expect("write final");

        let context = writing_context(tmp.path(), 2).expect("context");

        assert!(context.contains("memory/summaries/001.md"));
        assert!(context.contains("第一章摘要"));
        assert!(context.contains("Current chapter craft plan"));
        assert!(context.contains("第二章人味计划"));
        assert!(context.contains("Recent chapter 001"));
    }

    #[test]
    fn writing_context_includes_chapter_bridge_continuity_packet() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("跨章衔接测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("chapters/001/final.md"),
            "# 第 001 章\n\n林墨把账册塞进鞋底。\n\n苏棠站在禁厄牢门边，低声说：“天亮前，追兵会到。”\n\n他右手还在流血，草环贴着掌心发硬。",
            true,
        )
        .expect("write chapter 1");
        write_text(
            &tmp.path().join("chapters/002/draft.md"),
            "# 第 002 章\n\n林墨醒来时，右手的血已经粘住袖口。",
            true,
        )
        .expect("write chapter 2");

        let context = writing_context(tmp.path(), 2).expect("context");

        assert!(context.contains("## Chapter bridge continuity"));
        assert!(context.contains("previous_chapter: 001"));
        assert!(context.contains("草环"));
        assert!(context.contains("追兵"));
        assert!(context.contains("Previous ending excerpt"));
        assert!(context.contains("Current opening excerpt"));
    }

    #[test]
    fn writing_context_compacts_recent_memory_summaries_for_late_context() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("摘要压缩测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        let noisy_feedback = "这是一条写法反馈噪声。".repeat(240);
        let noisy_candidates = "{\"chapter\":1,\"kind\":\"knowledge\",\"target\":\"噪声\",\"change\":\"不应进入上下文\",\"confidence\":0.9}\n".repeat(40);
        write_text(
            &tmp.path().join("memory/summaries/001.md"),
            &format!(
                "# 第 001 章记忆\n\n\
                 ## 章节摘要\n{}\n\n\
                 ## 人物知识边界\n- 林墨：本章新知道陈岚隐瞒监控剪辑。\n- 陈岚：仍不知道林墨已经藏起备份。\n\n\
                 ## 新增事实锁\n- 事实锁：监控硬盘在林墨手里，证据见 chapters/001/final.md:8。\n\n\
                 ## 承诺推进\n- P-accident-001：事故真相从被动疑云推进为主动追查。\n\n\
                 ## 资源变化\n- 监控硬盘：控制权转到林墨，代价是被保安盯上。\n\n\
                 ## 地点与世界状态\n- 废仓库：已被陈岚清理，公开线索减少。\n\n\
                 ## 伏笔台账\n- F-accident-disk：推进，硬盘仍未公开。\n\n\
                 ## 写法反馈\n{}\n\n\
                 ## CANDIDATE_MEMORY_UPDATES\n{}",
                "长摘要。".repeat(500),
                noisy_feedback,
                noisy_candidates
            ),
            true,
        )
        .expect("write summary");
        write_text(
            &tmp.path().join("chapters/001/final.md"),
            "# 第 001 章\n\n正文",
            true,
        )
        .expect("write final");

        let context = writing_context(tmp.path(), 2).expect("context");

        assert!(context.contains("compact memory summary"));
        assert!(context.contains("林墨：本章新知道陈岚隐瞒监控剪辑"));
        assert!(context.contains("监控硬盘在林墨手里"));
        assert!(context.contains("P-accident-001"));
        assert!(!context.contains("写法反馈噪声"));
        assert!(!context.contains("不应进入上下文"));
        assert!(!context.contains("CANDIDATE_MEMORY_UPDATES"));
        let compact = compact_memory_summary_for_context(
            &read_required(&tmp.path().join("memory/summaries/001.md")).expect("summary"),
        );
        assert!(
            compact.len() <= MEMORY_SUMMARY_CONTEXT_LIMIT + "[...truncated...]".len() + 4,
            "summary packet should be compact: {}",
            compact.len()
        );
    }

    #[test]
    fn project_context_surfaces_canon_priority_and_empty_source_warnings() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("Canon 缺口测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("bible/world.md"),
            "# 世界设定\n\n## 规则\n",
            true,
        )
        .expect("write world");
        write_text(
            &tmp.path().join("bible/reader_promise.md"),
            "# 读者承诺\n\n## 爽点\n",
            true,
        )
        .expect("write promise");

        let context = project_context(tmp.path()).expect("context");

        assert!(context.contains("Canon Source Priority"));
        assert!(
            context.contains("outline/master_plan.md` and `outline/chapter_index.md` are plans")
        );
        assert!(context.contains("bible/world.md` has no substantive canon"));
        assert!(context.contains("cards/characters` has no non-template character cards"));
    }

    #[test]
    fn chapter_versions_preserve_final_before_overwrite() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("版本保护测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("chapters/001/final.md"),
            "# 第 001 章\n\n旧终稿",
            true,
        )
        .expect("write final");

        let backup = snapshot_chapter_file_before_write(
            tmp.path(),
            1,
            &tmp.path().join("chapters/001/final.md"),
            "pre-revise",
        )
        .expect("snapshot")
        .expect("backup");

        assert_eq!(
            backup
                .parent()
                .and_then(Path::file_name)
                .and_then(OsStr::to_str),
            Some(".versions")
        );
        assert_eq!(
            backup
                .parent()
                .and_then(Path::parent)
                .and_then(Path::file_name)
                .and_then(OsStr::to_str),
            Some("001")
        );
        assert!(backup.to_string_lossy().contains("pre-revise-final.md"));
        let saved = std::fs::read_to_string(backup).expect("saved backup");
        assert!(saved.contains("旧终稿"));
    }

    #[test]
    fn chapter_diff_compares_draft_to_final_without_git() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("差异测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("chapters/001/draft.md"),
            "# 第 001 章\n\n旧句子\n保留句",
            true,
        )
        .expect("write draft");
        write_text(
            &tmp.path().join("chapters/001/final.md"),
            "# 第 001 章\n\n新句子\n保留句",
            true,
        )
        .expect("write final");

        let diff = chapter_diff_packet(tmp.path(), 1).expect("diff");

        assert!(diff.contains("# Chapter Diff: 001"));
        assert!(diff.contains("-旧句子"));
        assert!(diff.contains("+新句子"));
        assert!(diff.contains("Lines: +1 -1"));
        assert!(diff.contains("Verification:"));
        assert!(diff.contains("deepseek remember 1"));
        assert!(diff.contains("deepseek undo 1"));
    }

    #[test]
    fn chapter_undo_restores_latest_version_and_preserves_current() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("回滚测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        let final_path = tmp.path().join("chapters/001/final.md");
        write_text(&final_path, "# 第 001 章\n\n旧终稿", true).expect("write old final");
        snapshot_chapter_file_before_write(tmp.path(), 1, &final_path, "pre-revise")
            .expect("snapshot old");
        write_text(&final_path, "# 第 001 章\n\n坏终稿", true).expect("write bad final");

        let report = chapter_undo(tmp.path(), 1).expect("undo");

        assert!(report.contains("Restored `chapters/001/final.md`"));
        let restored = std::fs::read_to_string(&final_path).expect("restored final");
        assert!(restored.contains("旧终稿"));
        let backups: Vec<_> = std::fs::read_dir(tmp.path().join("chapters/001/.versions"))
            .expect("versions dir")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();
        assert!(
            backups
                .iter()
                .any(|name| name.contains("before-undo-final.md")),
            "current final should be preserved: {backups:?}"
        );
    }

    #[test]
    fn project_context_includes_structured_card_assets() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("卡片测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("cards/characters/hero.yaml"),
            "name: 林墨\nstate: 隐瞒古剑来历\n",
            true,
        )
        .expect("write character card");
        write_text(
            &tmp.path().join("cards/world/sword_rules.toml"),
            "rule = \"古剑每次出鞘都要付出代价\"\n",
            true,
        )
        .expect("write world card");
        write_text(
            &tmp.path().join("cards/resources/spirit_stone.yaml"),
            "id: spirit_stone\nname: 灵石\nmarket_value: 一枚下品灵石\nordinary_income_equivalent: 外门弟子三日俸禄\nwho_controls_it: 青岚宗库房\ncost_to_use: 消耗后不可复原\n",
            true,
        )
        .expect("write resource card");

        let context = project_context(tmp.path()).expect("context");

        assert!(context.contains("cards/characters/hero.yaml"));
        assert!(context.contains("隐瞒古剑来历"));
        assert!(context.contains("cards/world/sword_rules.toml"));
        assert!(context.contains("每次出鞘都要付出代价"));
        assert!(context.contains("cards/resources/spirit_stone.yaml"));
        assert!(context.contains("外门弟子三日俸禄"));
    }

    #[test]
    fn writing_context_includes_local_materials_as_non_canon_references() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("素材上下文测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("materials/sources/city_fire.md"),
            "# 城市火灾资料\n\n旧城消防通道常被临停车辆堵塞。\n",
            true,
        )
        .expect("write material");

        let context = writing_context(tmp.path(), 1).expect("context");

        assert!(context.contains("Local materials boundary"));
        assert!(context.contains("materials/sources/city_fire.md"));
        assert!(context.contains("旧城消防通道常被临停车辆堵塞"));
        assert!(context.contains("They are not canon"));
        assert!(context.contains("must not override `book.toml`, `bible/`, `cards/`"));
    }

    #[test]
    fn audit_risk_summary_extracts_affected_nodes_from_section_only() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("影响节点测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("chapters/001/audit.md"),
            "## BLOCKER\n{\"characters\":[\"不应计入\"]}\n- 人物知识边界泄漏\n\n## AFFECTED_NODES\n- character:lin_mo\n{\"promises\":[\"事故真相\"],\"chapters\":[\"chapters/002\"]}\n\n## CANDIDATE_MEMORY_UPDATES\n{\"chapter\":1,\"kind\":\"knowledge\",\"target\":\"林墨\",\"change\":\"发现事故线索\",\"confidence\":0.9,\"affects\":[\"promise:accident\"]}\n",
            true,
        )
        .expect("write audit");

        let summary = audit_risk_summary(tmp.path(), 8).expect("summary");

        assert_eq!(summary.blockers, 2);
        assert_eq!(summary.majors, 0);
        assert_eq!(summary.affected_nodes, 3);
        assert!(
            summary
                .affected_previews
                .iter()
                .any(|item| item.contains("character:lin_mo"))
        );
        assert!(
            summary
                .affected_previews
                .iter()
                .any(|item| item.contains("事故真相"))
        );
        assert!(
            !summary
                .affected_previews
                .iter()
                .any(|item| item.contains("不应计入"))
        );
    }

    #[test]
    fn audit_risk_summary_groups_layered_audit_categories() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("分层风险".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("chapters/001/audit.md"),
            "## CONTINUITY_AUDIT\n- 人物不知道丹药来源却直接说出密令\n\n## CRAFT_AUDIT\n- 战斗只有剑气堆叠，没有观察链\n\n## MEMORY_CANDIDATE_AUDIT\n- none\n\n## READER_PROMISE_AUDIT\n- 断剑来历没有推进\n\n## BLOCKER\n- 人物知识边界泄漏\n",
            true,
        )
        .expect("write audit");

        let summary = audit_risk_summary(tmp.path(), 8).expect("summary");

        assert_eq!(summary.blockers, 1);
        assert!(
            summary
                .layered_counts
                .iter()
                .any(|(kind, count)| kind == "continuity" && *count == 1)
        );
        assert!(
            summary
                .layered_counts
                .iter()
                .any(|(kind, count)| kind == "craft" && *count == 1)
        );
        assert!(
            summary
                .layered_counts
                .iter()
                .any(|(kind, count)| kind == "reader_promise" && *count == 1)
        );
        assert!(
            !summary
                .layered_counts
                .iter()
                .any(|(kind, _)| kind == "memory_candidate")
        );
        assert!(
            summary
                .layered_previews
                .iter()
                .any(|item| item.contains("chapter 001 continuity"))
        );
        assert_eq!(
            format_audit_layer_counts(&summary.layered_counts),
            "continuity=1, craft=1, reader_promise=1"
        );
    }

    #[test]
    fn project_map_reports_assets_chapters_cards_and_memory() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("地图测试".to_string()),
            Some("悬疑".to_string()),
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("cards/characters/lin_mo.yaml"),
            "name: 林墨\nstate: 隐瞒重生记忆\n",
            true,
        )
        .expect("write character card");
        write_text(
            &tmp.path().join("cards/locations/warehouse.md"),
            "# 废仓库\n\n事故线索第一次出现的地点。\n",
            true,
        )
        .expect("write location card");
        write_text(
            &tmp.path().join("materials/notes/police_process.md"),
            "# 走访资料\n\n报警记录只能作为参考，不能覆盖角色卡和记忆图。\n",
            true,
        )
        .expect("write material");
        write_text(
            &tmp.path().join("chapters/001/brief.md"),
            "# 第 001 章简报\n",
            true,
        )
        .expect("write brief");
        write_text(
            &tmp.path().join("chapters/001/draft.md"),
            "# 第 001 章\n\n草稿",
            true,
        )
        .expect("write draft");
        write_text(
            &tmp.path().join("chapters/001/final.md"),
            "# 第 001 章\n\n终稿",
            true,
        )
        .expect("write final");
        write_text(
            &tmp.path().join("chapters/001/.versions/20260101-final.md"),
            "旧终稿",
            true,
        )
        .expect("write version");
        write_text(
            &tmp.path().join("memory/summaries/001.md"),
            "林墨发现事故线索。",
            true,
        )
        .expect("write summary");
        write_text(
            &tmp.path()
                .join("memory/reports/20260514T000000Z-analysis.md"),
            "# Imported RLM Manuscript Analysis\n\n时间线风险。",
            true,
        )
        .expect("write report");
        write_text(
            &tmp.path().join("memory/foreshadowing.jsonl"),
            "{\"promise\":\"事故真相\",\"status\":\"new\",\"first_chapter\":1}\n",
            true,
        )
        .expect("write foreshadowing");
        write_text(
            &tmp.path().join("memory/candidates/001.json"),
            r#"{"chapter":1,"candidates":[{"kind":"knowledge","target":"林墨","change":"发现事故线索","evidence":"chapters/001/final.md","confidence":0.9,"affects":["promise:accident"],"status":"candidate"}]}"#,
            true,
        )
        .expect("write candidates");
        let graph = build_memory_graph(tmp.path()).expect("graph");
        save_memory_graph(tmp.path(), &graph).expect("save graph");

        let map = project_map_packet(tmp.path()).expect("project map");

        assert!(map.contains("# Novel Project Map"));
        assert!(map.contains("Title: 地图测试"));
        assert!(map.contains("Workflow gate:"));
        assert!(map.contains("Next action:"));
        assert!(map.contains("Context score:"));
        assert!(map.contains("cards/characters/lin_mo.yaml"));
        assert!(map.contains("cards/locations/warehouse.md"));
        assert!(map.contains("## Local Materials"));
        assert!(map.contains("materials/notes/police_process.md"));
        assert!(map.contains("reference sources only"));
        assert!(map.contains("Chapter 001"));
        assert!(map.contains("brief"));
        assert!(map.contains("final"));
        assert!(map.contains("versions:1"));
        assert!(map.contains("summary"));
        assert!(map.contains("Schema: ok: v2"));
        assert!(map.contains("Reports"));
        assert!(map.contains("memory/reports/20260514T000000Z-analysis.md"));
        assert!(map.contains("Candidate updates: 1"));
        assert!(map.contains("Memory pressure:"));
        assert!(map.contains("Summary density:"));
        assert!(map.contains("事故真相 | new | chapter 001"));
    }

    #[test]
    fn project_map_compacts_late_book_by_default_and_full_lists_all() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("后期地图测试".to_string()),
            Some("长篇悬疑".to_string()),
            None,
            500_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        for chapter in 1..=30 {
            write_text(
                &tmp.path().join(format!("chapters/{chapter:03}/draft.md")),
                "草稿",
                true,
            )
            .expect("write draft");
            write_text(
                &tmp.path().join(format!("chapters/{chapter:03}/final.md")),
                "终稿",
                true,
            )
            .expect("write final");
            write_text(
                &tmp.path().join(format!("chapters/{chapter:03}/audit.md")),
                "审稿",
                true,
            )
            .expect("write audit");
            write_text(
                &tmp.path().join(format!("memory/summaries/{chapter:03}.md")),
                "记忆摘要",
                true,
            )
            .expect("write summary");
        }

        let compact = project_map_packet(tmp.path()).expect("compact map");
        let full = project_map_packet_with_options(tmp.path(), true).expect("full map");

        assert!(compact.contains("Compact view"));
        assert!(compact.contains("Workflow gate:"));
        assert!(compact.contains("Memory pressure:"));
        assert!(compact.contains("Chapter 030"));
        assert!(compact.contains("Chapter 007"));
        assert!(!compact.contains("Chapter 001"));
        assert!(compact.contains("Recent summaries: 30 total"));
        assert!(compact.contains("memory/summaries/030.md"));
        assert!(!compact.contains("memory/summaries/001.md"));
        assert!(full.contains("Chapter 001"));
        assert!(full.contains("memory/summaries/001.md"));
        assert!(!full.contains("Compact view"));
    }

    #[test]
    fn memory_query_uses_alias_index_for_entities_and_promise_ids() {
        let graph = MemoryGraph {
            schema_version: 2,
            updated_at: "2026-05-14T00:00:00Z".to_string(),
            nodes: vec![
                MemoryNode {
                    id: "character:lin_mo".to_string(),
                    kind: "character".to_string(),
                    label: "林墨".to_string(),
                    source: "cards/characters/lin_mo.yaml".to_string(),
                    summary: "主角".to_string(),
                    state: serde_json::json!({ "aliases": ["旧案人"] }),
                    hash: "hash".to_string(),
                },
                MemoryNode {
                    id: "object:drive".to_string(),
                    kind: "object".to_string(),
                    label: "监控硬盘".to_string(),
                    source: "cards/resources/drive.yaml".to_string(),
                    summary: "关键物证".to_string(),
                    state: serde_json::json!({ "aka": "黑匣子" }),
                    hash: "hash".to_string(),
                },
                MemoryNode {
                    id: "location:warehouse".to_string(),
                    kind: "location".to_string(),
                    label: "废仓库".to_string(),
                    source: "cards/locations/warehouse.md".to_string(),
                    summary: "事故线索地点".to_string(),
                    state: serde_json::json!({ "short_name": "西库" }),
                    hash: "hash".to_string(),
                },
                MemoryNode {
                    id: "promise:accident".to_string(),
                    kind: "promise".to_string(),
                    label: "事故真相".to_string(),
                    source: "memory/foreshadowing.jsonl:1".to_string(),
                    summary: "主线伏笔".to_string(),
                    state: serde_json::json!({
                        "promise_id": "P-accident-001",
                        "foreshadowing_id": "F-accident"
                    }),
                    hash: "hash".to_string(),
                },
            ],
            edges: Vec::new(),
            candidate_updates: Vec::new(),
        };

        assert_eq!(
            find_memory_seed_nodes(&graph, "旧案人"),
            vec!["character:lin_mo".to_string()]
        );
        assert_eq!(
            find_memory_seed_nodes(&graph, "黑匣子"),
            vec!["object:drive".to_string()]
        );
        assert_eq!(
            find_memory_seed_nodes(&graph, "西库"),
            vec!["location:warehouse".to_string()]
        );
        assert_eq!(
            find_memory_seed_nodes(&graph, "P-accident-001"),
            vec!["promise:accident".to_string()]
        );
        assert_eq!(
            find_memory_seed_nodes(&graph, "F-accident"),
            vec!["promise:accident".to_string()]
        );
    }

    #[test]
    fn memory_neighborhood_prioritizes_weighted_recent_high_confidence_edges() {
        let graph = MemoryGraph {
            schema_version: 2,
            updated_at: "2026-05-14T00:00:00Z".to_string(),
            nodes: vec![
                MemoryNode {
                    id: "character:lin_mo".to_string(),
                    kind: "character".to_string(),
                    label: "林墨".to_string(),
                    source: "cards/characters/lin_mo.yaml".to_string(),
                    summary: "主角".to_string(),
                    state: serde_json::Value::Null,
                    hash: "hash".to_string(),
                },
                MemoryNode {
                    id: "chapter:001".to_string(),
                    kind: "chapter".to_string(),
                    label: "Chapter 001".to_string(),
                    source: "chapters/001".to_string(),
                    summary: "早期章节".to_string(),
                    state: serde_json::json!({ "number": 1 }),
                    hash: "hash".to_string(),
                },
                MemoryNode {
                    id: "summary:030".to_string(),
                    kind: "memory".to_string(),
                    label: "Chapter 030 Summary".to_string(),
                    source: "memory/summaries/030.md".to_string(),
                    summary: "最近摘要".to_string(),
                    state: serde_json::Value::Null,
                    hash: "hash".to_string(),
                },
            ],
            edges: vec![
                MemoryEdge {
                    kind: "NEXT".to_string(),
                    source: "character:lin_mo".to_string(),
                    target: "chapter:001".to_string(),
                    evidence: "chapters/001/final.md".to_string(),
                    confidence: 1.0,
                    note: None,
                },
                MemoryEdge {
                    kind: "SUMMARIZED_BY".to_string(),
                    source: "character:lin_mo".to_string(),
                    target: "summary:030".to_string(),
                    evidence: "chapters/030/final.md".to_string(),
                    confidence: 0.9,
                    note: None,
                },
            ],
            candidate_updates: Vec::new(),
        };

        let neighborhood = memory_neighborhood(&graph, &["character:lin_mo".to_string()], 1, 2);
        let ids = neighborhood
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["character:lin_mo", "summary:030"]);
    }

    #[test]
    fn memory_source_fingerprint_uses_shallow_cache_entries() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("浅层指纹测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("memory/candidates/deep/a/b/too_deep.json"),
            "{}",
            true,
        )
        .expect("write deep candidate");
        let graph = build_memory_graph(tmp.path()).expect("graph");
        save_memory_graph(tmp.path(), &graph).expect("save graph");

        let graph_path = memory_graph_path(tmp.path());
        let _ = memory_graph_is_stale(tmp.path(), &graph_path);
        let cache_key = tmp
            .path()
            .canonicalize()
            .unwrap_or_else(|_| tmp.path().to_path_buf());
        let cache = MEMORY_SOURCE_FINGERPRINT_CACHE.get().expect("cache");
        let guard = cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let cached = guard.get(&cache_key).expect("cached fingerprint");
        let paths = cached
            .fingerprint
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>();

        assert_eq!(cached.graph_modified, file_modified(&graph_path));
        assert!(cached.checked_at.elapsed() <= MEMORY_SOURCE_FINGERPRINT_TTL);
        assert!(paths.contains(&"memory/candidates/deep/a"));
        assert!(!paths.contains(&"memory/candidates/deep/a/b/too_deep.json"));
    }

    #[test]
    fn chapter_memory_summary_prompt_requires_precision_sections() {
        let prompt = chapter_memory_summary_prompt(7, "项目上下文", "章节正文");

        for heading in [
            "## 章节摘要",
            "## 人物状态变化",
            "## 人物知识边界",
            "## 新增事实锁",
            "## 事件时间线",
            "## 承诺推进",
            "## 资源变化",
            "## 地点与世界状态",
            "## 伏笔台账",
            "## 人物体验沉淀",
            "## 写法反馈",
            "## 后续风险",
            "## CANDIDATE_MEMORY_UPDATES",
        ] {
            assert!(prompt.contains(heading), "missing heading: {heading}");
        }
        assert!(prompt.contains("本章前知道什么"));
        assert!(prompt.contains("稳定 id"));
        assert!(prompt.contains("已完成章节和 memory 为准"));
        assert!(prompt.contains("审稿建议"));
        assert!(prompt.contains("全文目标 1500-2500 个中文字符"));
        assert!(prompt.contains("每个分栏最多 3-5 条要点"));
        assert!(prompt.contains("最多 3-6 条"));
        assert!(prompt.contains(
            "\"kind\":\"knowledge|relationship|promise|event|location_state|object_state|memory\""
        ));
    }

    #[test]
    fn memory_graph_links_chapters_to_structured_assets() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("记忆图测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("cards/characters/lin_mo.yaml"),
            "name: 林墨\nstate: 隐瞒古剑来历\n",
            true,
        )
        .expect("write character");
        write_text(
            &tmp.path().join("cards/world/sword.md"),
            "# 古剑\n\n古剑每次出鞘都要付出代价。\n",
            true,
        )
        .expect("write world");
        write_text(
            &tmp.path().join("chapters/001/final.md"),
            "# 第 001 章\n\n林墨握住古剑，没有解释它的来历。",
            true,
        )
        .expect("write chapter");
        write_text(
            &tmp.path().join("memory/summaries/001.md"),
            "林墨继续隐瞒古剑来历。",
            true,
        )
        .expect("write memory summary");

        let graph = build_memory_graph(tmp.path()).expect("graph");

        assert!(graph.nodes.iter().any(|node| node.id == "chapter:001"));
        assert!(
            graph
                .nodes
                .iter()
                .any(|node| node.kind == "character" && node.label == "林墨")
        );
        assert!(
            !graph
                .nodes
                .iter()
                .any(|node| node.kind == "character" && node.label == "人物名")
        );
        assert!(graph.edges.iter().any(|edge| edge.kind == "MENTIONS"
            && edge.source == "chapter:001"
            && edge.target.contains("lin_mo")));
    }

    #[test]
    fn memory_graph_caps_textual_mentions_to_story_entities() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("提及边压缩".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        let mut chapter_text = String::from("# 第 001 章\n\n");
        for index in 1..=30 {
            let name = format!("角{index}");
            write_text(
                &tmp.path()
                    .join(format!("cards/characters/character_{index:02}.yaml")),
                &format!("name: {name}\naliases:\n  - 别{index}\n"),
                true,
            )
            .expect("write character");
            chapter_text.push_str(&name);
            chapter_text.push(' ');
        }
        write_text(
            &tmp.path().join("memory/facts.jsonl"),
            "{\"kind\":\"knowledge\",\"target\":\"角1\",\"change\":\"角1知道密令\",\"chapter\":1}\n",
            true,
        )
        .expect("write knowledge");
        write_text(
            &tmp.path().join("chapters/001/final.md"),
            &chapter_text,
            true,
        )
        .expect("write chapter");

        let graph = build_memory_graph(tmp.path()).expect("graph");
        let node_kinds = graph
            .nodes
            .iter()
            .map(|node| (node.id.as_str(), node.kind.as_str()))
            .collect::<BTreeMap<_, _>>();
        let chapter_mentions = graph
            .edges
            .iter()
            .filter(|edge| edge.kind == "MENTIONS" && edge.source == "chapter:001")
            .collect::<Vec<_>>();

        assert_eq!(chapter_mentions.len(), 16);
        assert!(chapter_mentions.iter().all(|edge| {
            matches!(
                node_kinds.get(edge.target.as_str()).copied(),
                Some("character" | "location" | "object" | "promise" | "world")
            )
        }));
    }

    #[test]
    fn memory_graph_schema_v2_tracks_narrative_semantics() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("语义图测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("cards/characters/lin_mo.yaml"),
            "name: 林墨\nwant: 查清事故真相\nfear: 父亲死亡\nsecret: 重生记忆\nknowledge:\n  - 陈岚说谎\nunknown:\n  - 父亲仍活着\nrelationships:\n  - 陈岚: 互相试探\nstate: 隐瞒重生记忆\n",
            true,
        )
        .expect("write character");
        write_text(
            &tmp.path().join("memory/events.jsonl"),
            "{\"kind\":\"event\",\"event\":\"林墨发现事故监控被剪掉\",\"chapter\":1}\n",
            true,
        )
        .expect("write events");
        write_text(
            &tmp.path().join("memory/foreshadowing.jsonl"),
            "{\"kind\":\"promise\",\"promise\":\"事故真相\",\"status\":\"new\",\"first_chapter\":1}\n",
            true,
        )
        .expect("write promises");
        write_text(
            &tmp.path().join("chapters/001/audit.md"),
            "## CANDIDATE_MEMORY_UPDATES\n- 候选记忆更新：target: 林墨 change: 确认陈岚说谎 confidence 0.91 affects: character:chen_lan,promise:accident_truth\n",
            true,
        )
        .expect("write audit");

        let graph = build_memory_graph(tmp.path()).expect("graph");

        assert_eq!(graph.schema_version, 2);
        let lin_mo = graph
            .nodes
            .iter()
            .find(|node| node.kind == "character" && node.label == "林墨")
            .expect("character node");
        assert_eq!(
            lin_mo
                .state
                .get("current_state")
                .and_then(serde_json::Value::as_str),
            Some("隐瞒重生记忆")
        );
        assert!(graph.edges.iter().any(|edge| edge.kind == "KNOWS"
            && edge.source == lin_mo.id
            && edge.target.contains("陈岚说谎")));
        assert!(graph.edges.iter().any(|edge| edge.kind == "DOES_NOT_KNOW"
            && edge.source == lin_mo.id
            && edge.target.contains("父亲仍活着")));
        assert!(
            graph
                .nodes
                .iter()
                .any(|node| node.kind == "event" && node.label.contains("林墨发现事故监控被剪掉"))
        );
        assert!(
            graph
                .nodes
                .iter()
                .any(|node| node.kind == "promise" && node.label.contains("事故真相"))
        );
        assert_eq!(graph.candidate_updates.len(), 1);
        assert_eq!(graph.candidate_updates[0].kind, "knowledge");
        assert_eq!(graph.candidate_updates[0].chapter, Some(1));
    }

    #[test]
    fn memory_query_formats_story_semantics() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("查询语义测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("cards/characters/lin_mo.yaml"),
            "name: 林墨\nrole: 主角\nwant: 查清事故真相\nfear: 父亲死亡\nsecret: 重生记忆\nknowledge:\n  - 陈岚说谎\nunknown:\n  - 父亲仍活着\nrelationships:\n  - 陈岚: 互相试探\nstate: 隐瞒重生记忆\nlast_seen_chapter: 1\n",
            true,
        )
        .expect("write character");
        write_text(
            &tmp.path().join("memory/events.jsonl"),
            "{\"kind\":\"event\",\"event\":\"林墨发现事故监控被剪掉\",\"chapter\":1}\n",
            true,
        )
        .expect("write events");
        write_text(
            &tmp.path().join("memory/foreshadowing.jsonl"),
            "{\"kind\":\"promise\",\"promise\":\"事故真相\",\"status\":\"progress\",\"first_chapter\":1,\"progress\":\"林墨拿到被剪掉的监控线索\",\"payoff_chapter\":8}\n",
            true,
        )
        .expect("write promises");
        write_text(
            &tmp.path().join("chapters/001/final.md"),
            "# 第 001 章\n\n林墨发现事故真相的第一条线索。",
            true,
        )
        .expect("write chapter");

        let graph = build_memory_graph(tmp.path()).expect("graph");
        let seeds = find_memory_seed_nodes(&graph, "林墨");
        let neighborhood = memory_neighborhood(&graph, &seeds, 2, 64);
        let packet = format_memory_neighborhood(&neighborhood);

        assert!(packet.contains("## Story Semantics"));
        assert!(packet.contains("### Character: 林墨"));
        assert!(packet.contains("Current state: 隐瞒重生记忆"));
        assert!(packet.contains("Knows: 陈岚说谎"));
        assert!(packet.contains("Does not know: 父亲仍活着"));
        assert!(packet.contains("Relationships: 陈岚: 互相试探"));
        assert!(packet.contains("Secrets: 重生记忆"));
        assert!(packet.contains("Appearances: Chapter 001"));
        assert!(packet.contains("### Promise: 事故真相"));
        assert!(packet.contains("Lifecycle status: progress (progress)"));
        assert!(packet.contains("First appearance: chapter 001"));
        assert!(packet.contains("Progress: 林墨拿到被剪掉的监控线索"));
        assert!(packet.contains("Payoff chapter: 008"));
        assert!(packet.contains("### Event Timeline"));
        assert!(packet.contains("chapter 001: 林墨发现事故监控被剪掉"));
    }

    #[test]
    fn memory_impact_packet_summarizes_story_surfaces() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("影响面测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("cards/characters/lin_mo.yaml"),
            "name: 林墨\nknowledge:\n  - 陈岚说谎\nunknown:\n  - 父亲仍活着\nstate: 隐瞒重生记忆\n",
            true,
        )
        .expect("write character");
        write_text(
            &tmp.path().join("cards/locations/warehouse.md"),
            "# 废仓库\n\n林墨在废仓库发现事故监控线索。\n",
            true,
        )
        .expect("write location");
        write_text(
            &tmp.path().join("memory/foreshadowing.jsonl"),
            "{\"kind\":\"promise\",\"promise\":\"事故真相\",\"status\":\"open\",\"first_chapter\":1}\n",
            true,
        )
        .expect("write promise");
        write_text(
            &tmp.path().join("chapters/001/final.md"),
            "# 第 001 章\n\n林墨在废仓库发现事故真相的线索，仍不知道父亲仍活着。",
            true,
        )
        .expect("write chapter 1");
        write_text(
            &tmp.path().join("chapters/002/final.md"),
            "# 第 002 章\n\n林墨继续追查事故真相。",
            true,
        )
        .expect("write chapter 2");
        write_text(
            &tmp.path().join("memory/candidates/001.json"),
            r#"{"chapter":1,"candidates":[{"kind":"knowledge","target":"林墨","change":"开始怀疑陈岚说谎","evidence":"chapters/001/final.md","confidence":0.88,"affects":["character:lin_mo","promise:accident_truth"],"status":"candidate"}]}"#,
            true,
        )
        .expect("write candidate");
        let graph = build_memory_graph(tmp.path()).expect("graph");
        save_memory_graph(tmp.path(), &graph).expect("save graph");

        let packet = memory_impact_packet(tmp.path(), 1, 2, 32).expect("impact");

        assert!(packet.contains("## Impact surface for chapter 001"));
        assert!(packet.contains("## Affected story surfaces"));
        assert!(packet.contains("Characters: 林墨"));
        assert!(packet.contains("Knowledge boundaries:"));
        assert!(packet.contains("Promises / foreshadowing: 事故真相"));
        assert!(packet.contains("Locations: 废仓库"));
        assert!(packet.contains("## Downstream chapters to re-check"));
        assert!(packet.contains("Chapter 002"));
        assert!(packet.contains("Pending candidate memory updates"));
        assert!(packet.contains("开始怀疑陈岚说谎"));
        assert!(packet.contains("## Resource Economy Impact"));
        assert!(packet.contains("deepseek remember 1"));
        assert!(packet.contains("## Nodes"));
    }

    #[test]
    fn memory_promises_packet_groups_lifecycle_entries() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("伏笔生命周期".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("memory/foreshadowing.jsonl"),
            "{\"kind\":\"promise\",\"promise\":\"事故真相\",\"status\":\"progress\",\"first_chapter\":1,\"progress\":\"林墨拿到被剪掉的监控线索\",\"payoff_chapter\":8}\n{\"kind\":\"promise\",\"promise\":\"旧城铃声\",\"status\":\"suspended\",\"first_chapter\":2,\"suspended_reason\":\"等待进入旧城篇\"}\n",
            true,
        )
        .expect("write promises");

        let packet = memory_promises_packet_from_workspace(tmp.path()).expect("promises");

        assert!(packet.contains("# Promise Lifecycle"));
        assert!(packet.contains("status_counts: progress=1, suspended=1"));
        assert!(packet.contains("## progress (progress) (1)"));
        assert!(packet.contains("事故真相 | first: 001 | payoff: 008"));
        assert!(packet.contains("progress: 林墨拿到被剪掉的监控线索"));
        assert!(packet.contains("## suspended (suspended) (1)"));
        assert!(packet.contains("旧城铃声 | first: 002 | payoff: ?"));
        assert!(packet.contains("hold: 等待进入旧城篇"));
        assert!(packet.contains("deepseek remember N"));
    }

    #[test]
    fn memory_archive_compacts_stage_and_regression_reports_windows() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("阶段归档".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        for chapter in 1..=4 {
            write_text(
                &tmp.path().join(format!("chapters/{chapter:03}/final.md")),
                &format!("# 第 {chapter:03} 章\n\n林墨追查事故真相。旧城铃声只被提到一次。"),
                true,
            )
            .expect("write chapter");
            write_text(
                &tmp.path().join(format!("memory/summaries/{chapter:03}.md")),
                &format!("第 {chapter:03} 章摘要：事故真相推进。"),
                true,
            )
            .expect("write summary");
        }
        write_text(
            &tmp.path().join("memory/foreshadowing.jsonl"),
            "{\"kind\":\"promise\",\"promise\":\"事故真相\",\"status\":\"progress\",\"first_chapter\":1,\"progress\":\"阶段内持续推进\"}\n{\"kind\":\"promise\",\"promise\":\"旧城铃声\",\"status\":\"open\",\"first_chapter\":1}\n",
            true,
        )
        .expect("write promise");

        let archive = archive_memory_stage_from_workspace(tmp.path(), 1, 3, Some("第一阶段"))
            .expect("archive");
        assert!(archive.contains("# Memory Archive Created"));
        assert!(archive.contains("memory/archives/stage-001-003.md"));

        let graph = load_memory_graph(tmp.path()).expect("graph");
        assert!(
            graph
                .nodes
                .iter()
                .any(|node| node.kind == "memory_archive" && node.label == "第一阶段")
        );
        let chapter = graph
            .nodes
            .iter()
            .find(|node| node.id == "chapter:001")
            .expect("chapter 1");
        assert_eq!(chapter.state["archived_stage"], "stage-001-003");
        assert!(chapter.summary.contains("stage memory"));

        let regression =
            memory_regression_report_from_workspace(tmp.path(), 2, false).expect("regression");
        assert!(regression.contains("# Continuity Regression"));
        assert!(regression.contains("## Chapters 001-002"));
        assert!(regression.contains("## Chapters 003-004"));
        assert!(regression.contains("active_promises"));
        assert!(regression.contains("anchor_carry:"));
        assert!(regression.contains("weak anchors:"));
        assert!(regression.contains("旧城铃声"));
        assert!(regression.contains("Every 10 chapters"));
    }

    #[test]
    fn quality_reports_surface_context_gaps_and_targeted_revision_goals() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("质量报告".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("memory/foreshadowing.jsonl"),
            "{\"kind\":\"promise\",\"promise\":\"旧城铃声\",\"status\":\"open\",\"first_chapter\":1}\n",
            true,
        )
        .expect("write promise");
        write_text(
            &tmp.path().join("chapters/001/draft.md"),
            "# 第 001 章\n\n旧城铃声出现。林墨站着。旧城铃声再次出现。",
            true,
        )
        .expect("write draft");
        let graph = build_memory_graph(tmp.path()).expect("graph");
        save_memory_graph(tmp.path(), &graph).expect("save graph");

        let context_report = context_quality_report(tmp.path(), 1).expect("context quality");
        assert!(context_report.contains("# ContextQualityReport"));
        assert!(context_report.contains("brief: missing"));
        assert!(context_report.contains("missing chapter brief"));
        assert!(context_report.contains("- structured_signals:"));
        assert!(
            context_report
                .contains("code: missing_chapter_brief, severity: blocker, category: context")
        );

        let draft = read_required(&tmp.path().join("chapters/001/draft.md")).expect("draft");
        let quality = chapter_quality_report(tmp.path(), 1, &draft).expect("chapter quality");
        assert!(quality.contains("# ChapterQualityReport"));
        assert!(quality.contains("- structured_signals:"));
        assert!(quality.contains("dialogue_function"));
        assert!(quality.contains("scene_causality"));
        assert!(quality.contains("promise_progress"));
        assert!(quality.contains("anchor_carry: anchors"));

        let targets =
            targeted_revision_targets(tmp.path(), 1, &draft, "## MAJOR\n- 人物知识边界泄漏\n")
                .expect("targets");
        assert!(targets.contains("audit:major: 人物知识边界泄漏"));
        assert!(targets.contains("quality:"));
        assert!(targets.lines().count() <= 3);
    }

    #[test]
    fn context_quality_flags_summary_bloat_and_candidate_pressure() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("上下文噪声测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("chapters/002/brief.md"),
            "# Brief\n\n继续追查事故真相。",
            true,
        )
        .expect("write brief");
        write_text(
            &tmp.path().join("cards/characters/lin_mo.yaml"),
            "id: lin_mo\nname: 林墨\nrole: 主角\n",
            true,
        )
        .expect("write card");
        write_text(
            &tmp.path().join("memory/foreshadowing.jsonl"),
            "{\"kind\":\"promise\",\"promise\":\"事故真相\",\"status\":\"open\",\"first_chapter\":1}\n",
            true,
        )
        .expect("write promise");
        write_text(
            &tmp.path().join("memory/summaries/001.md"),
            &format!(
                "# 第 001 章记忆\n\n## 章节摘要\n{}\n\n## 写法反馈\n- 解释过量。\n\n## CANDIDATE_MEMORY_UPDATES\n- none\n",
                "这份摘要反复复述正文。".repeat(900)
            ),
            true,
        )
        .expect("write summary");
        let candidates = (0..7)
            .map(|index| {
                format!(
                    "{{\"kind\":\"knowledge\",\"target\":\"林墨\",\"change\":\"确认线索 {index}\",\"evidence\":\"chapters/001/final.md\",\"confidence\":0.9,\"status\":\"candidate\"}}"
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        write_text(
            &tmp.path().join("memory/candidates/001.json"),
            &format!("{{\"chapter\":1,\"candidates\":[{candidates}]}}"),
            true,
        )
        .expect("write candidates");
        let graph = build_memory_graph(tmp.path()).expect("graph");
        save_memory_graph(tmp.path(), &graph).expect("save graph");

        let report = context_quality_report(tmp.path(), 2).expect("context quality");

        assert!(report.contains("overlong_memory_summaries"));
        assert!(report.contains("memory_summary_canon_sparse"));
        assert!(report.contains("memory_candidate_bloat"));
        assert!(report.contains("recent_summary_chars"));
        assert!(report.contains("recent_pending_candidates"));
    }

    #[test]
    fn chapter_quality_reports_style_discipline_scene_gear_and_viewpoint_signals() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("风格纪律".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("chapters/001/brief.md"),
            "# 第 001 章\n\n场景档位: A档 · 高压爆发",
            true,
        )
        .expect("write brief");
        let long = "沈昭不由得深吸一口气，心头一跳。他心想这件事终于瞒不住了。随即，非常，非常，非常，非常，非常，非常多的脚步声压过来。这一刻他终于明白了，命运的齿轮开始转动。";
        let draft = format!(
            "# 第 001 章\n\n{}\n\n{}\n\n{}\n\n短。\n\n短。\n\n短。",
            long.repeat(3),
            long.repeat(3),
            long
        );

        let quality = chapter_quality_report(tmp.path(), 1, &draft).expect("quality");

        assert!(quality.contains("- style_discipline:"));
        assert!(quality.contains("style_zero_tolerance_terms"));
        assert!(quality.contains("style_budget_terms"));
        assert!(quality.contains("style_ai_summary_sentence"));
        assert!(quality.contains("style_paragraph_rhythm"));
        assert!(quality.contains("viewpoint_inner_state_boundary"));
        assert!(quality.contains("scene_gear_a_paragraph_too_long"));
        assert!(quality.contains("scene_gear: A档 · 高压爆发"));
    }

    #[test]
    fn chapter_quality_flags_static_ai_opening_without_viewpoint_action() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("开头纪律".to_string()),
            Some("玄幻仙侠".to_string()),
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("chapters/001/brief.md"),
            "# 第 001 章\n\n场景档位: B档 · 正常推进",
            true,
        )
        .expect("write brief");
        let draft = "# 第 001 章\n\n坊市的废墟还在冒烟。\n\n沈照攥着三枚厄钱走进巷口。\n“货呢？”\n卖家笑了一声。\n他把命灰塞进袖里，决定先救师妹。";

        let quality = chapter_quality_report(tmp.path(), 1, draft).expect("chapter quality");

        assert!(quality.contains("style_ai_opening"));
        assert!(quality.contains("ai_opening_hits: 1"));
        assert!(quality.contains("opening_signal: 坊市的废墟还在冒烟。"));
    }

    #[test]
    fn chapter_quality_allows_opening_environment_attached_to_action() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("开头负例".to_string()),
            Some("玄幻仙侠".to_string()),
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("chapters/001/brief.md"),
            "# 第 001 章\n\n场景档位: B档 · 正常推进",
            true,
        )
        .expect("write brief");
        let draft = "# 第 001 章\n\n沈照攥着三枚厄钱蹲在冒烟的坊市废墟旁，盯着镇厄司铁索后的第三间棚屋。\n\n“货呢？”\n卖家笑了一声。\n他把命灰塞进袖里，决定先救师妹。";

        let quality = chapter_quality_report(tmp.path(), 1, draft).expect("chapter quality");

        assert!(!quality.contains("style_ai_opening"));
        assert!(quality.contains("ai_opening_hits: 0"));
    }

    #[test]
    fn targeted_revision_targets_preserve_audit_source_categories() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("分层审查".to_string()),
            Some("玄幻仙侠".to_string()),
            None,
            500_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("memory/foreshadowing.jsonl"),
            "{\"kind\":\"promise\",\"promise\":\"断剑来历\",\"status\":\"open\",\"first_chapter\":1}\n",
            true,
        )
        .expect("write promise");
        let graph = build_memory_graph(tmp.path()).expect("graph");
        save_memory_graph(tmp.path(), &graph).expect("save graph");
        let draft = "# 第 001 章\n\n沈砚拿起筑基丹。";
        let audit = "## CONTINUITY_AUDIT\n- 人物不知道丹药来源却直接说出丹房密令\n\n## CRAFT_AUDIT\n- 战斗只有剑气堆叠，没有观察链\n\n## READER_PROMISE_AUDIT\n- 断剑来历没有推进\n\n## CANDIDATE_MEMORY_UPDATES\n{\"chapter\":1,\"kind\":\"object_state\",\"target\":\"筑基丹\",\"change\":\"归沈砚持有\",\"evidence\":\"chapters/001/draft.md:2\",\"confidence\":0.9}\n";

        let targets = targeted_revision_targets(tmp.path(), 1, draft, audit).expect("targets");

        assert!(targets.contains("audit:continuity:"));
        assert!(targets.contains("audit:craft:"));
        assert!(targets.lines().count() <= 3);
    }

    #[test]
    fn targeted_revision_targets_ignore_praise_and_no_problem_audit_lines() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("不硬批测试".to_string()),
            Some("玄幻仙侠".to_string()),
            None,
            500_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        let draft = "# 第 001 章\n\n沈砚攥着残剑进门。\n\n“账呢？”\n掌柜停住算盘。\n沈砚把剑鞘压在桌沿，等他改口。";
        let audit = "## CONTINUITY_AUDIT\n- 无 canonical 冲突，时间线连贯。\n- 林墨的短句对白符合人物声口，应保留。\n\n## CRAFT_AUDIT\n- 制度入戏做得很好，不是信息搬运。\n- 动作余味有效，可保留。\n\n## BLOCKER\n- 无 blocker 级别问题\n\n## MAJOR\n- none\n\n## PROTECTED_STRENGTHS\n- 掌柜停住算盘这一动作有效，修订时不得洗掉。\n";

        let audit_targets = extract_actionable_audit_targets(audit);
        let targets = targeted_revision_targets(tmp.path(), 1, draft, audit).expect("targets");
        let strengths = protected_revision_strengths(audit);

        assert!(audit_targets.is_empty(), "{audit_targets:?}");
        assert!(!targets.contains("无 canonical 冲突"));
        assert!(!targets.contains("做得很好"));
        assert!(!targets.contains("应保留"));
        assert!(strengths.contains("掌柜停住算盘"));
        assert!(strengths.contains("动作余味有效"));
    }

    #[test]
    fn targeted_revision_targets_only_take_confirmed_problem_sections() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("问题抽取测试".to_string()),
            Some("玄幻仙侠".to_string()),
            None,
            500_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        let draft = "# 第 001 章\n\n沈砚拿起筑基丹。";
        let audit = "## CONTINUITY_AUDIT\n- 人物不知道丹药来源却直接说出丹房密令\n\n## CRAFT_AUDIT\n- 战斗只有剑气堆叠，没有观察链\n\n## BLOCKER\n- 问题：人物知识边界泄漏，沈砚说出尚未获得的丹房密令。\n\n## MAJOR\n- 风险：断剑来历没有推进。\n\n## PROTECTED_STRENGTHS\n- 筑基丹归属动作清楚，应保护。\n";

        let audit_targets = extract_actionable_audit_targets(audit);
        let targets = targeted_revision_targets(tmp.path(), 1, draft, audit).expect("targets");

        assert!(
            audit_targets
                .iter()
                .any(|target| target.contains("audit:craft: 战斗只有剑气堆叠，没有观察链"))
        );
        assert!(targets.contains("audit:blocker: 问题：人物知识边界泄漏"));
        assert!(targets.contains("audit:major: 风险：断剑来历没有推进"));
        assert!(targets.contains("audit:continuity: 人物不知道丹药来源"));
        assert!(!targets.contains("筑基丹归属动作清楚"));
    }

    #[test]
    fn chapter_quality_flags_dialogue_without_visible_state_change() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("对白功能测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("chapters/001/brief.md"),
            "# 第 001 章\n\n场景档位: B档 · 正常推进",
            true,
        )
        .expect("write brief");
        let filler = "桌边很静。灯芯很直。".repeat(140);
        let draft = format!(
            "# 第 001 章\n\n沈砚坐在桌边。\n\n“你来了。”\n“我来了。”\n“坐吧。”\n“好。”\n\n{filler}"
        );

        let quality = chapter_quality_report(tmp.path(), 1, &draft).expect("quality");

        assert!(quality.contains("scene_goal_missing"));
        assert!(quality.contains("dialogue_no_state_change"));
        assert!(quality.contains("- scene_function:"));
    }

    #[test]
    fn chapter_quality_flags_opening_that_drops_previous_ending() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("章首断裂测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("chapters/001/final.md"),
            "# 第 001 章\n\n林墨把账册塞进鞋底。\n\n苏棠站在禁厄牢门边，低声说：“天亮前，追兵会到。”\n\n他右手还在流血，草环贴着掌心发硬。",
            true,
        )
        .expect("write chapter 1");
        write_text(
            &tmp.path().join("chapters/002/brief.md"),
            "# Brief\n\n场景档位: B档 · 正常推进",
            true,
        )
        .expect("write brief");
        let draft = "# 第 002 章\n\n山外的集市正热闹，掌柜掂着灵石看客人进门。\n\n林墨站在人群外，没有说话。";

        let quality = chapter_quality_report(tmp.path(), 2, draft).expect("quality");

        assert!(quality.contains("chapter_bridge_opening"));
        assert!(quality.contains("- chapter_bridge:"));
        assert!(quality.contains("previous_keywords ["));
    }

    #[test]
    fn chapter_quality_allows_opening_that_bridges_previous_ending() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("章首承接测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("chapters/001/final.md"),
            "# 第 001 章\n\n林墨把账册塞进鞋底。\n\n苏棠站在禁厄牢门边，低声说：“天亮前，追兵会到。”\n\n他右手还在流血，草环贴着掌心发硬。",
            true,
        )
        .expect("write chapter 1");
        write_text(
            &tmp.path().join("chapters/002/brief.md"),
            "# Brief\n\n场景档位: B档 · 正常推进",
            true,
        )
        .expect("write brief");
        let draft = "# 第 002 章\n\n天没亮，林墨右手的血已经粘住袖口，账册仍压在鞋底，草环硌着掌心。\n\n追兵的马蹄声从巷口压过来。";

        let quality = chapter_quality_report(tmp.path(), 2, draft).expect("quality");

        assert!(quality.contains("- chapter_bridge:"));
        assert!(!quality.contains("chapter_bridge_opening"));
    }

    #[test]
    fn chapter_quality_flags_humanizer_inspired_structure_patterns() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("结构套话测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("chapters/001/brief.md"),
            "# 第 001 章\n\n场景档位: B档 · 正常推进",
            true,
        )
        .expect("write brief");
        let draft = "# 第 001 章\n\n林墨攥着账册站在门口。\n\n这不仅仅是一场选择，更深层的意义在于他终于看清命运、真相、秘密。\n\n不是因为他不害怕，而是因为真正重要的是权力、信息、关系。\n\n某种意义上，恐惧、愤怒、痛苦都成了他的台阶。\n\n这意味着他必须往前走。";

        let quality = chapter_quality_report(tmp.path(), 1, draft).expect("quality");

        assert!(quality.contains("style_ai_structure"));
        assert!(quality.contains("style_rule_of_three"));
        assert!(quality.contains("ai_structure_hits:"));
        assert!(quality.contains("rule_of_three_hits:"));
        assert!(quality.contains("structure_examples:"));
    }

    #[test]
    fn default_anti_ai_patterns_include_localized_humanizer_rules() {
        assert!(DEFAULT_ANTI_AI_PATTERNS.contains("不是因为"));
        assert!(DEFAULT_ANTI_AI_PATTERNS.contains("三联排"));
        assert!(DEFAULT_ANTI_AI_PATTERNS.contains("机械轮换称谓"));
        assert!(DEFAULT_ANTI_AI_PATTERNS.contains("这意味着/这说明/这象征"));
    }

    #[test]
    fn xianxia_quality_reports_surface_genre_specific_gaps() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("问剑长生".to_string()),
            Some("玄幻仙侠".to_string()),
            None,
            1_000_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("cards/characters/protagonist.yaml"),
            "id: protagonist\nname: 沈砚\nvoice: 说话先算代价\nknowledge: []\n",
            true,
        )
        .expect("write character card");
        write_text(
            &tmp.path().join("memory/foreshadowing.jsonl"),
            "{\"kind\":\"promise\",\"promise\":\"断剑来历\",\"status\":\"open\",\"first_chapter\":1}\n",
            true,
        )
        .expect("write promise");
        let graph = build_memory_graph(tmp.path()).expect("graph");
        save_memory_graph(tmp.path(), &graph).expect("save graph");

        let context_report = context_quality_report(tmp.path(), 1).expect("context quality");
        assert!(context_report.contains("- genre_profile: xianxia"));
        assert!(context_report.contains("xianxia: missing world/rule cards"));
        assert!(context_report.contains("xianxia: missing resource cards"));
        assert!(context_report.contains("xianxia: missing realm/cultivation rules"));
        assert!(context_report.contains("xianxia: missing resource/economy anchor"));

        let draft = format!(
            "# 第 001 章\n\n{}",
            [
                "沈砚感到无比震惊，觉得极其愤怒，非常痛苦，十分绝望。",
                "他拿到一枚筑基丹和一柄飞剑，却无人说明来源、配给、消耗或后果。",
                "飞剑斩来，剑气轰鸣，灵力炸开，拳罡压下，阵法亮起，血落在石阶上。",
                "“你敢？”",
                "“你敢？”",
                "“你敢？”",
                "宗门境界灵脉洞府坊市山门禁地阵法法宝丹药功法天劫全在一段说明里。",
                "他站在那里。"
            ]
            .join("\n")
        );
        let quality = chapter_quality_report(tmp.path(), 1, &draft).expect("chapter quality");
        assert!(quality.contains("xianxia_emotion_texture"));
        assert!(quality.contains("xianxia_resource_anchor"));
        assert!(quality.contains("xianxia_resource_obligation"));
        assert!(quality.contains("xianxia_combat_knowledge_loop"));
        assert!(quality.contains("xianxia_dialogue_voice"));
        assert!(quality.contains("severity: major"));
        assert!(quality.contains("category: resource_economy"));
        assert!(quality.contains("xianxia_resource_anchor [major|resource_economy]"));
        assert!(quality.contains("xianxia_abstract_emotion_terms:"));
        assert!(quality.contains("xianxia_resource_anchor: resources"));
        assert!(quality.contains("xianxia_combat_loop: combat"));
    }

    #[test]
    fn xianxia_context_scores_resource_cards_and_memory_graph_nodes() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("资源经济".to_string()),
            Some("修仙".to_string()),
            None,
            1_000_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("cards/resources/foundation_pill.yaml"),
            "id: foundation_pill\nname: 筑基丹\ncategory: pill\nrarity: 宗门管制\nmarket_value: 三百下品灵石\nordinary_income_equivalent: 外门弟子十年俸禄\nwho_controls_it: 丹房长老\ncost_to_use: 服用失败会伤经脉\ndebt_or_obligation: 需替宗门守矿三年\nfirst_seen: chapters/001\ncanon_status: canon\nevidence: chapters/001/final.md:8\n",
            true,
        )
        .expect("write resource");
        let graph = build_memory_graph(tmp.path()).expect("graph");
        let resource = graph
            .nodes
            .iter()
            .find(|node| node.source == "cards/resources/foundation_pill.yaml")
            .expect("resource node");
        assert_eq!(resource.kind, "object");
        assert_eq!(resource.label, "筑基丹");
        assert_eq!(resource.state["market_value"], "三百下品灵石");
        assert_eq!(resource.state["has_value_anchor"], true);
        assert_eq!(resource.state["has_control_anchor"], true);
        assert!(graph.edges.iter().any(|edge| {
            edge.source == resource.id
                && edge.kind == "CONTROLLED_BY"
                && edge.target.contains("丹房长老")
        }));

        save_memory_graph(tmp.path(), &graph).expect("save graph");
        let context_report = context_quality_report(tmp.path(), 1).expect("context quality");
        assert!(context_report.contains("resource_cards: 1"));
        assert!(context_report.contains("resource_cards_with_value_anchor: 1"));
        assert!(context_report.contains("resource_cards_with_control_anchor: 1"));
        assert!(!context_report.contains("xianxia: missing resource cards"));
    }

    #[test]
    fn resource_ledger_reports_cards_changes_and_explicit_impacts() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("资源账本".to_string()),
            Some("玄幻仙侠".to_string()),
            None,
            1_000_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("cards/characters/shen_yan.yaml"),
            "id: shen_yan\nname: 沈砚\nstate: 欠丹房人情\n",
            true,
        )
        .expect("write character");
        write_text(
            &tmp.path().join("cards/resources/foundation_pill.yaml"),
            "id: foundation_pill\nname: 筑基丹\ncategory: pill\nmarket_value: 三百下品灵石\nwho_controls_it: 丹房长老\ncost_to_use: 经脉受损风险\ndebt_or_obligation: 替宗门守矿三年\n",
            true,
        )
        .expect("write resource");
        write_text(
            &tmp.path().join("memory/foreshadowing.jsonl"),
            "{\"kind\":\"promise\",\"promise\":\"丹房债务\",\"status\":\"open\",\"first_chapter\":1}\n",
            true,
        )
        .expect("write promise");
        write_text(
            &tmp.path().join("chapters/001/final.md"),
            "# 第 001 章\n\n沈砚拿到筑基丹，也背上丹房债务。",
            true,
        )
        .expect("write chapter");
        write_text(
            &tmp.path().join("memory/candidates/001.json"),
            r#"{"chapter":1,"candidates":[{"kind":"object_state","target":"筑基丹","change":"筑基丹转交沈砚，触发丹房债务","evidence":"chapters/001/final.md","confidence":0.91,"affects":["character:shen_yan","promise:丹房债务"],"status":"candidate"}]}"#,
            true,
        )
        .expect("write candidate");
        let graph = build_memory_graph(tmp.path()).expect("graph");
        save_memory_graph(tmp.path(), &graph).expect("save graph");

        let report =
            resource_ledger_report_from_workspace(tmp.path(), Some(1)).expect("resource ledger");

        assert!(report.contains("# Resource Ledger"));
        assert!(report.contains("chapter_filter: 001"));
        assert!(report.contains("## Resource Economy Impact"));
        assert!(report.contains("筑基丹 | value: 三百下品灵石"));
        assert!(report.contains("controller: 丹房长老"));
        assert!(report.contains("cost: 经脉受损风险"));
        assert!(report.contains("筑基丹转交沈砚，触发丹房债务"));
        assert!(report.contains("impact: character:沈砚"));
        assert!(report.contains("promise:丹房债务"));
        assert!(report.contains("affects: character:shen_yan, promise:丹房债务"));
    }

    #[test]
    fn collect_failure_fixture_archives_original_regression_sample() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("失败样本".to_string()),
            Some("玄幻仙侠".to_string()),
            None,
            500_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("chapters/001/draft.md"),
            "# 第 001 章\n\n沈砚拿到筑基丹，却没有价格、来源或代价。",
            true,
        )
        .expect("write draft");

        let report = collect_failure_fixture(
            tmp.path(),
            FailureKind::ResourceWithoutCost,
            Some(1),
            Path::new("chapters/001/draft.md"),
            "xianxia_resource_anchor",
            "补上价格、控制方、债务或身体代价",
            Some("原创夹具"),
        )
        .expect("collect fixture");

        assert!(report.contains("# Failure Fixture Collected"));
        let files = collect_files_with_extensions(
            &tmp.path().join("eval/failures/resource_without_cost"),
            &["json"],
        )
        .expect("fixture files");
        assert_eq!(files.len(), 1);
        let raw = read_required(&files[0]).expect("fixture");
        assert!(raw.contains("\"kind\": \"resource_without_cost\""));
        assert!(raw.contains("\"expected_signal\": \"xianxia_resource_anchor\""));
        assert!(raw.contains("does not prove real reader quality"));
        assert!(raw.contains("沈砚拿到筑基丹"));
    }

    #[test]
    fn eval_coverage_reports_failure_and_non_trigger_pairs() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("覆盖率".to_string()),
            Some("玄幻仙侠".to_string()),
            None,
            500_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("chapters/001/draft.md"),
            "沈砚拿到筑基丹，却没有代价。",
            true,
        )
        .expect("write failure");
        write_text(
            &tmp.path().join("chapters/002/draft.md"),
            "筑基丹标价三百灵石，由丹房长老控制，服用失败会伤经脉。",
            true,
        )
        .expect("write non trigger");
        collect_failure_fixture(
            tmp.path(),
            FailureKind::ResourceWithoutCost,
            Some(1),
            Path::new("chapters/001/draft.md"),
            "xianxia_resource_anchor",
            "补资源价格和代价",
            None,
        )
        .expect("failure fixture");
        collect_non_trigger_fixture(
            tmp.path(),
            "xianxia_resource_anchor",
            Path::new("chapters/002/draft.md"),
            Some("资源锚点完整"),
        )
        .expect("non trigger");

        let report = eval_fixture_coverage_report(tmp.path()).expect("coverage");

        assert!(report.contains("# Eval Fixture Coverage"));
        assert!(
            report.contains("xianxia_resource_anchor: failures 1, non_triggers 1, status covered")
        );
        assert!(report.contains("not a writing capability result"));
    }

    #[test]
    fn experiment_plan_records_reproducible_scaffold_without_running_chapters() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("实验计划".to_string()),
            Some("玄幻仙侠".to_string()),
            None,
            1_000_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");

        let report = write_experiment_plan(
            tmp.path(),
            "short-run",
            10,
            "targeted_revise",
            Some("deepseek-v4-pro"),
            Some(0.6),
            Some("xianxia-craft"),
            false,
        )
        .expect("experiment plan");

        assert!(report.contains("# Experiment Plan Written"));
        let files =
            collect_files_with_extensions(&tmp.path().join("experiments/configs"), &["json"])
                .expect("experiment configs");
        assert_eq!(files.len(), 1);
        let raw = read_required(&files[0]).expect("config");
        assert!(raw.contains("\"planned_chapters\": 10"));
        assert!(raw.contains("\"workflow\": \"targeted_revise\""));
        assert!(raw.contains("deepseek revise 10"));
        assert!(raw.contains("ContextQualityReport"));
        assert!(raw.contains("\"acceptance_baseline\""));
        assert!(raw.contains("experiments/baselines/long_form_acceptance.md"));
        assert!(raw.contains("\"default_words_per_chapter\": 3500"));
        assert!(raw.contains("chapter_bridge_opening_count"));
        assert!(raw.contains("Plan only"));
        assert!(!tmp.path().join("chapters/010/draft.md").exists());
    }

    #[test]
    fn experiment_snapshot_collects_existing_quality_reports_and_collapse_candidates() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("实验快照".to_string()),
            Some("玄幻仙侠".to_string()),
            None,
            1_000_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("memory/foreshadowing.jsonl"),
            "{\"kind\":\"promise\",\"promise\":\"断剑来历\",\"status\":\"open\",\"first_chapter\":1}\n",
            true,
        )
        .expect("write promise");
        save_memory_graph(tmp.path(), &build_memory_graph(tmp.path()).expect("graph"))
            .expect("save graph");
        write_text(
            &tmp.path().join("chapters/001/draft.md"),
            "# 第 001 章\n\n沈砚拿到筑基丹。旧剑、宗门、灵石都被提到，但没有价格、来源或代价。",
            true,
        )
        .expect("write draft");

        let report = write_experiment_snapshot(tmp.path(), 1, Some(1), Some("run-a"), false)
            .expect("snapshot");

        assert!(report.contains("# Experiment Snapshot Written"));
        let files =
            collect_files_with_extensions(&tmp.path().join("experiments/reports"), &["json"])
                .expect("snapshot files");
        assert_eq!(files.len(), 1);
        let raw = read_required(&files[0]).expect("snapshot json");
        assert!(raw.contains("\"run_id\": \"run-a\""));
        assert!(raw.contains("ContextQualityReport"));
        assert!(raw.contains("ChapterQualityReport"));
        assert!(raw.contains("resource_without_cost"));
        assert!(raw.contains("missing_audit"));
        assert!(raw.contains("no capability claim"));
    }

    #[test]
    fn general_quality_report_does_not_emit_xianxia_signals() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("都市追凶".to_string()),
            Some("都市悬疑".to_string()),
            None,
            300_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("memory/foreshadowing.jsonl"),
            "{\"kind\":\"promise\",\"promise\":\"事故真相\",\"status\":\"open\",\"first_chapter\":1}\n",
            true,
        )
        .expect("write promise");
        let graph = build_memory_graph(tmp.path()).expect("graph");
        save_memory_graph(tmp.path(), &graph).expect("save graph");
        let draft = "# 第 001 章\n\n林墨感到非常愤怒，非常痛苦，非常震惊。旧剑、阵列和资源报价都写在案卷里。\n“你敢？”\n“你敢？”\n“你敢？”\n";

        let quality = chapter_quality_report(tmp.path(), 1, draft).expect("chapter quality");
        assert!(quality.contains("- genre_profile: general"));
        assert!(!quality.contains("xianxia_"));
    }

    #[test]
    fn memory_graph_reads_structured_candidate_files() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("候选文件测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("memory/candidates/001.json"),
            r#"{
  "chapter": 1,
  "source": "memory/summaries/001.md",
  "candidates": [
    {
      "kind": "relationship",
      "target": "林墨/陈岚",
      "change": "互相试探升级为公开对峙",
      "evidence": "chapters/001/final.md:42",
      "confidence": 0.86,
      "affects": ["character:lin_mo", "character:chen_lan"],
      "status": "candidate"
    }
  ]
}"#,
            true,
        )
        .expect("write candidates");

        let graph = build_memory_graph(tmp.path()).expect("graph");

        assert_eq!(graph.candidate_updates.len(), 1);
        assert_eq!(graph.candidate_updates[0].kind, "relationship");
        assert_eq!(graph.candidate_updates[0].target, "林墨/陈岚");
        assert_eq!(graph.candidate_updates[0].chapter, Some(1));
    }

    #[test]
    fn remember_candidate_writer_extracts_machine_readable_json() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("候选写入测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        let summary_path = tmp.path().join("memory/summaries/001.md");
        write_text(&summary_path, "# 第 001 章记忆\n", true).expect("write summary");
        let count = write_memory_candidate_file(
            tmp.path(),
            1,
            &summary_path,
            "## CANDIDATE_MEMORY_UPDATES\n- 候选记忆更新：target: 林墨 change: 确认陈岚说谎 evidence: 终章对话 confidence 0.91 affects: character:chen_lan,promise:accident_truth\n",
        )
        .expect("write candidates");

        assert_eq!(count, 1);
        let raw = std::fs::read_to_string(tmp.path().join("memory/candidates/001.json"))
            .expect("candidate json");
        let value: serde_json::Value = serde_json::from_str(&raw).expect("parse json");
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["chapter"], 1);
        assert_eq!(value["status"], "pending_review");
        assert_eq!(value["source"], "memory/summaries/001.md");
        assert_eq!(value["candidates"][0]["kind"], "knowledge");
        assert_eq!(value["candidates"][0]["status"], "candidate");
        let confidence = value["candidates"][0]["confidence"]
            .as_f64()
            .expect("confidence");
        assert!((confidence - 0.91).abs() < 0.001);
        assert_eq!(value["candidates"][0]["change"], "确认陈岚说谎");
    }

    #[test]
    fn candidate_writer_extracts_json_lines_from_candidate_section_only() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("JSON候选测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        let audit_path = tmp.path().join("chapters/001/audit.md");
        let count = write_memory_candidate_file(
            tmp.path(),
            1,
            &audit_path,
            "## BLOCKER\n{\"kind\":\"memory\",\"target\":\"示例\",\"change\":\"不应抓取\",\"confidence\":1.0}\n\n## CANDIDATE_MEMORY_UPDATES\n{\"chapter\":1,\"kind\":\"promise\",\"target\":\"事故真相\",\"change\":\"从新埋推进为林墨主动追查\",\"evidence\":\"chapters/001/final.md:12\",\"confidence\":0.87,\"affects\":[\"character:lin_mo\",\"promise:accident_truth\"]}\n",
        )
        .expect("write candidates");

        assert_eq!(count, 1);
        let raw = std::fs::read_to_string(tmp.path().join("memory/candidates/001.json"))
            .expect("candidate json");
        let value: serde_json::Value = serde_json::from_str(&raw).expect("parse json");
        assert_eq!(value["candidates"].as_array().unwrap().len(), 1);
        assert_eq!(value["candidates"][0]["kind"], "promise");
        assert_eq!(value["candidates"][0]["target"], "事故真相");
        assert_eq!(
            value["candidates"][0]["affects"][0],
            serde_json::Value::String("character:lin_mo".to_string())
        );
    }

    #[test]
    fn candidate_writer_accepts_localized_candidate_heading_suffix() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("本地化候选标题测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        let audit_path = tmp.path().join("chapters/001/audit.md");
        let count = write_memory_candidate_file(
            tmp.path(),
            1,
            &audit_path,
            "## MEMORY_CANDIDATE_AUDIT（记忆候选审计）\n下列变化值得进入候选记忆：\n\n## CANDIDATE_MEMORY_UPDATES（候选记忆更新）\n{\"chapter\":1,\"kind\":\"knowledge\",\"target\":\"林墨\",\"change\":\"确认陈岚隐藏事故线索\",\"evidence\":\"chapters/001/final.md:9\",\"confidence\":0.91}\n",
        )
        .expect("write candidates");

        assert_eq!(count, 1);
        let raw = std::fs::read_to_string(tmp.path().join("memory/candidates/001.json"))
            .expect("candidate json");
        let value: serde_json::Value = serde_json::from_str(&raw).expect("parse json");
        assert_eq!(value["candidates"].as_array().unwrap().len(), 1);
        assert_eq!(value["candidates"][0]["target"], "林墨");
        assert!(!raw.contains("下列变化值得进入候选记忆"));
    }

    #[test]
    fn candidate_writer_does_not_fallback_when_candidate_section_is_none() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("空候选标题测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        let audit_path = tmp.path().join("chapters/001/audit.md");
        let count = write_memory_candidate_file(
            tmp.path(),
            1,
            &audit_path,
            "## MEMORY_CANDIDATE_AUDIT（记忆候选审计）\n下列变化值得进入候选记忆：\n\n## CANDIDATE_MEMORY_UPDATES（候选记忆更新）\n- none\n",
        )
        .expect("write candidates");

        assert_eq!(count, 0);
        let raw = std::fs::read_to_string(tmp.path().join("memory/candidates/001.json"))
            .expect("candidate json");
        let value: serde_json::Value = serde_json::from_str(&raw).expect("parse json");
        assert_eq!(value["candidates"].as_array().unwrap().len(), 0);
        assert!(!raw.contains("下列变化值得进入候选记忆"));
    }

    #[test]
    fn candidate_writer_ignores_candidate_language_outside_section() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("候选污染测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        let audit_path = tmp.path().join("chapters/001/audit.md");
        let count = write_memory_candidate_file(
            tmp.path(),
            1,
            &audit_path,
            "## MEMORY_CANDIDATE_AUDIT\n- 下列变化值得进入候选记忆：候选记忆更新：target: 江鹤 change: 注意**只是审稿建议 evidence: audit confidence 0.9\n\n## CRAFT_AUDIT\n- 需要登记人物动机，但这不是事实。\n",
        )
        .expect("write candidates");

        assert_eq!(count, 0);
        let raw = std::fs::read_to_string(tmp.path().join("memory/candidates/001.json"))
            .expect("candidate json");
        assert!(!raw.contains("江鹤"));
        assert!(!raw.contains("注意**"));
    }

    #[test]
    fn candidate_writer_rejects_malformed_candidate_fields() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("畸形候选测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        let audit_path = tmp.path().join("chapters/001/audit.md");
        let count = write_memory_candidate_file(
            tmp.path(),
            1,
            &audit_path,
            "## CANDIDATE_MEMORY_UPDATES\n- 候选记忆更新：target: 注意** change: target: 江鹤 change: 诊断建议 evidence: audit confidence 0.9\n{\"chapter\":1,\"kind\":\"knowledge\",\"target\":\"候选记忆更新：target: 林墨\",\"change\":\"确认线索\",\"confidence\":0.9}\n{\"chapter\":1,\"kind\":\"knowledge\",\"target\":\"林墨\",\"change\":\"确认陈岚隐藏事故线索\",\"evidence\":\"chapters/001/final.md:9\",\"confidence\":0.91}\n",
        )
        .expect("write candidates");

        assert_eq!(count, 1);
        let raw = std::fs::read_to_string(tmp.path().join("memory/candidates/001.json"))
            .expect("candidate json");
        let value: serde_json::Value = serde_json::from_str(&raw).expect("parse json");
        assert_eq!(value["candidates"].as_array().unwrap().len(), 1);
        assert_eq!(value["candidates"][0]["target"], "林墨");
        assert!(!raw.contains("注意**"));
        assert!(!raw.contains("候选记忆更新：target"));
    }

    #[test]
    fn remember_candidate_writer_does_not_fallback_when_candidate_section_says_none() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("候选兜底测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        let summary_path = tmp.path().join("memory/summaries/001.md");
        let summary = r#"# 第 001 章记忆

## 人物状态变化
- 林墨发现外门账本与实际发放的灵石数量不一致，决定暂时隐瞒并独自调查。

## 新增事实锁
- 外门月例灵石成色下降，三块下品灵石的灵力价值低于正常水准。

## 伏笔台账
- 账本副本成为后续调查资源流向的关键线索。

## 写法反馈
- 场景用观察链替代了抽象情绪。

## CANDIDATE_MEMORY_UPDATES
- none
"#;
        write_text(&summary_path, summary, true).expect("write summary");
        let count = write_memory_candidate_file_with_fallback(
            tmp.path(),
            1,
            &summary_path,
            summary,
            Some("林墨盯着三块劣质灵石，确认账本数目对不上。"),
        )
        .expect("write candidates");

        assert_eq!(count, 0);
        let raw = std::fs::read_to_string(tmp.path().join("memory/candidates/001.json"))
            .expect("candidate json");
        let value: serde_json::Value = serde_json::from_str(&raw).expect("parse json");
        let candidates = value["candidates"].as_array().expect("candidates");
        assert!(candidates.is_empty());
        assert!(!raw.contains("场景用观察链替代了抽象情绪"));
    }

    #[test]
    fn remember_candidate_writer_falls_back_from_chapter_text_when_section_missing() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("候选缺节兜底测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        let summary_path = tmp.path().join("memory/summaries/001.md");
        let summary = r#"# 第 001 章记忆

## 人物状态变化
- 林墨发现外门账本与实际发放的灵石数量不一致，决定暂时隐瞒并独自调查。

## 写法反馈
- 场景用观察链替代了抽象情绪。
"#;
        write_text(&summary_path, summary, true).expect("write summary");
        let count = write_memory_candidate_file_with_fallback(
            tmp.path(),
            1,
            &summary_path,
            summary,
            Some(
                "林墨发现外门账本与实际发放的灵石数量不一致。\n林墨决定隐瞒账本副本，独自追查资源流向。\n账本副本成为后续调查资源流向的关键线索。",
            ),
        )
        .expect("write candidates");

        assert!(
            count >= 2,
            "expected chapter-text fallback candidates, got {count}"
        );
        let raw = std::fs::read_to_string(tmp.path().join("memory/candidates/001.json"))
            .expect("candidate json");
        let value: serde_json::Value = serde_json::from_str(&raw).expect("parse json");
        let candidates = value["candidates"].as_array().expect("candidates");
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate["kind"] == "object_state")
        );
        assert!(raw.contains("deterministic chapter fallback"));
        assert!(!raw.contains("场景用观察链替代了抽象情绪"));
    }

    #[test]
    fn apply_memory_candidates_appends_ledgers_and_marks_applied() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("应用候选测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("memory/candidates/001.json"),
            r#"{
  "chapter": 1,
  "source": "memory/summaries/001.md",
  "candidates": [
    {
      "kind": "knowledge",
      "target": "事故监控",
      "change": "林墨发现事故监控被剪掉",
      "evidence": "chapters/001/final.md:42",
      "confidence": 0.86,
      "affects": ["character:lin_mo", "promise:accident_truth"],
      "status": "candidate"
    }
  ]
}"#,
            true,
        )
        .expect("write candidates");

        let report = apply_memory_candidates(tmp.path(), Some(1), false).expect("apply");

        assert!(report.contains("Applied candidates"));
        assert!(report.contains("memory/behavior.jsonl"));
        let facts =
            std::fs::read_to_string(tmp.path().join("memory/facts.jsonl")).expect("facts ledger");
        assert!(facts.contains("林墨发现事故监控被剪掉"));
        let behavior =
            std::fs::read_to_string(tmp.path().join("memory/behavior.jsonl")).expect("behavior");
        assert!(behavior.contains("lin_mo"));
        assert!(behavior.contains("林墨发现事故监控被剪掉"));
        let candidates = std::fs::read_to_string(tmp.path().join("memory/candidates/001.json"))
            .expect("candidate file");
        assert!(candidates.contains("\"status\": \"applied\""));
        let pending = memory_candidates_for_display(tmp.path(), Some(1), false).expect("pending");
        assert!(pending.is_empty());
    }

    #[test]
    fn apply_memory_candidates_canonicalizes_promise_and_timeline_ledgers() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("候选规范化测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("memory/candidates/001.json"),
            r#"{
  "chapter": 1,
  "candidates": [
    {
      "kind": "foreshadowing",
      "target": "事故真相",
      "change": "推进为林墨主动追查事故真相",
      "evidence": "chapters/001/final.md:12",
      "confidence": 0.9,
      "affects": ["promise:accident_truth"],
      "status": "candidate"
    },
    {
      "kind": "timeline",
      "target": "林墨发现监控被剪",
      "change": "林墨在第一章发现监控被剪",
      "evidence": "chapters/001/final.md:20",
      "confidence": 0.88,
      "affects": ["character:lin_mo"],
      "status": "candidate"
    }
  ]
}"#,
            true,
        )
        .expect("write candidates");

        let report = apply_memory_candidates(tmp.path(), Some(1), false).expect("apply");
        assert!(report.contains("Applied candidates"));

        let promises =
            std::fs::read_to_string(tmp.path().join("memory/foreshadowing.jsonl")).expect("read");
        assert!(promises.contains(r#""kind":"promise""#));
        assert!(promises.contains(r#""promise":"事故真相""#));
        assert!(promises.contains(r#""status":"progress""#));
        assert!(promises.contains(r#""first_chapter":1"#));
        assert!(!promises.contains(r#""kind":"foreshadowing""#));

        let events = std::fs::read_to_string(tmp.path().join("memory/events.jsonl")).expect("read");
        assert!(events.contains(r#""kind":"event""#));
        assert!(events.contains(r#""event":"林墨发现监控被剪""#));
        assert!(!events.contains(r#""kind":"timeline""#));

        let graph = build_memory_graph(tmp.path()).expect("graph");
        assert!(
            graph
                .nodes
                .iter()
                .any(|node| node.kind == "promise" && node.label == "事故真相")
        );
        assert!(
            graph
                .nodes
                .iter()
                .any(|node| node.kind == "event" && node.label == "林墨发现监控被剪")
        );

        let seeds = find_memory_seed_nodes(&graph, "事故真相");
        let packet = format_memory_neighborhood(&memory_neighborhood(&graph, &seeds, 2, 32));
        assert!(packet.contains("### Promise: 事故真相"));
        assert!(packet.contains("Lifecycle status: progress (progress)"));
        assert!(packet.contains("First appearance: chapter 001"));
        assert!(packet.contains("Progress: 推进为林墨主动追查事故真相"));

        let seeds = find_memory_seed_nodes(&graph, "林墨发现监控被剪");
        let packet = format_memory_neighborhood(&memory_neighborhood(&graph, &seeds, 2, 32));
        assert!(packet.contains("### Event Timeline"));
        assert!(packet.contains("chapter 001: 林墨发现监控被剪"));
    }

    #[test]
    fn memory_query_summarizes_applied_state_changes() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("状态变更测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("cards/characters/lin_mo.yaml"),
            "name: 林墨\nstate: 隐瞒重生记忆\n",
            true,
        )
        .expect("write character");
        write_text(
            &tmp.path().join("cards/locations/warehouse.md"),
            "# 废仓库\n\n事故线索所在地。\n",
            true,
        )
        .expect("write location");
        write_text(
            &tmp.path().join("memory/candidates/001.json"),
            r#"{
  "chapter": 1,
  "candidates": [
    {
      "kind": "character_state",
      "target": "林墨",
      "change": "从被动躲避转为主动追查事故真相",
      "evidence": "chapters/001/final.md:30",
      "confidence": 0.91,
      "affects": ["character:lin_mo"],
      "status": "candidate"
    },
    {
      "kind": "location_state",
      "target": "废仓库",
      "change": "监控硬盘被人提前取走",
      "evidence": "chapters/001/final.md:46",
      "confidence": 0.82,
      "affects": ["location:warehouse"],
      "status": "candidate"
    },
    {
      "kind": "object_state",
      "target": "监控硬盘",
      "change": "从废仓库失踪，可能落入陈岚手中",
      "evidence": "chapters/001/final.md:48",
      "confidence": 0.8,
      "affects": ["object:drive", "character:chen_lan"],
      "status": "candidate"
    }
  ]
}"#,
            true,
        )
        .expect("write candidates");

        apply_memory_candidates(tmp.path(), Some(1), false).expect("apply");
        let graph = build_memory_graph(tmp.path()).expect("graph");

        assert!(
            graph
                .nodes
                .iter()
                .any(|node| node.kind == "character_state" && node.label == "林墨")
        );
        assert!(
            graph
                .nodes
                .iter()
                .any(|node| node.kind == "location_state" && node.label == "废仓库")
        );
        assert!(
            graph
                .nodes
                .iter()
                .any(|node| node.kind == "object_state" && node.label == "监控硬盘")
        );
        let object_id = graph
            .nodes
            .iter()
            .find(|node| node.kind == "object" && node.label == "监控硬盘")
            .expect("object node")
            .id
            .clone();
        assert!(graph.edges.iter().any(|edge| edge.kind == "CHANGES"
            && edge.target == object_id
            && edge.source.starts_with("object_state:")));

        let seeds = find_memory_seed_nodes(&graph, "林墨");
        let packet = format_memory_neighborhood(&memory_neighborhood(&graph, &seeds, 2, 64));
        assert!(packet.contains("### State Changes"));
        assert!(packet.contains("chapter 001: 林墨 -> 从被动躲避转为主动追查事故真相"));

        let seeds = find_memory_seed_nodes(&graph, "废仓库");
        let packet = format_memory_neighborhood(&memory_neighborhood(&graph, &seeds, 2, 64));
        assert!(packet.contains("chapter 001: 废仓库 -> 监控硬盘被人提前取走"));
        assert!(packet.contains("chapter 001: 监控硬盘 -> 从废仓库失踪，可能落入陈岚手中"));

        let seeds = find_memory_seed_nodes(&graph, "监控硬盘");
        let packet = format_memory_neighborhood(&memory_neighborhood(&graph, &seeds, 2, 64));
        assert!(packet.contains("[object] 监控硬盘"));
        assert!(packet.contains("监控硬盘 -> 从废仓库失踪，可能落入陈岚手中"));
    }

    #[test]
    fn memory_query_formats_applied_relationship_changes() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("关系变更测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("cards/characters/lin_mo.yaml"),
            "name: 林墨\nrelationships:\n  - 陈岚: 互相试探\n",
            true,
        )
        .expect("write lin mo");
        write_text(
            &tmp.path().join("cards/characters/chen_lan.yaml"),
            "name: 陈岚\nrelationships:\n  - 林墨: 隐瞒关键线索\n",
            true,
        )
        .expect("write chen lan");
        write_text(
            &tmp.path().join("memory/candidates/001.json"),
            r#"{
  "chapter": 1,
  "candidates": [
    {
      "kind": "relationship",
      "target": "林墨/陈岚",
      "change": "从互相试探升级为公开对峙",
      "evidence": "chapters/001/final.md:52",
      "confidence": 0.86,
      "affects": ["character:lin_mo", "character:chen_lan"],
      "status": "candidate"
    }
  ]
}"#,
            true,
        )
        .expect("write candidates");

        apply_memory_candidates(tmp.path(), Some(1), false).expect("apply");
        let graph = build_memory_graph(tmp.path()).expect("graph");
        let lin_mo_id = graph
            .nodes
            .iter()
            .find(|node| node.kind == "character" && node.label == "林墨")
            .expect("lin mo")
            .id
            .clone();
        let chen_lan_id = graph
            .nodes
            .iter()
            .find(|node| node.kind == "character" && node.label == "陈岚")
            .expect("chen lan")
            .id
            .clone();
        let relationship_id = graph
            .nodes
            .iter()
            .find(|node| node.kind == "relationship" && node.label == "林墨/陈岚")
            .expect("relationship")
            .id
            .clone();

        assert!(graph.edges.iter().any(|edge| edge.kind == "AFFECTS"
            && edge.source == relationship_id
            && edge.target == lin_mo_id));
        assert!(graph.edges.iter().any(|edge| edge.kind == "AFFECTS"
            && edge.source == relationship_id
            && edge.target == chen_lan_id));

        let seeds = find_memory_seed_nodes(&graph, "林墨");
        let packet = format_memory_neighborhood(&memory_neighborhood(&graph, &seeds, 2, 64));

        assert!(packet.contains("### Relationship States"));
        assert!(packet.contains("陈岚: 互相试探"));
        assert!(packet.contains("chapter 001: 林墨/陈岚 -> 从互相试探升级为公开对峙"));
        assert!(!packet.contains(r#""kind":"relationship""#));
    }

    #[test]
    fn writing_context_includes_memory_graph_packet_when_present() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("图上下文测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("cards/characters/lin_mo.yaml"),
            "name: 林墨\nstate: 隐瞒古剑来历\n",
            true,
        )
        .expect("write character");
        write_text(
            &tmp.path().join("chapters/001/final.md"),
            "# 第 001 章\n\n林墨握住古剑。",
            true,
        )
        .expect("write chapter");
        let graph = build_memory_graph(tmp.path()).expect("graph");
        save_memory_graph(tmp.path(), &graph).expect("save graph");

        let context = writing_context(tmp.path(), 2).expect("context");

        assert!(context.contains("Long-form memory graph context"));
        assert!(context.contains("Memory Graph Context"));
        assert!(context.contains("林墨"));
        assert!(!context.contains("人物名"));
    }

    #[test]
    fn writing_context_rebuilds_stale_memory_graph_packet() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("图谱陈旧测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("cards/characters/lin_mo.yaml"),
            "name: 林墨\nstate: 旧状态\n",
            true,
        )
        .expect("write old character");
        let graph = build_memory_graph(tmp.path()).expect("graph");
        save_memory_graph(tmp.path(), &graph).expect("save graph");
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_text(
            &tmp.path().join("cards/characters/lin_mo.yaml"),
            "name: 林墨\nstate: 新状态\n",
            true,
        )
        .expect("write new character");

        let context = writing_context(tmp.path(), 1).expect("context");

        assert!(context.contains("新状态"), "{context}");
        assert!(!context.contains("旧状态"), "{context}");
    }

    #[test]
    fn context_quality_blockers_gate_write_unless_explicitly_allowed() {
        let report = "# ContextQualityReport\n\n- structured_signals:\n  - code: missing_chapter_brief, severity: blocker, category: context, message: missing chapter brief\n";

        let err = ensure_context_quality_allows_generation(2, report, false)
            .expect_err("blocker should stop generation");
        assert!(err.to_string().contains("--allow-degraded-context"));
        ensure_context_quality_allows_generation(2, report, true).expect("allowed");
    }

    #[test]
    fn limited_writing_context_preserves_current_chapter_packet() {
        let context = limit_novel_context_prioritized_with_limit(
            vec![
                format!("# Project\n\n{}", "远期大纲噪声。".repeat(4_000)),
                "## Current chapter brief\n\nCURRENT_BRIEF_MUST_SURVIVE".to_string(),
            ],
            1,
            Some(5_000),
        );

        assert!(context.len() <= 5_000);
        assert!(context.contains("CURRENT_BRIEF_MUST_SURVIVE"), "{context}");
    }

    #[test]
    fn writing_context_injects_scene_gear_behavior_and_authorized_references() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("写前上下文纪律".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("chapters/002/brief.md"),
            "# 第 002 章\n\n场景档位: A档 · 高压爆发\n沈昭被追到巷口。",
            true,
        )
        .expect("write brief");
        write_text(
            &tmp.path().join("memory/behavior.jsonl"),
            "{\"chapter\":1,\"character\":\"沈昭\",\"situation\":\"发现木牌有旧裂纹\",\"choice\":\"没告诉何伯，藏入棚柱夹层\",\"result\":\"证据保住了\",\"evidence\":\"chapters/001/final.md\",\"source\":\"test\"}\n",
            true,
        )
        .expect("write behavior");
        write_text(
            &tmp.path().join("craft/examples/battle.md"),
            "authorized: true\nscene: battle\n\n瓦片在身后裂开。沈昭没有回头，手肘压住木牌，贴着墙根滑进雨线里。",
            true,
        )
        .expect("write reference");
        write_text(
            &tmp.path().join("craft/examples/unauthorized.md"),
            "scene: battle\n\n这段没有授权标记，不能进入上下文。",
            true,
        )
        .expect("write unauthorized");

        let context = writing_context(tmp.path(), 2).expect("context");

        assert!(context.contains("## SceneGear"));
        assert!(context.contains("A档 · 高压爆发"));
        assert!(context.contains("## Recent Character Behavior Ledger"));
        assert!(context.contains("沈昭 | 发现木牌有旧裂纹 | 没告诉何伯"));
        assert!(context.contains("## Authorized Reference Passages"));
        assert!(context.contains("craft/examples/battle.md"));
        assert!(!context.contains("不能进入上下文"));
    }

    #[test]
    fn import_analysis_report_stages_candidates_and_rebuilds_graph() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("RLM 导入测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        let report = tmp.path().join("analysis.md");
        write_text(
            &report,
            "## CANDIDATE_MEMORY_UPDATES\n{\"chapter\":1,\"kind\":\"knowledge\",\"target\":\"林墨\",\"change\":\"确认陈岚隐藏事故线索\",\"evidence\":\"chapters/001/final.md:9\",\"confidence\":0.91,\"affects\":[\"character:lin_mo\"]}\n",
            true,
        )
        .expect("write report");

        let message = import_analysis_report_from_workspace(tmp.path(), &report).expect("import");

        assert!(message.contains("candidate updates: 1"));
        let candidates =
            collect_files_with_extensions(&tmp.path().join("memory/candidates"), &["json"])
                .expect("candidates");
        assert!(candidates.iter().any(|path| {
            path.file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|name| name.starts_with("analysis-"))
        }));
        let graph = read_required(&tmp.path().join("memory/graph.json")).expect("graph");
        assert!(graph.contains("确认陈岚隐藏事故线索"));
    }

    #[test]
    fn memory_reports_packet_lists_imported_analysis_and_candidate_counts() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("RLM 报告列表测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        let report = tmp.path().join("analysis.md");
        write_text(
            &report,
            "## CANDIDATE_MEMORY_UPDATES\n{\"chapter\":1,\"kind\":\"knowledge\",\"target\":\"林墨\",\"change\":\"确认陈岚隐藏事故线索\",\"evidence\":\"chapters/001/final.md:9\",\"confidence\":0.91}\n",
            true,
        )
        .expect("write report");
        import_analysis_report_from_workspace(tmp.path(), &report).expect("import");

        let packet = memory_reports_packet_from_workspace(tmp.path()).expect("reports");

        assert!(packet.contains("# Imported Manuscript Analysis Reports"));
        assert!(packet.contains("reports: 1"));
        assert!(packet.contains("candidates: 1"));
        assert!(packet.contains("pending: 1"));
        assert!(packet.contains("source: `analysis.md`"));
        assert!(packet.contains("candidate_file: `memory/candidates/analysis-"));
    }

    #[test]
    fn workspace_summary_surfaces_workflow_readiness_and_memory_noise() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("闭环状态测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("outline/master_plan.md"),
            &"总纲已经完成，足够长，避免仍被识别为模板。".repeat(8),
            true,
        )
        .expect("plan");
        write_text(
            &tmp.path().join("cards/characters/lin_mo.yaml"),
            "name: 林墨\naliases: 林先生\n",
            true,
        )
        .expect("card");
        write_text(
            &tmp.path().join("chapters/001/brief.md"),
            "# 第 001 章\n\n场景档位: A档\n",
            true,
        )
        .expect("brief");
        write_text(
            &tmp.path().join("chapters/001/draft.md"),
            "# 第 001 章\n\n林墨推门入局。",
            true,
        )
        .expect("draft");
        write_text(
            &tmp.path().join("memory/summaries/001.md"),
            &format!(
                "# 第 001 章记忆\n\n## 章节摘要\n{}\n\n## CANDIDATE_MEMORY_UPDATES\n- none\n",
                "摘要噪声。".repeat(900)
            ),
            true,
        )
        .expect("summary");
        write_text(
            &tmp.path().join("memory/candidates/001.json"),
            r#"{
  "candidates": [
    {"chapter":1,"kind":"knowledge","target":"林墨","change":"知道旧案一","evidence":"chapters/001/draft.md","confidence":0.9},
    {"chapter":1,"kind":"knowledge","target":"林墨","change":"知道旧案二","evidence":"chapters/001/draft.md","confidence":0.9},
    {"chapter":1,"kind":"knowledge","target":"林墨","change":"知道旧案三","evidence":"chapters/001/draft.md","confidence":0.9},
    {"chapter":1,"kind":"knowledge","target":"林墨","change":"知道旧案四","evidence":"chapters/001/draft.md","confidence":0.9},
    {"chapter":1,"kind":"knowledge","target":"林墨","change":"知道旧案五","evidence":"chapters/001/draft.md","confidence":0.9},
    {"chapter":1,"kind":"knowledge","target":"林墨","change":"知道旧案六","evidence":"chapters/001/draft.md","confidence":0.9},
    {"chapter":1,"kind":"knowledge","target":"林墨","change":"知道旧案七","evidence":"chapters/001/draft.md","confidence":0.9}
  ]
}"#,
            true,
        )
        .expect("candidates");
        rebuild_memory_graph(tmp.path()).expect("graph");

        let summary = workspace_summary(tmp.path()).expect("summary");

        assert_eq!(summary.readiness.next_action, "deepseek audit 1");
        assert_eq!(summary.readiness.quality_gate, "needs-review");
        assert!(summary.readiness.blockers.is_empty());
        assert_eq!(summary.readiness.candidate_pressure_max_per_chapter, 7);
        assert!(summary.readiness.recent_summary_overweight > 0);
        assert!(summary.readiness.recent_summary_canon_sparse > 0);
        assert!(
            summary
                .readiness
                .warnings
                .iter()
                .any(|line| line.contains("pending memory candidates exceed target"))
        );
    }

    #[test]
    fn workspace_summary_blocks_only_when_next_brief_is_the_next_action() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("下一章门禁测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("outline/master_plan.md"),
            &"总纲已经完成，足够长，避免仍被识别为模板。".repeat(8),
            true,
        )
        .expect("plan");
        write_text(
            &tmp.path().join("cards/characters/lin_mo.yaml"),
            "name: 林墨\naliases: 林先生\n",
            true,
        )
        .expect("card");
        write_text(
            &tmp.path().join("chapters/001/brief.md"),
            "# 第 001 章\n\n场景档位: A档\n",
            true,
        )
        .expect("brief");
        write_text(
            &tmp.path().join("chapters/001/final.md"),
            "# 第 001 章\n\n林墨完成第一步。",
            true,
        )
        .expect("final");
        write_text(
            &tmp.path().join("chapters/001/audit.md"),
            "## OK\n- 通过\n",
            true,
        )
        .expect("audit");
        write_text(
            &tmp.path().join("memory/summaries/001.md"),
            "# 第 001 章记忆\n\n## 章节摘要\n林墨完成第一步。\n\n## 人物知识边界\n- 林墨：知道第一步结果。\n\n## 承诺推进\n- 无。\n\n## 资源变化\n- 无。\n\n## 地点状态\n- 无。\n\n## 未兑现伏笔\n- 无。\n",
            true,
        )
        .expect("summary");
        rebuild_memory_graph(tmp.path()).expect("graph");

        let summary = workspace_summary(tmp.path()).expect("summary");

        assert_eq!(summary.readiness.next_action, "deepseek brief 2");
        assert_eq!(summary.readiness.quality_gate, "blocked");
        assert!(
            summary
                .readiness
                .blockers
                .iter()
                .any(|line| line.contains("chapter 002 brief is missing"))
        );
    }

    #[test]
    fn memory_cleanup_dry_run_reports_without_writing() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("清理预览测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        let candidate_path = tmp.path().join("memory/candidates/001.json");
        write_text(
            &candidate_path,
            r#"{
  "chapter": 1,
  "candidates": [
    {"kind":"knowledge","target":"候选记忆更新：target: 林墨","change":"确认线索","confidence":0.9},
    {"kind":"knowledge","target":"林墨","change":"确认陈岚隐藏事故线索","evidence":"chapters/001/final.md:9","confidence":0.91}
  ]
}"#,
            true,
        )
        .expect("write candidates");
        write_text(
            &tmp.path().join("memory/summaries/001.md"),
            "# 第 001 章记忆\n\n## 章节摘要\n- 林墨发现线索。\n",
            true,
        )
        .expect("write summary");
        write_text(
            &tmp.path().join("memory/facts.jsonl"),
            "{\"kind\":\"knowledge\",\"target\":\"注意**\",\"change\":\"候选记忆更新：target: 江鹤 change: 审稿建议\",\"confidence\":0.9}\n{\"kind\":\"knowledge\",\"target\":\"林墨\",\"change\":\"确认线索\",\"confidence\":0.8}\n",
            true,
        )
        .expect("write facts");

        let before = read_required(&candidate_path).expect("before");
        let report = cleanup_memory_workspace_from_workspace(tmp.path(), false).expect("cleanup");
        let after = read_required(&candidate_path).expect("after");

        assert!(report.contains("# Memory Cleanup"));
        assert!(report.contains("mode: dry-run"));
        assert!(report.contains("removed_candidates 1"));
        assert!(report.contains("removed_records 1"));
        assert!(report.contains("added_missing_sections"));
        assert_eq!(before, after);
        assert!(!tmp.path().join("memory/.cleanup").exists());
    }

    #[test]
    fn memory_cleanup_apply_filters_candidates_ledgers_and_summaries() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("清理应用测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("memory/candidates/001.json"),
            r#"{
  "chapter": 1,
  "candidates": [
    {"kind":"knowledge","target":"注意**","change":"target: 江鹤 change: 审稿建议","confidence":0.9},
    {"kind":"knowledge","target":"林墨","change":"确认陈岚隐藏事故线索","evidence":"chapters/001/final.md:9","confidence":0.91}
  ]
}"#,
            true,
        )
        .expect("write candidates");
        write_text(
            &tmp.path().join("memory/summaries/001.md"),
            "# 第 001 章记忆\n\n## 章节摘要\n- 林墨发现线索。\n",
            true,
        )
        .expect("write summary");
        write_text(
            &tmp.path().join("memory/facts.jsonl"),
            "{\"kind\":\"knowledge\",\"target\":\"注意**\",\"change\":\"候选记忆更新：target: 江鹤 change: 审稿建议\",\"confidence\":0.9}\n{\"kind\":\"knowledge\",\"target\":\"林墨\",\"change\":\"确认线索\",\"confidence\":0.8}\n",
            true,
        )
        .expect("write facts");

        let report = cleanup_memory_workspace_from_workspace(tmp.path(), true).expect("cleanup");

        assert!(report.contains("mode: apply"));
        assert!(report.contains("Backups"));
        assert!(report.contains("graph_rebuilt: ok: v2"));
        let candidates =
            read_required(&tmp.path().join("memory/candidates/001.json")).expect("candidates");
        assert!(candidates.contains("确认陈岚隐藏事故线索"));
        assert!(!candidates.contains("注意**"));
        let summary = read_required(&tmp.path().join("memory/summaries/001.md")).expect("summary");
        assert!(summary.contains("## CANDIDATE_MEMORY_UPDATES"));
        assert!(summary.contains("- none"));
        let facts = read_required(&tmp.path().join("memory/facts.jsonl")).expect("facts");
        assert!(facts.contains("确认线索"));
        assert!(!facts.contains("候选记忆更新"));
        assert!(tmp.path().join("memory/.cleanup").is_dir());
        assert!(tmp.path().join("memory/graph.json").is_file());
    }

    #[test]
    fn cite_material_appends_reference_only_section_to_brief() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("资料引用测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        let source = tmp.path().join("materials/sources/fire.md");
        write_text(
            &source,
            "# 消防资料\n\n旧城消防通道容易被临停车辆堵塞。",
            true,
        )
        .expect("write source");

        let message = cite_material_from_workspace(tmp.path(), &source, Some(2), None)
            .expect("cite material");

        assert!(message.contains("chapters/002/brief.md"));
        let brief = read_required(&tmp.path().join("chapters/002/brief.md")).expect("brief");
        assert!(brief.contains("Reference Material: materials/sources/fire.md"));
        assert!(brief.contains("canon_status: reference_only"));
        assert!(brief.contains("旧城消防通道"));
    }

    #[test]
    fn cite_material_rejects_non_material_sources() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("资料边界测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        let source = tmp.path().join("bible/world.md");

        let err = cite_material_from_workspace(tmp.path(), &source, Some(1), None)
            .expect_err("must reject bible as material source");

        assert!(err.to_string().contains("material source must be under"));
    }

    #[test]
    fn material_references_packet_groups_cited_sources() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("资料引用汇总测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        let source = tmp.path().join("materials/sources/fire.md");
        write_text(&source, "旧城消防通道容易被临停车辆堵塞。", true).expect("write source");
        cite_material_from_workspace(tmp.path(), &source, Some(3), None).expect("cite");
        cite_material_from_workspace(
            tmp.path(),
            &source,
            None,
            Some(Path::new("cards/world/fire_process.md")),
        )
        .expect("cite card");

        let packet = material_references_packet_from_workspace(tmp.path()).expect("packet");

        assert!(packet.contains("# Material References"));
        assert!(packet.contains("citations: 2"));
        assert!(packet.contains("unique sources: 1"));
        assert!(packet.contains("health: ok=2"));
        assert!(packet.contains("materials/sources/fire.md"));
        assert!(packet.contains("chapters/003/brief.md"));
        assert!(packet.contains("cards/world/fire_process.md"));
        assert!(packet.contains("source_health: ok"));
        assert!(packet.contains("reference_only"));
    }

    #[test]
    fn material_references_packet_reports_stale_and_missing_sources() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("资料引用健康测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        let stale = tmp.path().join("materials/sources/stale.md");
        let missing = tmp.path().join("materials/sources/missing.md");
        write_text(&stale, "旧资料", true).expect("write stale");
        write_text(&missing, "待删除资料", true).expect("write missing");
        cite_material_from_workspace(tmp.path(), &stale, Some(1), None).expect("cite stale");
        cite_material_from_workspace(tmp.path(), &missing, Some(2), None).expect("cite missing");
        write_text(&stale, "资料已经修改", true).expect("modify stale");
        std::fs::remove_file(&missing).expect("remove missing");

        let packet = material_references_packet_from_workspace(tmp.path()).expect("packet");

        assert!(packet.contains("stale_source=1"));
        assert!(packet.contains("missing_source=1"));
        assert!(packet.contains("source_health: stale_source"));
        assert!(packet.contains("source_health: missing_source"));
    }

    #[test]
    fn memory_schema_validation_reports_missing_edge_targets() {
        let graph = MemoryGraph {
            schema_version: 2,
            updated_at: "2026-05-14T00:00:00Z".to_string(),
            nodes: vec![MemoryNode {
                id: "character:lin_mo".to_string(),
                kind: "character".to_string(),
                label: "林墨".to_string(),
                source: "cards/characters/lin_mo.yaml".to_string(),
                summary: "主角".to_string(),
                state: serde_json::json!({}),
                hash: "hash".to_string(),
            }],
            edges: vec![MemoryEdge {
                kind: "KNOWS".to_string(),
                source: "character:lin_mo".to_string(),
                target: "secret:missing".to_string(),
                evidence: "memory/facts.jsonl:1".to_string(),
                confidence: 0.8,
                note: None,
            }],
            candidate_updates: Vec::new(),
        };

        let status = memory_schema_validation_status(&graph);

        assert!(status.contains("invalid"));
        assert!(status.contains("missing target"));
    }

    #[test]
    fn migrate_memory_workspace_writes_schema_files_and_candidate_metadata() {
        let tmp = TempDir::new().expect("temp dir");
        init_project(
            tmp.path(),
            Some("迁移测试".to_string()),
            None,
            None,
            80_000,
            "zh-CN".to_string(),
            false,
        )
        .expect("init project");
        write_text(
            &tmp.path().join("memory/candidates/legacy.json"),
            r#"{"chapter":1,"candidates":[{"kind":"knowledge","target":"林墨","change":"知道旧案线索","evidence":"analysis","confidence":0.8}]}"#,
            true,
        )
        .expect("legacy candidate");
        std::fs::remove_file(tmp.path().join("memory/graph.schema.json")).expect("remove schema");

        let report = migrate_memory_workspace_from_workspace(tmp.path()).expect("migrate");

        assert!(report.contains("# Memory Migration"));
        assert!(report.contains("migrated candidate files: 1"));
        assert!(tmp.path().join("memory/graph.schema.json").is_file());
        let candidate =
            read_required(&tmp.path().join("memory/candidates/legacy.json")).expect("candidate");
        assert!(candidate.contains("\"schema_version\": 1"));
        assert!(candidate.contains("\"status\": \"pending_review\""));
        assert!(candidate.contains("\"status\": \"candidate\""));
    }
}

const MEMORY_SCHEMA_DOC: &str = r#"# Memory Graph Schema

`memory/graph.json` is a generated narrative memory graph. Do not edit it by hand; edit `bible/`, `cards/`, `outline/`, chapters, or confirmed memory ledgers, then rebuild.

## Current Version

- `schema_version`: `2`
- Stability: additive changes are allowed inside `state` and `note`; existing top-level keys keep their meaning for v2 readers.

## Top-Level Shape

```json
{
  "schema_version": 2,
  "updated_at": "RFC3339 timestamp",
  "nodes": [],
  "edges": [],
  "candidate_updates": []
}
```

## Node Contract

Each node has:

- `id`: stable graph id such as `character:林墨`, `chapter:001`, `promise:事故真相`.
- `kind`: one of `book`, `chapter`, `character`, `location`, `world`, `outline`, `memory`, `knowledge`, `secret`, `promise`, `event`, `relationship`, `state`, `object`, or compatible future values.
- `label`: human-readable name.
- `source`: project-relative source path or generated source id.
- `summary`: compact evidence summary.
- `state`: JSON object for semantic payloads such as `knowledge`, `unknown`, `secret`, `status`, `chapter`, `first_chapter`, `payoff_chapter`, `participants`, `target`, or `confidence`.
- `hash`: stable content hash used for change detection.

## Edge Contract

Each edge has:

- `kind`: semantic relation. Core v2 kinds include `CONTAINS`, `NEXT`, `AFFECTS`, `KNOWS`, `DOES_NOT_KNOW`, `PROMISES`, `CAUSES`, and `CHANGES`.
- `source` / `target`: node ids.
- `evidence`: source path, chapter, or ledger line reference.
- `confidence`: `0.0` to `1.0`.
- `note`: optional explanation.

## Candidate Updates

`memory/candidates/*.json` files use `schema_version: 1` and remain reviewable. Durable ledgers are written only by `memory apply`.

## Character Behavior Ledger

`memory/behavior.jsonl` stores applied character behavior records derived from reviewed candidates:

```json
{"chapter":1,"character":"林墨","situation":"chapters/001/final.md","choice":"藏起证据","result":"memory candidate applied as knowledge","evidence":"chapters/001/final.md:8","source":"memory_candidate"}
```

The ledger is injected before drafting so character continuity is grounded in recent choices rather than abstract personality tags.
"#;

const MEMORY_GRAPH_JSON_SCHEMA: &str = r##"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://deepseek.local/novel-studio/memory-graph.schema.json",
  "title": "Writer Memory Graph",
  "type": "object",
  "required": ["schema_version", "updated_at", "nodes", "edges"],
  "properties": {
    "schema_version": { "const": 2 },
    "updated_at": { "type": "string" },
    "nodes": {
      "type": "array",
      "items": { "$ref": "#/$defs/node" }
    },
    "edges": {
      "type": "array",
      "items": { "$ref": "#/$defs/edge" }
    },
    "candidate_updates": {
      "type": "array",
      "items": { "$ref": "#/$defs/candidate" }
    }
  },
  "additionalProperties": true,
  "$defs": {
    "node": {
      "type": "object",
      "required": ["id", "kind", "label", "source", "summary", "hash"],
      "properties": {
        "id": { "type": "string", "minLength": 1 },
        "kind": { "type": "string", "minLength": 1 },
        "label": { "type": "string", "minLength": 1 },
        "source": { "type": "string" },
        "summary": { "type": "string" },
        "state": true,
        "hash": { "type": "string" }
      },
      "additionalProperties": true
    },
    "edge": {
      "type": "object",
      "required": ["kind", "source", "target", "evidence", "confidence"],
      "properties": {
        "kind": { "type": "string", "minLength": 1 },
        "source": { "type": "string", "minLength": 1 },
        "target": { "type": "string", "minLength": 1 },
        "evidence": { "type": "string" },
        "confidence": { "type": "number", "minimum": 0, "maximum": 1 },
        "note": { "type": ["string", "null"] }
      },
      "additionalProperties": true
    },
    "candidate": {
      "type": "object",
      "required": ["kind", "target", "change", "evidence", "confidence"],
      "properties": {
        "chapter": { "type": ["integer", "null"], "minimum": 1 },
        "kind": { "type": "string" },
        "target": { "type": "string" },
        "change": { "type": "string" },
        "evidence": { "type": "string" },
        "confidence": { "type": "number", "minimum": 0, "maximum": 1 },
        "affects": { "type": "array", "items": { "type": "string" } },
        "status": { "type": "string" },
        "applied_at": { "type": "string" }
      },
      "additionalProperties": true
    }
  }
}
"##;

const NOVEL_SYSTEM_PROMPT: &str = r#"你是 Writer 的长篇小说创作引擎，专门服务中长篇和超长篇中文小说生产。

你的工作不是聊天补句，而是维护一本书的长期工程：设定、人物、章节、伏笔、事实锁、连续性、文风和读者期待。

硬性规则：
- 以项目文件为真源。不得无视已经给出的世界观、人物状态、前文和风格契约。
- 写正文时只输出可直接保存的正文，不解释创作过程。
- 叙事优先使用场景、行动、对话和选择呈现，少用抽象总结。
- 像作者一样先判断人物在此刻想要什么、怕失去什么、藏着什么、误判了什么，再让事件发生。
- 保持人物欲望、信息边界、因果链和情绪弧线稳定。
- 每场戏至少改变一个东西：权力、信息、关系、资源、身体状态、公开立场或角色选择。
- 避免模板化 AI 腔：不要机械使用“然而/与此同时/值得注意的是/总之”等路标词，不要用空泛形容堆叠替代具体动作，不要用总结升华替代余味。
- 如果资料不足，做保守补全，并让补全与已有设定兼容。
"#;

const EMPOWER_SYSTEM_PROMPT: &str = r#"你是 Writer 的记忆辅助写作编辑，负责把章节目标转成可选写法札记。

你的输出不是审查模板，也不是正文生成流水线。你的职责是帮助作者看见本章会触碰的长期记忆、人物可能性和叙事风险，同时保留正文临场生成的自由。

硬性规则：
- 只输出可保存的 Markdown 札记，不解释你的方法。
- 不给固定评分表，不要求每章套同一组动作。
- 每条建议都必须服务长期记忆、人物可信度或当前场景可能性。
- 尊重既有设定、简报、人物信息边界和读者承诺。
"#;

const EDITOR_SYSTEM_PROMPT: &str = r#"你是 Writer 的长篇记忆诊断员。

你专门维护长篇小说的记忆一致性：人物状态、时间线、设定规则、地点状态、物件归属、伏笔状态、信息边界和章节后果。

审查不是把正文套成固定模板。只指出会破坏连续性、人物可信度或后续记忆管理的问题，并给出可写入记忆图的候选更新。
"#;

const DEFAULT_HUMAN_TEXTURE_GUIDE: &str = r#"# 人味正文赋能规则

这份文件控制“怎么写”，不是控制“写什么”。它的目标是让正文像作者完成的场景，而不是模型按提示词摊开的说明。

## 场景动力

- 每个主要场景都要有一个即时目标：人物此刻想拿到、避开、确认、隐瞒或改变什么。
- 阻力必须具体：另一个人的欲望、规则限制、资源不足、时间压力、身体代价、误判或公开身份。
- 场景结束时至少改变一项：信息、关系、权力、资源、伤病、位置、公开立场、下一步选择。
- 如果一段文字没有目标、阻力和变化，它通常应该被压缩、删掉，或改成行动中的信息。

## 人物像人

- 人物先有欲望、恐惧、体面和自我欺骗，再有台词。
- 让人物保留遮掩：嘴上说的、手上做的、心里避开的可以不一致。
- 不要让角色替作者解释世界观；让角色为了赢、逃、试探、掩饰或交易而说话。
- 情绪优先外化为动作、停顿、身体反应、物件处理和环境选择。

## 对话

- 关键对话必须改变权力、信息、关系或选择。
- 台词要有目的：试探、回避、压迫、讨价还价、承认、威胁、转移、求证。
- 避免角色轮流讲背景资料；设定信息要藏在误解、冲突、代价或交易里。

## 设定入戏

- 世界规则要通过阻碍、误用、代价、漏洞、惩罚或交换进入正文。
- 能力、制度、势力和物件不应只被介绍；它们要改变人物选择。
- 新设定出现时，要给读者一个可感知后果，而不是一段说明书。

## 节奏与余味

- 长段之后用短句、动作或对话换气。
- 连续解释超过一段时，插入角色选择或场景压力。
- 章末要同时留下“已经发生的后果”和“尚未解决的问题”。
- 余味来自具体变化，不来自作者替读者总结意义。
"#;

const DEFAULT_ANTI_AI_PATTERNS: &str = r#"# 反 AI 腔清单

## 高风险写法

- 用“他心中涌起一种难以言说的情绪”替代具体反应。
- 用“眼神复杂”“嘴角微微上扬”“空气仿佛凝固”这类泛化动作制造氛围。
- 用“然而/与此同时/更重要的是/总之/这一刻”机械转场。
- 段落长度过于平均，每段都像同一套句式展开。
- 角色轮流解释背景，台词结束后局面没有变化。
- 冲突结束后一切照旧，没有代价、信息差或选择后果。
- 章末只悬空设问，没有兑现本章事件的后果。
- 频繁使用“不是因为……而是因为……”“不仅仅是……更是……”来替作者解释意义。
- 连续三联排抽象词：恐惧、愤怒、痛苦；权力、信息、关系；命运、真相、秘密。
- 为了显得不重复而机械轮换称谓：主角/少年/他/这个承厄户/林墨来回切换，反而削弱视角贴近。
- 用“这意味着/这说明/这象征/真正重要的是”替读者总结。
- 章节结尾用空泛宣言收束，而不是用具体余波、损失、关系变化或下一步阻碍收束。

## 替代原则

- 情绪词换成动作：拿杯子的力度、停顿、回避视线、改口、整理物件、脚步方向。
- 氛围词换成场景压力：时间限制、旁人目光、规则惩罚、身体不适、资源损耗。
- 转场词换成因果：上一句的选择直接带出下一句的后果。
- 解释设定换成误用设定、违反设定、利用设定或为设定付代价。
- 总结升华换成一个具体余波：某人闭嘴、某物损坏、关系变冷、选择被迫提前。
- 三联排改成一个能改变局面的具体动作或物件状态。
- 称谓保持贴近当前视角；除非人物关系或视角距离改变，不要为了避重复机械换称。
"#;

const EVAL_README: &str = r#"# Evaluation Fixtures

This directory stores small regression fixtures for deterministic quality signals.

Rules:

- Fixtures must be original project text or user-authorized local material.
- Do not copy external novel passages into this repository.
- A fixture proves that a signal can catch one defined failure mode; it does not prove real reader quality or million-word stability.
- True long-run validation happens only after development is complete.
"#;

const EVAL_RUBRICS_README: &str = r#"# Quality Signal Rubrics

Use these rubrics to classify small fixtures before running real writing experiments.

- Continuity: character state, timeline, location state, object/resource ownership, and canon conflicts.
- Knowledge boundary: what each character can know at this chapter.
- Reader promise: promise status, progression, payoff, suspension, or drift.
- Xianxia resource economy: value anchor, income equivalent, control party, use cost, debt, or sect obligation.
- Craft texture: abstract emotion density, dialogue voice difference, combat observation-to-reversal chain, and worldbuilding through action.
- Revision fidelity: targeted revision fixes the top issues without washing away working voice and scene order.
"#;

const EXPERIMENTS_README: &str = r#"# Long-Run Experiment Scaffold

This directory records reproducible writing experiments after development is complete.

Capture for each run:

- model, temperature, skill package, genre, target words, memory configuration, and command chain.
- per-chapter ContextQualityReport, ChapterQualityReport, audit.md, memory candidates, and revision diff.
- suspected collapse points: character drift, promise breakage, resource without cost, knowledge leak, revision voice loss, and context growth.
- baseline gate status from `experiments/baselines/long_form_acceptance.md`.

Do not treat one run as a final capability claim.
"#;

const LONG_FORM_ACCEPTANCE_BASELINE: &str = r#"# Long-Form Acceptance Baseline

This baseline defines when a Writer long-form run is evidence-bearing. It is not
a guarantee of reader quality and it is not satisfied by a single short smoke
test.

## Required Runs

- 10 chapters: smoke plus failure triage.
- 30 chapters: short-book continuity and cost profile.
- 50 chapters: late-context noise and memory-pressure profile.

Use 3500 target words per chapter unless the experiment plan records another
target.

## Required Artifacts

Every measured run must keep:

- `experiments/configs/<run_id>.json`
- `experiments/reports/<snapshot>.json`
- `memory/reports/` regression reports for every 10-chapter window
- per-chapter `ContextQualityReport` and `ChapterQualityReport`
- per-chapter `audit.md`
- per-chapter `memory/summaries/NNN.md`
- reviewed or explicitly deferred `memory/candidates/NNN.json`

## Pass Gates

- No context-quality blocker before drafting unless the run explicitly records
  `--allow-degraded-context` and explains why.
- Every completed chapter has a summary, audit, and memory candidate decision.
- 10-chapter regression reports do not show unresolved character knowledge
  leaks, promise breakage, or timeline contradictions.
- Recent memory summaries stay below the configured overweight threshold unless
  the run records a compaction action.
- Pending candidate pressure is reviewed before the next 10-chapter window.
- Revision does not erase the accepted scene order, point of view, or working
  voice without a recorded reason.
- Chapter opening/ending bridge issues trend down after targeted revision.

## Metrics To Report

- completed chapters / attempted chapters
- average seconds per chapter stage: brief, write, audit, revise, remember
- generated words per chapter
- context chars per chapter and prompt chars per chapter
- blocker and major signal counts by code
- pending candidate count and max candidates per chapter
- summary max chars and canon-sparse summary count
- memory graph nodes, edges, and schema status
- chapter bridge opening count
- revision voice loss count

Only make a capability claim from measured runs that include the artifacts above.
"#;

const LEADERBOARD_README: &str = r#"# Experiment Leaderboard

Store structured comparisons of completed experiments here.

Compare workflows only after the engineering stages are done, for example:

- no memory
- memory graph
- memory graph plus archive
- targeted revise
- xianxia craft package

Rows should cite run ids and report paths rather than making unsupported claims.
"#;
