//! Slash command registry and dispatch system
//!
//! This module provides a modular command system inspired by Codex-rs.
//! Commands are organized by category and dispatched through a central registry.

mod anchor;
mod attachment;
mod change;
mod config;
mod core;
mod cycle;
mod debug;
mod feedback;
mod goal;
mod hooks;
mod init;
mod jobs;
mod mcp;
mod memory;
mod network;
mod note;
mod provider;
mod queue;
mod rename;
mod restore;
mod review;
mod session;
pub mod share;
mod skills;
mod stash;
mod status;
mod task;
mod user_commands;

use std::fmt::Write as _;

use crate::localization::{Locale, MessageId, tr};
use crate::novel;
use crate::tui::app::{App, AppAction};

/// Result of executing a command
#[derive(Debug, Clone)]
pub struct CommandResult {
    /// Optional message to display to the user
    pub message: Option<String>,
    /// Optional action for the app to take
    pub action: Option<AppAction>,
    /// Whether the command failed.
    pub is_error: bool,
}

impl CommandResult {
    /// Create an empty result (command succeeded with no output)
    pub fn ok() -> Self {
        Self {
            message: None,
            action: None,
            is_error: false,
        }
    }

    /// Create a result with just a message
    pub fn message(msg: impl Into<String>) -> Self {
        Self {
            message: Some(msg.into()),
            action: None,
            is_error: false,
        }
    }

    /// Create a result with an action
    pub fn action(action: AppAction) -> Self {
        Self {
            message: None,
            action: Some(action),
            is_error: false,
        }
    }

    /// Create a result with both message and action
    #[allow(dead_code)]
    pub fn with_message_and_action(msg: impl Into<String>, action: AppAction) -> Self {
        Self {
            message: Some(msg.into()),
            action: Some(action),
            is_error: false,
        }
    }

    /// Create an error message result
    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            message: Some(format!("Error: {}", msg.into())),
            action: None,
            is_error: true,
        }
    }
}

/// Command metadata for help and autocomplete.
///
/// The English description lives in `localization::english` (private), keyed
/// by `description_id`. Callers resolve a localized description through
/// [`CommandInfo::description_for`] which delegates to
/// [`crate::localization::tr`].
#[derive(Debug, Clone, Copy)]
pub struct CommandInfo {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub usage: &'static str,
    pub description_id: MessageId,
}

impl CommandInfo {
    pub fn requires_argument(&self) -> bool {
        self.usage.contains('<') || self.usage.contains('[')
    }

    pub fn palette_command(&self) -> String {
        if self.requires_argument() {
            format!("/{} ", self.name)
        } else {
            format!("/{}", self.name)
        }
    }

    pub fn description_for(&self, locale: Locale) -> &'static str {
        tr(locale, self.description_id)
    }

    pub fn palette_description_for(&self, locale: Locale) -> String {
        let desc = self.description_for(locale);
        if self.aliases.is_empty() {
            desc.to_string()
        } else {
            format!("{}  aliases: {}", desc, self.aliases.join(", "))
        }
    }
}

/// All registered commands
pub const COMMANDS: &[CommandInfo] = &[
    // Core commands
    CommandInfo {
        name: "anchor",
        aliases: &[],
        usage: "/anchor <text> | /anchor list | /anchor remove <n>",
        description_id: MessageId::CmdAnchorDescription,
    },
    CommandInfo {
        name: "help",
        aliases: &["?"],
        usage: "/help [command]",
        description_id: MessageId::CmdHelpDescription,
    },
    CommandInfo {
        name: "clear",
        aliases: &[],
        usage: "/clear",
        description_id: MessageId::CmdClearDescription,
    },
    CommandInfo {
        name: "exit",
        aliases: &["quit", "q"],
        usage: "/exit",
        description_id: MessageId::CmdExitDescription,
    },
    CommandInfo {
        name: "model",
        aliases: &[],
        usage: "/model [name]",
        description_id: MessageId::CmdModelDescription,
    },
    CommandInfo {
        name: "models",
        aliases: &[],
        usage: "/models",
        description_id: MessageId::CmdModelsDescription,
    },
    CommandInfo {
        name: "provider",
        aliases: &[],
        usage: "/provider [name]",
        description_id: MessageId::CmdProviderDescription,
    },
    CommandInfo {
        name: "queue",
        aliases: &["queued"],
        usage: "/queue [list|edit <n>|drop <n>|clear]",
        description_id: MessageId::CmdQueueDescription,
    },
    CommandInfo {
        name: "stash",
        aliases: &["park"],
        usage: "/stash [list|pop|clear]",
        description_id: MessageId::CmdStashDescription,
    },
    CommandInfo {
        name: "hooks",
        aliases: &["hook"],
        usage: "/hooks [list|events]",
        description_id: MessageId::CmdHooksDescription,
    },
    CommandInfo {
        name: "subagents",
        aliases: &["agents"],
        usage: "/subagents",
        description_id: MessageId::CmdSubagentsDescription,
    },
    CommandInfo {
        name: "agent",
        aliases: &[],
        usage: "/agent [N] <task>",
        description_id: MessageId::CmdAgentDescription,
    },
    CommandInfo {
        name: "links",
        aliases: &["dashboard", "api"],
        usage: "/links",
        description_id: MessageId::CmdLinksDescription,
    },
    CommandInfo {
        name: "feedback",
        aliases: &[],
        usage: "/feedback [bug|feature|security]",
        description_id: MessageId::CmdFeedbackDescription,
    },
    CommandInfo {
        name: "home",
        aliases: &["stats", "overview"],
        usage: "/home",
        description_id: MessageId::CmdHomeDescription,
    },
    CommandInfo {
        name: "map",
        aliases: &["project-map"],
        usage: "/map [--full]",
        description_id: MessageId::CmdStatusDescription,
    },
    CommandInfo {
        name: "note",
        aliases: &[],
        usage: "/note [add|list|show|edit|remove|clear|path]",
        description_id: MessageId::CmdNoteDescription,
    },
    CommandInfo {
        name: "memory",
        aliases: &[],
        usage: "/memory [status|build|reports|promises|archive|regression|validate|migrate|cleanup|context <chapter>|query <text>|impact <chapter>|candidates|apply|import-analysis|import-mcp|cite-material|references|help]",
        description_id: MessageId::CmdMemoryDescription,
    },
    CommandInfo {
        name: "analyze",
        aliases: &["manuscript"],
        usage: "/analyze [N] <focus>",
        description_id: MessageId::CmdRlmDescription,
    },
    CommandInfo {
        name: "plan",
        aliases: &["outline"],
        usage: "/plan [direction]",
        description_id: MessageId::CmdPlanDescription,
    },
    CommandInfo {
        name: "brief",
        aliases: &[],
        usage: "/brief <chapter> [direction]",
        description_id: MessageId::CmdBriefDescription,
    },
    CommandInfo {
        name: "empower",
        aliases: &["craft"],
        usage: "/empower <chapter> [direction]",
        description_id: MessageId::CmdEmpowerDescription,
    },
    CommandInfo {
        name: "write",
        aliases: &["draft"],
        usage: "/write <chapter> [direction]",
        description_id: MessageId::CmdWriteDescription,
    },
    CommandInfo {
        name: "audit",
        aliases: &["diagnose"],
        usage: "/audit <chapter>",
        description_id: MessageId::CmdAuditDescription,
    },
    CommandInfo {
        name: "revise",
        aliases: &[],
        usage: "/revise <chapter> [direction]",
        description_id: MessageId::CmdReviseDescription,
    },
    CommandInfo {
        name: "chapter-diff",
        aliases: &["cdiff"],
        usage: "/chapter-diff <chapter>",
        description_id: MessageId::CmdChapterDiffDescription,
    },
    CommandInfo {
        name: "chapter-undo",
        aliases: &["cundo"],
        usage: "/chapter-undo <chapter>",
        description_id: MessageId::CmdChapterUndoDescription,
    },
    CommandInfo {
        name: "remember",
        aliases: &[],
        usage: "/remember <chapter>",
        description_id: MessageId::CmdRememberDescription,
    },
    CommandInfo {
        name: "attach",
        aliases: &["image", "media"],
        usage: "/attach <path>",
        description_id: MessageId::CmdAttachDescription,
    },
    CommandInfo {
        name: "task",
        aliases: &["tasks"],
        usage: "/task [add <prompt>|list|show <id>|cancel <id>]",
        description_id: MessageId::CmdTaskDescription,
    },
    CommandInfo {
        name: "jobs",
        aliases: &["job"],
        usage: "/jobs [list|show <id>|poll <id>|wait <id>|stdin <id> <input>|cancel <id>]",
        description_id: MessageId::CmdJobsDescription,
    },
    CommandInfo {
        name: "mcp",
        aliases: &[],
        usage: "/mcp [init|add stdio <name> <command> [args...]|add http <name> <url>|enable <name>|disable <name>|remove <name>|validate|reload]",
        description_id: MessageId::CmdMcpDescription,
    },
    CommandInfo {
        name: "network",
        aliases: &[],
        usage: "/network [list|allow <host>|deny <host>|remove <host>|default <allow|deny|prompt>]",
        description_id: MessageId::CmdNetworkDescription,
    },
    // Session commands
    CommandInfo {
        name: "rename",
        aliases: &[],
        usage: "/rename <new title>",
        description_id: MessageId::CmdRenameDescription,
    },
    CommandInfo {
        name: "save",
        aliases: &[],
        usage: "/save [path]",
        description_id: MessageId::CmdSaveDescription,
    },
    CommandInfo {
        name: "sessions",
        aliases: &["resume"],
        usage: "/sessions [show|prune <days>]",
        description_id: MessageId::CmdSessionsDescription,
    },
    CommandInfo {
        name: "load",
        aliases: &[],
        usage: "/load [path]",
        description_id: MessageId::CmdLoadDescription,
    },
    CommandInfo {
        name: "compact",
        aliases: &[],
        usage: "/compact",
        description_id: MessageId::CmdCompactDescription,
    },
    CommandInfo {
        name: "relay",
        aliases: &["batonpass", "接力"],
        usage: "/relay [focus]",
        description_id: MessageId::CmdRelayDescription,
    },
    CommandInfo {
        name: "context",
        aliases: &["ctx"],
        usage: "/context",
        description_id: MessageId::CmdContextDescription,
    },
    CommandInfo {
        name: "cycles",
        aliases: &[],
        usage: "/cycles",
        description_id: MessageId::CmdCyclesDescription,
    },
    CommandInfo {
        name: "cycle",
        aliases: &[],
        usage: "/cycle <n>",
        description_id: MessageId::CmdCycleDescription,
    },
    CommandInfo {
        name: "recall",
        aliases: &[],
        usage: "/recall <query>",
        description_id: MessageId::CmdRecallDescription,
    },
    CommandInfo {
        name: "export",
        aliases: &[],
        usage: "/export [path]",
        description_id: MessageId::CmdExportDescription,
    },
    // Config commands
    CommandInfo {
        name: "config",
        aliases: &[],
        usage: "/config",
        description_id: MessageId::CmdConfigDescription,
    },
    CommandInfo {
        name: "mode",
        aliases: &[],
        usage: "/mode [agent|plan|yolo|1|2|3]",
        description_id: MessageId::CmdModeDescription,
    },
    CommandInfo {
        name: "theme",
        aliases: &[],
        usage: "/theme [dark|light|grayscale|system]",
        description_id: MessageId::CmdThemeDescription,
    },
    CommandInfo {
        name: "verbose",
        aliases: &[],
        usage: "/verbose [on|off]",
        description_id: MessageId::CmdVerboseDescription,
    },
    CommandInfo {
        name: "trust",
        aliases: &[],
        usage: "/trust [on|off|add <path>|remove <path>|list]",
        description_id: MessageId::CmdTrustDescription,
    },
    CommandInfo {
        name: "logout",
        aliases: &[],
        usage: "/logout",
        description_id: MessageId::CmdLogoutDescription,
    },
    // Debug commands
    CommandInfo {
        name: "tokens",
        aliases: &[],
        usage: "/tokens",
        description_id: MessageId::CmdTokensDescription,
    },
    CommandInfo {
        name: "translate",
        aliases: &["translation", "transale"],
        usage: "/translate",
        description_id: MessageId::CmdTranslateDescription,
    },
    CommandInfo {
        name: "system",
        aliases: &[],
        usage: "/system",
        description_id: MessageId::CmdSystemDescription,
    },
    CommandInfo {
        name: "edit",
        aliases: &[],
        usage: "/edit",
        description_id: MessageId::CmdEditDescription,
    },
    CommandInfo {
        name: "change",
        aliases: &[],
        usage: "/change",
        description_id: MessageId::CmdChangeDescription,
    },
    CommandInfo {
        name: "retry",
        aliases: &[],
        usage: "/retry",
        description_id: MessageId::CmdRetryDescription,
    },
    CommandInfo {
        name: "init",
        aliases: &[],
        usage: "/init",
        description_id: MessageId::CmdInitDescription,
    },
    CommandInfo {
        name: "lsp",
        aliases: &[],
        usage: "/lsp [on|off|status]",
        description_id: MessageId::CmdLspDescription,
    },
    CommandInfo {
        name: "share",
        aliases: &[],
        usage: "/share",
        description_id: MessageId::CmdShareDescription,
    },
    CommandInfo {
        name: "goal",
        aliases: &[],
        usage: "/goal [objective] [budget: N]",
        description_id: MessageId::CmdGoalDescription,
    },
    CommandInfo {
        name: "settings",
        aliases: &[],
        usage: "/settings",
        description_id: MessageId::CmdSettingsDescription,
    },
    CommandInfo {
        name: "status",
        aliases: &[],
        usage: "/status",
        description_id: MessageId::CmdStatusDescription,
    },
    CommandInfo {
        name: "statusline",
        aliases: &[],
        usage: "/statusline",
        description_id: MessageId::CmdStatuslineDescription,
    },
    // Skills commands
    CommandInfo {
        name: "skills",
        aliases: &[],
        usage: "/skills [--remote|sync|<prefix>]",
        description_id: MessageId::CmdSkillsDescription,
    },
    CommandInfo {
        name: "skill",
        aliases: &[],
        usage: "/skill <name|install <spec>|update <name>|uninstall <name>|trust <name>>",
        description_id: MessageId::CmdSkillDescription,
    },
    CommandInfo {
        name: "restore",
        aliases: &[],
        usage: "/restore [N]",
        description_id: MessageId::CmdRestoreDescription,
    },
    // RLM command
    CommandInfo {
        name: "rlm",
        aliases: &["recursive"],
        usage: "/rlm [N] <file_or_text>",
        description_id: MessageId::CmdRlmDescription,
    },
    // Debug/cost command
    CommandInfo {
        name: "cost",
        aliases: &[],
        usage: "/cost",
        description_id: MessageId::CmdCostDescription,
    },
    // Profile switching (#390)
    CommandInfo {
        name: "profile",
        aliases: &[],
        usage: "/profile <name>",
        description_id: MessageId::CmdHelpDescription, // reuse for now
    },
    // Cache telemetry (#263)
    CommandInfo {
        name: "cache",
        aliases: &[],
        usage: "/cache [count|inspect|warmup]",
        description_id: MessageId::CmdCacheDescription,
    },
];

/// Execute a slash command
pub fn execute(cmd: &str, app: &mut App) -> CommandResult {
    let parts: Vec<&str> = cmd.trim().splitn(2, ' ').collect();
    let command = parts[0].to_lowercase();
    let command = command.strip_prefix('/').unwrap_or(&command);
    let arg = parts.get(1).map(|s| s.trim());

    // Check user-defined commands FIRST so they can override built-ins.
    if let Some(result) = user_commands::try_dispatch_user_command(app, cmd.trim()) {
        return result;
    }

    // Match command or alias
    match command {
        // Core commands
        "anchor" => anchor::anchor(app, arg),
        "help" | "?" => core::help(app, arg),
        "clear" => core::clear(app),
        "exit" | "quit" | "q" => core::exit(),
        "model" => core::model(app, arg),
        "models" => core::models(app),
        "provider" => provider::provider(app, arg),
        "queue" | "queued" => queue::queue(app, arg),
        "stash" | "park" => stash::stash(app, arg),
        "hooks" | "hook" => hooks::hooks(app, arg),
        "subagents" | "agents" => core::subagents(app),
        "agent" => agent(app, arg),
        "links" | "dashboard" | "api" => core::deepseek_links(app),
        "feedback" => feedback::feedback(app, arg),
        "home" | "stats" | "overview" => core::home_dashboard(app),
        "map" | "project-map" => novel_project_map(app, arg),
        "note" => note::note(app, arg),
        "memory" => memory::memory(app, arg),
        "analyze" | "manuscript" => novel_analyze(app, arg),
        "plan" | "outline" => novel_plan(app, arg),
        "brief" => novel_chapter_command(app, "brief", arg),
        "empower" | "craft" => novel_chapter_command(app, "empower", arg),
        "write" | "draft" => novel_chapter_command(app, "write", arg),
        "audit" | "diagnose" => novel_chapter_command(app, "audit", arg),
        "revise" => novel_chapter_command(app, "revise", arg),
        "chapter-diff" | "cdiff" => novel_chapter_diff(app, arg),
        "chapter-undo" | "cundo" => novel_chapter_undo(app, arg),
        "remember" => novel_chapter_command(app, "remember", arg),
        "attach" | "image" | "media" => attachment::attach(app, arg),
        "task" | "tasks" => task::task(app, arg),
        "jobs" | "job" => jobs::jobs(app, arg),
        "mcp" => mcp::mcp(app, arg),
        "network" => network::network(app, arg),

        // Session commands
        "rename" => rename::rename(app, arg),
        "save" => session::save(app, arg),
        "sessions" | "resume" => session::sessions(app, arg),
        "load" => session::load(app, arg),
        "compact" => session::compact(app),
        "relay" | "batonpass" | "接力" => relay(app, arg),
        "cycles" => cycle::list_cycles(app),
        "cycle" => cycle::show_cycle(app, arg),
        "recall" => cycle::recall_archive(app, arg),
        "export" => session::export(app, arg),

        // Config commands
        "config" => config::config_command(app, arg),
        "settings" => config::show_settings(app),
        "status" => status::status(app),
        "statusline" => config::status_line(app),
        "mode" => config::mode(app, arg),
        "theme" => config::theme(app, arg),
        "verbose" => config::verbose(app, arg),
        "trust" => config::trust(app, arg),
        "logout" => config::logout(app),

        // Debug commands
        "translate" | "translation" | "transale" => core::translate(app),
        "tokens" => debug::tokens(app),
        "cost" => debug::cost(app),
        "cache" => debug::cache(app, arg),

        // ChangeLog command
        "change" => change::change(app),
        "system" => debug::system_prompt(app),
        "context" | "ctx" => debug::context(app),
        "edit" => debug::edit(app),
        "diff" => debug::diff(app),
        "undo" => {
            // Try surgical patch-undo first; fall back to conversation undo
            // if no snapshots are available or if the snapshot undo couldn't
            // find anything useful.
            let result = debug::patch_undo(app);
            if result.message.as_deref().is_none_or(|m| {
                m.starts_with("No snapshots found")
                    || m.starts_with("No tool or pre-turn")
                    || m.starts_with("Snapshot repo")
            }) {
                debug::undo_conversation(app)
            } else {
                result
            }
        }
        "retry" => debug::retry(app),

        // Project commands
        "init" => init::init(app),
        "lsp" => config::lsp_command(app, arg),
        "share" => share::share(app, arg),
        "goal" => goal::goal(app, arg),

        // Skills commands
        "skills" => skills::list_skills(app, arg),
        "skill" => skills::run_skill(app, arg),
        "review" => review::review(app, arg),
        "restore" => restore::restore(app, arg),

        // Profile switch (#390)
        "profile" => core::profile_switch(app, arg),

        // RLM command
        "rlm" | "recursive" => rlm(app, arg),

        // Legacy command migrations (kept out of registry/autocomplete intentionally).
        "set" => CommandResult::error(
            "The /set command was retired. Use /config to edit settings and /settings to inspect current values.",
        ),
        "deepseek" => CommandResult::error(
            "The /deepseek command was renamed. Use /links (aliases: /dashboard, /api).",
        ),

        _ => {
            // Third source: skills (lowest precedence after native and user-config).
            // Try to run a skill whose name matches the command.
            if skills::run_skill_by_name(app, command, arg).is_some() {
                return skills::run_skill_by_name(app, command, arg).unwrap();
            }
            let suggestions = suggest_command_names(command, 3);
            if suggestions.is_empty() {
                CommandResult::error(format!(
                    "Unknown command: /{command}. Type /help for available commands."
                ))
            } else {
                let list = suggestions
                    .into_iter()
                    .map(|name| format!("/{name}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                CommandResult::error(format!(
                    "Unknown command: /{command}. Did you mean: {list}? Type /help for available commands."
                ))
            }
        }
    }
}

/// Update a configuration value programmatically (used by interactive UI views).
pub fn set_config_value(app: &mut App, key: &str, value: &str, persist: bool) -> CommandResult {
    config::set_config_value(app, key, value, persist)
}

/// Persist the user's chosen footer items to `~/.deepseek/config.toml` under
/// `tui.status_items`. See [`config::persist_status_items`] for details.
pub fn persist_status_items(
    items: &[crate::config::StatusItem],
) -> anyhow::Result<std::path::PathBuf> {
    config::persist_status_items(items)
}

/// Persist a root-level string key in `config.toml`.
pub fn persist_root_string_key(key: &str, value: &str) -> anyhow::Result<std::path::PathBuf> {
    config::persist_root_string_key(key, value)
}

pub fn switch_mode(app: &mut App, mode: crate::tui::app::AppMode) -> String {
    config::switch_mode(app, mode)
}

/// Auto-select a model based on request complexity.
pub fn auto_model_heuristic(input: &str, current_model: &str) -> String {
    config::auto_model_heuristic(input, current_model)
}

pub use config::{
    AutoRouteRecommendation, AutoRouteSelection, normalize_auto_route_effort,
    parse_auto_route_recommendation, resolve_auto_route_with_flash,
};

/// Execute a Recursive Language Model (RLM) turn — Algorithm 1 from
/// Zhang et al. (arXiv:2512.24601).
///
/// The user's prompt text is passed as the argument. It will be stored
/// in the REPL as the `PROMPT` variable. The root LLM will only see
/// metadata about the REPL state, never the prompt text directly.
pub fn rlm(app: &mut App, arg: Option<&str>) -> CommandResult {
    let (max_depth, target) = match parse_depth_prefixed_arg(arg, 1) {
        Ok(parsed) => parsed,
        Err(message) => return CommandResult::error(message),
    };
    let target = match target {
        Some(p) if !p.trim().is_empty() => p.trim().to_string(),
        _ => {
            return CommandResult::error(
                "Usage: /rlm [N] <file_or_text>\n\n\
                 Opens a persistent RLM context with sub_rlm depth N (0-3, default 1)."
                    .to_string(),
            );
        }
    };

    let source_arg = if resolves_to_existing_file(app, &target) {
        format!(r#"file_path: "{target}""#)
    } else {
        format!("content: {:?}", target)
    };
    let message = format!(
        "Open and use a persistent RLM session for this request. Call `rlm_open` with name `slash_rlm` and {source_arg}. Then call `rlm_configure` with `sub_rlm_max_depth: {max_depth}`. Use `rlm_eval` to inspect the context through `peek`, `search`, and `chunk`, and call `finalize(...)` from the REPL when ready. If a `var_handle` is returned, use `handle_read` for bounded slices or projections before answering."
    );

    CommandResult::with_message_and_action(
        format!("Opening persistent RLM context at depth {max_depth}..."),
        AppAction::SendMessage(message),
    )
}

fn novel_analyze(app: &mut App, arg: Option<&str>) -> CommandResult {
    let (max_depth, focus) = match parse_depth_prefixed_arg(arg, 2) {
        Ok(parsed) => parsed,
        Err(message) => return CommandResult::error(message),
    };
    let focus = focus
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("全书连续性、人物状态、时间线、伏笔、知识边界和候选记忆更新。");
    let prompt = build_novel_analysis_prompt(&app.workspace, focus);
    let source_packet = match novel::manuscript_analysis_index_packet(&app.workspace, focus) {
        Ok(packet) => packet,
        Err(err) => {
            return CommandResult::error(format!(
                "failed to build manuscript analysis index: {err}"
            ));
        }
    };
    let source_arg = format!("content: {source_packet:?}");
    let message = format!(
        "Open and use a persistent RLM manuscript-analysis session for this Novel Studio workspace. Call `rlm_open` with name `novel_analysis` and {source_arg}. Then call `rlm_configure` with `sub_rlm_max_depth: {max_depth}`. Use `rlm_eval` with this task:\n\n{prompt}\n\nInspect the book through `search`, `chunk`, and bounded `peek` calls. Split by chapter, character, location, event, relationship, promise/foreshadowing, and knowledge boundary. Use `sub_query_batch` or `sub_rlm` for semantic slices. Call `finalize(...)` with coverage, chapter/source evidence, continuity risks, and candidate memory updates. Save or ask the parent to save the final report, then import it with `/memory import-analysis <report-path>` so candidates become reviewable project assets. If a `var_handle` is returned, use `handle_read` for bounded projections before answering."
    );

    CommandResult::with_message_and_action(
        format!("Opening novel manuscript analysis at RLM depth {max_depth}..."),
        AppAction::SendMessage(message),
    )
}

fn novel_project_map(app: &mut App, arg: Option<&str>) -> CommandResult {
    let full = arg
        .map(str::trim)
        .is_some_and(|arg| matches!(arg, "--full" | "full"));
    match novel::project_map_packet_with_options(&app.workspace, full) {
        Ok(packet) => CommandResult::message(packet),
        Err(err) => CommandResult::error(format!("failed to build novel project map: {err}")),
    }
}

pub(crate) fn build_novel_analysis_prompt(workspace: &std::path::Path, focus: &str) -> String {
    format!(
        "Analyze this DeepSeek Novel Studio workspace as a long-form manuscript, not as source code.\n\n\
         Workspace: {workspace}\n\
         Focus: {focus}\n\n\
         Required method:\n\
         - First locate `book.toml`, `bible/`, `cards/`, `outline/`, `chapters/`, `memory/graph.json`, `memory/summaries/`, and memory ledgers when present.\n\
         - Use deterministic Python for file lists, chapter ordering, coverage counts, and JSON/JSONL parsing.\n\
         - Use bounded RLM helpers only; do not copy full chapters into the parent answer.\n\
         - Split manuscript evidence by chapter, character, location, event, relationship, promise/foreshadowing, and knowledge boundary.\n\
         - Report BLOCKER/MAJOR/MINOR continuity risks with concrete chapter or source evidence.\n\
         - Return candidate memory updates with `chapter`, `kind`, `target`, `change`, `evidence`, `confidence`, and `affects` fields when supported.\n\
         - Put final candidate records under a `## CANDIDATE_MEMORY_UPDATES` heading as JSON lines when possible.\n\
         - After finalizing, save the report under `memory/reports/` or give the parent a report body that can be saved, then run `/memory import-analysis <report-path>` to stage reviewable candidates.\n\
         - Do not score prose or force a universal writing formula.\n\n\
         Final answer shape:\n\
         1. Coverage: chapters/assets inspected and gaps.\n\
         2. Findings: ordered by continuity or reader-promise risk.\n\
         3. Candidate memory updates: machine-readable bullets or JSON-like records.\n\
         4. Follow-up assets to write or verify.",
        workspace = workspace.display()
    )
}

/// Open a persistent sub-agent session from a slash command.
pub fn agent(_app: &mut App, arg: Option<&str>) -> CommandResult {
    let (max_depth, task) = match parse_depth_prefixed_arg(arg, 1) {
        Ok(parsed) => parsed,
        Err(message) => return CommandResult::error(message),
    };
    let task = match task {
        Some(task) if !task.trim().is_empty() => task.trim().to_string(),
        _ => {
            return CommandResult::error(
                "Usage: /agent [N] <task>\n\n\
                 Opens a persistent sub-agent session with recursive agent depth N (0-3, default 1).",
            );
        }
    };
    let message = format!(
        "Open a persistent writing sub-agent session for this Novel Studio task. Call `agent_open` with name `slash_agent`, `prompt: {:?}`, and `max_depth: {max_depth}`. Use `agent_eval` to wait for the next terminal/current projection and `handle_read` on the returned transcript_handle if you need more detail. Verify claimed book side effects such as chapter edits, memory updates, exports, or command results before reporting success.",
        task
    );
    CommandResult::with_message_and_action(
        format!("Opening persistent sub-agent at depth {max_depth}..."),
        AppAction::SendMessage(message),
    )
}

fn novel_plan(app: &mut App, arg: Option<&str>) -> CommandResult {
    let direction = arg.map(str::trim).filter(|value| !value.is_empty());
    let mut message = String::new();
    let _ = writeln!(
        message,
        "Use DeepSeek Novel Studio's top-level book workflow to generate or refresh this novel's plan."
    );
    let _ = writeln!(message);
    let _ = writeln!(message, "Workspace: {}", app.workspace.display());
    let _ = writeln!(
        message,
        "Read `book.toml`, `bible/`, `cards/`, `outline/`, and `memory/graph.json` if present."
    );
    let _ = writeln!(
        message,
        "Then update `outline/master_plan.md` and `outline/chapter_index.md` with a long-form plan that preserves character agency, causality, reader promise, and memory continuity."
    );
    if let Some(direction) = direction {
        let _ = writeln!(message);
        let _ = writeln!(message, "Direction: {direction}");
    }
    CommandResult::with_message_and_action(
        "Planning the novel from durable book assets...",
        AppAction::SendMessage(message),
    )
}

fn novel_chapter_command(app: &mut App, action: &str, arg: Option<&str>) -> CommandResult {
    let Some((chapter, direction)) = parse_chapter_arg(arg) else {
        return CommandResult::error(format!("Usage: /{action} <chapter> [direction]"));
    };
    let message = build_novel_chapter_instruction(app, action, chapter, direction.as_deref());
    CommandResult::with_message_and_action(
        format!("Starting {action} for chapter {chapter:03}..."),
        AppAction::SendMessage(message),
    )
}

fn novel_chapter_diff(app: &mut App, arg: Option<&str>) -> CommandResult {
    let Some((chapter, _)) = parse_chapter_arg(arg) else {
        return CommandResult::error("Usage: /chapter-diff <chapter>");
    };
    match novel::chapter_diff_packet(&app.workspace, chapter) {
        Ok(packet) => CommandResult::message(packet),
        Err(err) => CommandResult::error(format!("failed to diff chapter {chapter:03}: {err}")),
    }
}

fn novel_chapter_undo(app: &mut App, arg: Option<&str>) -> CommandResult {
    let Some((chapter, _)) = parse_chapter_arg(arg) else {
        return CommandResult::error("Usage: /chapter-undo <chapter>");
    };
    match novel::chapter_undo_from_workspace(&app.workspace, chapter) {
        Ok(report) => CommandResult::message(report),
        Err(err) => CommandResult::error(format!("failed to undo chapter {chapter:03}: {err}")),
    }
}

fn parse_chapter_arg(arg: Option<&str>) -> Option<(u32, Option<String>)> {
    let raw = arg?.trim();
    if raw.is_empty() {
        return None;
    }
    let mut parts = raw.splitn(2, char::is_whitespace);
    let chapter = parts.next()?.parse::<u32>().ok()?;
    if chapter == 0 {
        return None;
    }
    let direction = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    Some((chapter, direction))
}

fn build_novel_chapter_instruction(
    app: &App,
    action: &str,
    chapter: u32,
    direction: Option<&str>,
) -> String {
    let chapter_dir = format!("chapters/{chapter:03}");
    let direction = direction.unwrap_or("按书稿记忆、总纲和当前章节状态推进。");
    match action {
        "brief" => format!(
            "Generate a focused chapter brief for chapter {chapter:03}.\n\n\
             Workspace: {workspace}\n\
             Read `book.toml`, `bible/`, `cards/`, `outline/`, recent `memory/summaries/`, `memory/graph.json`, and nearby chapters as needed.\n\
             Write `{chapter_dir}/brief.md` with scene goals, character knowledge boundaries, causality, conflicts, facts that must not be broken, foreshadowing, and the chapter-end hook.\n\
             Direction: {direction}",
            workspace = app.workspace.display(),
        ),
        "empower" => format!(
            "Build optional craft notes for chapter {chapter:03}; do not turn them into a mandatory formula.\n\n\
             Workspace: {workspace}\n\
             Read the chapter brief, book bible, memory graph, craft/human_texture.md, and nearby chapter memory.\n\
             Write `{chapter_dir}/craft_plan.md` as a flexible thinking note: memory relationships, character possibilities, scene pressure, language tendency, places to leave open for drafting, and memory-update candidates.\n\
             Direction: {direction}",
            workspace = app.workspace.display(),
        ),
        "write" => format!(
            "Draft chapter {chapter:03} as human-feeling novel prose.\n\n\
             Workspace: {workspace}\n\
             Read `book.toml`, bible/cards/outline, `{chapter_dir}/brief.md`, optional `{chapter_dir}/craft_plan.md`, `memory/graph.json`, recent summaries, and nearby chapters.\n\
             Before drafting, assemble a ContextQualityReport: memory graph status, brief/craft presence, character-card coverage, recent summaries, nearby chapters, and open/progress promises. For xianxia/xuanhuan projects, also check world/rule cards, realm rules, faction pressure, resource/economy anchors, and artifact/spell constraints. If major context gaps exist, state them before writing and keep assumptions conservative.\n\
             Write `{chapter_dir}/draft.md`. Output chapter text in the file, not a checklist. Preserve continuity, causality, character agency, sensory pressure, dialogue conflict, misjudgment, concealment, hesitation, consequence, and natural Chinese rhythm. Avoid generic AI transitions, moral summaries, and template-shaped scene beats.\n\
             Direction: {direction}",
            workspace = app.workspace.display(),
        ),
        "audit" => format!(
            "Diagnose chapter {chapter:03} for long-form memory and continuity risks.\n\n\
             Workspace: {workspace}\n\
             Read the current draft/final, memory graph, bible/cards/outline, chapter brief, recent summaries, and affected prior chapters.\n\
             Start with a deterministic ChapterQualityReport covering length, dialogue function, scene causality/consequence, promise progress, anchor carry, and generic prose signals. For xianxia/xuanhuan projects only, include resource anchors, combat knowledge loops, dialogue voice, worldbuilding-in-action, concrete emotion texture, and short-breath rhythm. Use these as evidence, not as a generic scorecard.\n\
             Write `{chapter_dir}/audit.md`. This is not style scoring and not a formula. Findings should focus on memory conflicts, canon gaps, knowledge-boundary leaks, timeline drift, broken promises, missing memory updates, and compatible fixes.\n\
             Required layered sections: `## AUDIT_OVERVIEW`, `## CONTINUITY_AUDIT`, `## CRAFT_AUDIT`, `## MEMORY_CANDIDATE_AUDIT`, and `## READER_PROMISE_AUDIT`.\n\
             Required compatibility sections: `## BLOCKER`, `## MAJOR`, `## MINOR`, `## AFFECTED_NODES`, and `## CANDIDATE_MEMORY_UPDATES`.\n\
             In `## CANDIDATE_MEMORY_UPDATES`, write one JSON object per line when possible with fields `chapter`, `kind`, `target`, `change`, `evidence`, `confidence`, `affects`, and `status: \"candidate\"`.\n\
             Example: {{\"chapter\":{chapter},\"kind\":\"promise\",\"target\":\"事故真相\",\"change\":\"从新埋推进为林墨主动追查\",\"evidence\":\"chapters/{chapter:03}/draft.md:12\",\"confidence\":0.87,\"affects\":[\"character:lin_mo\",\"promise:accident_truth\"],\"status\":\"candidate\"}}\n\
             Use `- none` only if there are no durable memory updates.\n\
             Direction: {direction}",
            workspace = app.workspace.display(),
        ),
        "revise" => format!(
            "Revise chapter {chapter:03} using memory diagnosis while preserving natural prose.\n\n\
             Workspace: {workspace}\n\
             Read `{chapter_dir}/draft.md`, `{chapter_dir}/audit.md` if present, book bible/cards/outline, memory graph, recent summaries, and nearby chapters.\n\
             Before writing, state the exact change scope: read-only inputs, the single writable target `{chapter_dir}/final.md`, and the version-protection step under `{chapter_dir}/.versions/`.\n\
             Before changing `{chapter_dir}/final.md`, preserve the current final if it exists; if no final exists, preserve `{chapter_dir}/draft.md` as the source version for this revision. Write only `{chapter_dir}/final.md` as the complete revised chapter. Do not modify `draft.md`, `audit.md`, memory ledgers, or unrelated chapters in this step.\n\
             Extract the top 3 actionable issues from the audit and deterministic quality signals, then revise only those targets. Preserve working scenes, voice, order, and prose texture; do not rewrite the whole chapter unless one of the top 3 issues requires it. After writing, report changed files and suggest `/chapter-diff {chapter}` plus `/remember {chapter}` as the verification path.\n\
             Direction: {direction}",
            workspace = app.workspace.display(),
        ),
        "remember" => format!(
            "Extract durable long-form memory from chapter {chapter:03}.\n\n\
             Workspace: {workspace}\n\
             Read `{chapter_dir}/final.md` or `{chapter_dir}/draft.md`, plus bible/cards/outline and existing memory ledgers.\n\
             Write `memory/summaries/{chapter:03}.md` and `memory/candidates/{chapter:03}.json`. Do not directly edit `memory/facts.jsonl`, `memory/events.jsonl`, or `memory/foreshadowing.jsonl`; durable ledger writes happen only after review with `/memory candidates {chapter}` and confirmation with `/memory apply {chapter}`.\n\
             Capture causality, character state changes, new facts, timeline events, foreshadowing status, relationships, location state, objects/resources, hidden emotions, and future continuity risks.\n\
             Candidate records must be machine-readable objects with `chapter`, `kind`, `target`, `change`, `evidence`, `confidence`, `affects`, and `status: \"candidate\"`. Rebuild or request rebuild of `memory/graph.json` only after candidates are applied.\n\
             Direction: {direction}",
            workspace = app.workspace.display(),
        ),
        _ => format!("Handle chapter {chapter:03}. Direction: {direction}"),
    }
}

/// Ask the active model to write a compact relay artifact for the next thread.
///
/// The visible command is `/relay` (with `/接力` for Chinese users), but the
/// durable file path remains `.deepseek/handoff.md` for compatibility with
/// existing sessions and startup prompt loading.
pub fn relay(app: &mut App, arg: Option<&str>) -> CommandResult {
    let focus = arg.map(str::trim).filter(|value| !value.is_empty());
    let message = build_relay_instruction(app, focus);
    CommandResult::with_message_and_action(
        "Preparing session relay at .deepseek/handoff.md...",
        AppAction::SendMessage(message),
    )
}

fn build_relay_instruction(app: &App, focus: Option<&str>) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Create a compact session relay (接力) for a future DeepSeek TUI thread."
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "Write or update `.deepseek/handoff.md`.");
    let _ = writeln!(
        out,
        "Keep the existing file path for compatibility, but title the artifact `# Session relay`."
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "Current session snapshot:");
    let _ = writeln!(out, "- Workspace: {}", app.workspace.display());
    let _ = writeln!(out, "- Mode: {}", app.mode.label());
    let _ = writeln!(out, "- Model: {}", app.model_display_label());
    if let Some(focus) = focus {
        let _ = writeln!(out, "- Requested relay focus: {focus}");
    }
    if let Some(goal) = app.goal.goal_objective.as_deref() {
        let _ = writeln!(out, "- Goal: {goal}");
    }
    if let Some(budget) = app.goal.goal_token_budget {
        let _ = writeln!(out, "- Goal token budget: {budget}");
    }
    if app.cycle_count > 0 {
        let _ = writeln!(out, "- Cycle count: {}", app.cycle_count);
    }

    if let Ok(novel_context) = novel::creative_relay_context_packet(&app.workspace) {
        let _ = writeln!(out, "\n{novel_context}");
    }

    if let Ok(todos) = app.todos.try_lock() {
        let snapshot = todos.snapshot();
        if !snapshot.items.is_empty() {
            let _ = writeln!(
                out,
                "\nWork checklist (primary progress surface, {}% complete):",
                snapshot.completion_pct
            );
            for item in snapshot.items {
                let _ = writeln!(
                    out,
                    "- #{} [{}] {}",
                    item.id,
                    item.status.as_str(),
                    item.content
                );
            }
        }
    } else {
        let _ = writeln!(
            out,
            "\nWork checklist: unavailable because the checklist is busy."
        );
    }

    if let Ok(plan) = app.plan_state.try_lock() {
        let snapshot = plan.snapshot();
        if snapshot.explanation.is_some() || !snapshot.items.is_empty() {
            let _ = writeln!(out, "\nOptional strategy metadata from update_plan:");
            if let Some(explanation) = snapshot.explanation.as_deref() {
                let _ = writeln!(out, "- Explanation: {explanation}");
            }
            for item in snapshot.items {
                let _ = writeln!(out, "- [{}] {}", plan_status_label(&item.status), item.step);
            }
        }
    } else {
        let _ = writeln!(
            out,
            "\nStrategy metadata: unavailable because plan state is busy."
        );
    }

    let _ = writeln!(
        out,
        "\nBefore writing, inspect the current transcript context and any live tool evidence you need. Do not invent test results, file changes, blockers, or decisions."
    );
    let _ = writeln!(
        out,
        "\nUse this compact structure:\n\
         # Session relay\n\
         \n\
         ## Goal\n\
         [the user's objective and any explicit constraints]\n\
         \n\
         ## Novel state\n\
         [book title, current volume/chapter, next likely chapter, character states, open promises, candidate memory updates]\n\
         \n\
         ## Current work\n\
         [the active Work checklist item, progress, and what is mid-flight]\n\
         \n\
         ## Files and state\n\
         [changed files, important paths, sub-agents/RLM sessions, commands run]\n\
         \n\
         ## Decisions\n\
         [why key choices were made]\n\
         \n\
         ## Verification\n\
         [what passed, what failed, what was not run]\n\
         \n\
         ## Next action\n\
         [one concrete writing or verification action for the next thread]"
    );
    let _ = writeln!(
        out,
        "\nKeep it under about 900 words unless the session genuinely needs more. After writing, report the path and the single next action."
    );
    out
}

fn plan_status_label(status: &crate::tools::plan::StepStatus) -> &'static str {
    match status {
        crate::tools::plan::StepStatus::Pending => "pending",
        crate::tools::plan::StepStatus::InProgress => "in_progress",
        crate::tools::plan::StepStatus::Completed => "completed",
    }
}

fn parse_depth_prefixed_arg(
    arg: Option<&str>,
    default_depth: u32,
) -> Result<(u32, Option<&str>), String> {
    let Some(raw) = arg.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok((default_depth, None));
    };
    let mut parts = raw.splitn(2, char::is_whitespace);
    let first = parts.next().unwrap_or_default();
    if first.chars().all(|ch| ch.is_ascii_digit()) {
        let depth: u32 = first
            .parse()
            .map_err(|_| "Depth must be an integer from 0 to 3".to_string())?;
        if depth > 3 {
            return Err("Depth must be between 0 and 3".to_string());
        }
        Ok((depth, parts.next().map(str::trim)))
    } else {
        Ok((default_depth, Some(raw)))
    }
}

fn resolves_to_existing_file(app: &App, input: &str) -> bool {
    let path = std::path::Path::new(input);
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        app.workspace.join(path)
    };
    candidate.is_file()
}

/// Get command info by name or alias
pub fn get_command_info(name: &str) -> Option<&'static CommandInfo> {
    let name = name.strip_prefix('/').unwrap_or(name);
    COMMANDS
        .iter()
        .find(|cmd| cmd.name == name || cmd.aliases.contains(&name))
}

/// Get all command names matching a prefix, including both built-in
/// static commands and user-defined commands, formatted as `/name`.
///
/// `workspace` is used to also scan workspace-local command directories;
/// pass `None` when no workspace context is available.
pub fn all_command_names_matching(
    prefix: &str,
    workspace: Option<&std::path::Path>,
) -> Vec<String> {
    let prefix = prefix.strip_prefix('/').unwrap_or(prefix).to_lowercase();
    let mut result: Vec<String> = COMMANDS
        .iter()
        .filter(|cmd| {
            cmd.name.starts_with(&prefix) || cmd.aliases.iter().any(|a| a.starts_with(&prefix))
        })
        .map(|cmd| format!("/{}", cmd.name))
        .collect();

    // Add user-defined commands
    result.extend(user_commands::user_commands_matching(&prefix, workspace));

    result.sort();
    result.dedup();
    result
}

/// Get all commands matching a prefix (for autocomplete)
#[allow(dead_code)]
pub fn commands_matching(prefix: &str) -> Vec<&'static CommandInfo> {
    let prefix = prefix.strip_prefix('/').unwrap_or(prefix).to_lowercase();
    COMMANDS
        .iter()
        .filter(|cmd| {
            cmd.name.starts_with(&prefix) || cmd.aliases.iter().any(|a| a.starts_with(&prefix))
        })
        .collect()
}

fn edit_distance(a: &str, b: &str) -> usize {
    if a == b {
        return 0;
    }
    if a.is_empty() {
        return b.chars().count();
    }
    if b.is_empty() {
        return a.chars().count();
    }

    let b_chars: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b_chars.len()).collect();
    let mut curr = vec![0usize; b_chars.len() + 1];

    for (i, a_ch) in a.chars().enumerate() {
        curr[0] = i + 1;
        for (j, b_ch) in b_chars.iter().enumerate() {
            let cost = if a_ch == *b_ch { 0 } else { 1 };
            let delete = prev[j + 1] + 1;
            let insert = curr[j] + 1;
            let substitute = prev[j] + cost;
            curr[j + 1] = delete.min(insert).min(substitute);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[b_chars.len()]
}

fn suggest_command_names(input: &str, limit: usize) -> Vec<String> {
    let query = input.trim().to_ascii_lowercase();
    if query.is_empty() || limit == 0 {
        return Vec::new();
    }

    let mut scored: Vec<(u8, usize, String)> = Vec::new();
    for command in COMMANDS {
        let mut best: Option<(u8, usize)> = None;
        for candidate in std::iter::once(command.name).chain(command.aliases.iter().copied()) {
            let candidate = candidate.to_ascii_lowercase();
            let prefix_match = candidate.starts_with(&query) || query.starts_with(&candidate);
            let contains_match = candidate.contains(&query) || query.contains(&candidate);
            let distance = edit_distance(&candidate, &query);
            let close_typo = distance <= 2;
            if !(prefix_match || contains_match || close_typo) {
                continue;
            }

            let rank = if prefix_match {
                0
            } else if contains_match {
                1
            } else {
                2
            };

            match best {
                Some((best_rank, best_distance))
                    if rank > best_rank || (rank == best_rank && distance >= best_distance) => {}
                _ => best = Some((rank, distance)),
            }
        }

        if let Some((rank, distance)) = best {
            scored.push((rank, distance, command.name.to_string()));
        }
    }

    scored.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    scored
        .into_iter()
        .take(limit)
        .map(|(_, _, name)| name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tools::plan::{PlanItemArg, StepStatus, UpdatePlanArgs};
    use crate::tools::todo::TodoStatus;
    use crate::tui::app::{App, AppAction, TuiOptions};
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::sync::MutexGuard;

    fn create_test_app() -> App {
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: PathBuf::from("."),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: PathBuf::from("."),
            memory_path: PathBuf::from("memory.md"),
            notes_path: PathBuf::from("notes.txt"),
            mcp_config_path: PathBuf::from("mcp.json"),
            use_memory: false,
            start_in_agent_mode: false,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        App::new(options, &Config::default())
    }

    #[test]
    fn command_registry_contains_config_and_links_but_not_set_or_deepseek() {
        assert!(COMMANDS.iter().any(|cmd| cmd.name == "config"));
        assert!(COMMANDS.iter().any(|cmd| cmd.name == "links"));
        assert!(COMMANDS.iter().any(|cmd| cmd.name == "memory"));
        assert!(!COMMANDS.iter().any(|cmd| cmd.name == "set"));
        assert!(!COMMANDS.iter().any(|cmd| cmd.name == "deepseek"));
    }

    #[test]
    fn command_registry_surfaces_chapter_version_commands_not_legacy_debug_names() {
        let chapter_diff = COMMANDS
            .iter()
            .find(|cmd| cmd.name == "chapter-diff")
            .expect("chapter-diff command should exist");
        let chapter_undo = COMMANDS
            .iter()
            .find(|cmd| cmd.name == "chapter-undo")
            .expect("chapter-undo command should exist");

        assert!(!COMMANDS.iter().any(|cmd| cmd.name == "diff"));
        assert!(!COMMANDS.iter().any(|cmd| cmd.name == "undo"));
        assert_eq!(
            chapter_diff.description_id,
            MessageId::CmdChapterDiffDescription
        );
        assert_eq!(
            chapter_undo.description_id,
            MessageId::CmdChapterUndoDescription
        );

        let diff_description = chapter_diff.description_for(Locale::En);
        let undo_description = chapter_undo.description_for(Locale::En);
        assert!(diff_description.contains("chapter"));
        assert!(undo_description.contains("chapter"));
        assert!(!diff_description.contains("session start"));
        assert!(!undo_description.contains("message pair"));
        assert!(!all_command_names_matching("di", None).contains(&"/diff".to_string()));
        assert!(!all_command_names_matching("un", None).contains(&"/undo".to_string()));
    }

    #[test]
    fn legacy_diff_and_undo_still_dispatch_for_compatibility() {
        for invocation in ["/diff", "/undo"] {
            let (mut app, _tmpdir, _guard) = create_isolated_test_app();
            let result = execute(invocation, &mut app);
            if let Some(message) = &result.message {
                assert!(
                    !message.contains("Unknown command"),
                    "{invocation} should remain dispatch-compatible: {message}",
                );
            }
        }
    }

    #[test]
    fn links_command_has_dashboard_and_api_aliases() {
        let links = COMMANDS
            .iter()
            .find(|cmd| cmd.name == "links")
            .expect("links command should exist");
        assert_eq!(links.aliases, &["dashboard", "api"]);
    }

    #[test]
    fn rlm_slash_command_routes_to_persistent_tool_instruction() {
        let mut app = create_test_app();
        let result = execute("/rlm 2 inspect this long corpus", &mut app);
        assert!(!result.is_error);
        assert!(result.message.as_deref().unwrap_or("").contains("depth 2"));
        let Some(AppAction::SendMessage(message)) = result.action else {
            panic!("expected SendMessage action");
        };
        assert!(message.contains("rlm_open"));
        assert!(message.contains("rlm_configure"));
        assert!(message.contains("sub_rlm_max_depth: 2"));
    }

    #[test]
    fn analyze_slash_command_routes_to_novel_rlm_workflow() {
        let (mut app, tmpdir, _guard) = create_isolated_test_app();
        novel::initialize_project(
            tmpdir.path(),
            novel::NovelInitOptions {
                title: Some("RLM 分析测试".to_string()),
                genre: Some("悬疑".to_string()),
                premise: Some("失踪者留下反常记忆".to_string()),
                target_words: 80_000,
                language: "zh-CN".to_string(),
                force: false,
            },
        )
        .expect("init project");
        std::fs::create_dir_all(tmpdir.path().join("chapters/001")).expect("chapter dir");
        std::fs::write(
            tmpdir.path().join("chapters/001/final.md"),
            "林墨发现事故真相的第一条线索。",
        )
        .expect("write chapter");

        let result = execute("/analyze 3 林墨 知识边界", &mut app);
        assert!(!result.is_error);
        let Some(AppAction::SendMessage(message)) = result.action else {
            panic!("expected SendMessage action");
        };
        assert!(message.contains("rlm_open"));
        assert!(message.contains("novel_analysis"));
        assert!(message.contains("content:"));
        assert!(!message.contains(r#"file_path: ".""#));
        assert!(message.contains("Novel Manuscript Analysis Index"));
        assert!(message.contains("Chapter 001"));
        assert!(message.contains("sub_rlm_max_depth: 3"));
        assert!(message.contains("Split by chapter"));
        assert!(message.contains("knowledge boundary"));
        assert!(message.contains("candidate memory updates"));
        assert!(message.contains("Do not score prose"));
        assert!(message.contains("林墨 知识边界"));
    }

    #[test]
    fn novel_analysis_prompt_names_book_assets_and_candidate_schema() {
        let prompt = build_novel_analysis_prompt(std::path::Path::new("book-root"), "伏笔回收");

        assert!(prompt.contains("book.toml"));
        assert!(prompt.contains("chapters/"));
        assert!(prompt.contains("memory/graph.json"));
        assert!(prompt.contains("chapter"));
        assert!(prompt.contains("promise/foreshadowing"));
        assert!(prompt.contains("confidence"));
        assert!(prompt.contains("affects"));
        assert!(prompt.contains("伏笔回收"));
    }

    #[test]
    fn manuscript_analysis_index_summarizes_book_assets_for_rlm() {
        let tmpdir = tempfile::TempDir::new().expect("temp dir");
        novel::initialize_project(
            tmpdir.path(),
            novel::NovelInitOptions {
                title: Some("索引包测试".to_string()),
                genre: Some("悬疑".to_string()),
                premise: Some("失踪者留下反常记忆".to_string()),
                target_words: 80_000,
                language: "zh-CN".to_string(),
                force: false,
            },
        )
        .expect("init project");
        std::fs::write(
            tmpdir.path().join("cards/characters/lin_mo.yaml"),
            "name: 林墨\nknowledge:\n  - 陈岚说谎\nunknown:\n  - 父亲仍活着\n",
        )
        .expect("write character");
        std::fs::write(
            tmpdir.path().join("memory/events.jsonl"),
            "{\"kind\":\"event\",\"event\":\"林墨发现监控被剪\",\"chapter\":1}\n",
        )
        .expect("write events");
        std::fs::create_dir_all(tmpdir.path().join("chapters/001")).expect("chapter dir");
        std::fs::write(
            tmpdir.path().join("chapters/001/final.md"),
            "林墨发现事故真相的第一条线索。",
        )
        .expect("write chapter");

        let packet = novel::manuscript_analysis_index_packet(tmpdir.path(), "知识边界")
            .expect("analysis index");

        assert!(packet.contains("# Novel Manuscript Analysis Index"));
        assert!(packet.contains("Focus: 知识边界"));
        assert!(packet.contains("Character Cards"));
        assert!(packet.contains("林墨"));
        assert!(packet.contains("Memory Ledger: Events"));
        assert!(packet.contains("林墨发现监控被剪"));
        assert!(packet.contains("Chapter 001"));
        assert!(packet.contains("chapters/001/final.md"));
        assert!(packet.contains("local materials are reference only"));
    }

    #[test]
    fn map_slash_command_returns_novel_project_map() {
        let (mut app, tmpdir, _guard) = create_isolated_test_app();
        novel::initialize_project(
            tmpdir.path(),
            novel::NovelInitOptions {
                title: Some("地图命令测试".to_string()),
                genre: Some("悬疑".to_string()),
                premise: None,
                target_words: 80_000,
                language: "zh-CN".to_string(),
                force: false,
            },
        )
        .expect("init project");
        std::fs::write(
            tmpdir.path().join("cards/characters/lin_mo.yaml"),
            "name: 林墨\n",
        )
        .expect("write card");

        let result = execute("/map", &mut app);

        assert!(!result.is_error);
        let message = result.message.expect("map output");
        assert!(message.contains("# Novel Project Map"));
        assert!(message.contains("地图命令测试"));
        assert!(message.contains("cards/characters/lin_mo.yaml"));
    }

    #[test]
    fn agent_slash_command_routes_to_persistent_tool_instruction() {
        let mut app = create_test_app();
        let result = execute("/agent 0 inspect the parser", &mut app);
        assert!(!result.is_error);
        let Some(AppAction::SendMessage(message)) = result.action else {
            panic!("expected SendMessage action");
        };
        assert!(message.contains("agent_open"));
        assert!(message.contains("max_depth: 0"));
        assert!(message.contains("Novel Studio task"));
        assert!(message.contains("chapter edits"));
        assert!(message.contains("memory updates"));
    }

    #[test]
    fn novel_slash_commands_route_to_book_workflows() {
        let mut app = create_test_app();
        let result = execute("/write 12 强化人物误判", &mut app);
        assert!(!result.is_error);
        let Some(AppAction::SendMessage(message)) = result.action else {
            panic!("expected SendMessage action");
        };
        assert!(message.contains("Draft chapter 012"));
        assert!(message.contains("chapters/012/draft.md"));
        assert!(message.contains("memory/graph.json"));
        assert!(message.contains("ContextQualityReport"));
        assert!(message.contains("强化人物误判"));

        let result = execute("/audit 12", &mut app);
        assert!(!result.is_error);
        let Some(AppAction::SendMessage(message)) = result.action else {
            panic!("expected SendMessage action");
        };
        assert!(message.contains("Diagnose chapter 012"));
        assert!(message.contains("chapters/012/audit.md"));
        assert!(message.contains("not style scoring"));
        assert!(message.contains("ChapterQualityReport"));
        assert!(message.contains("## BLOCKER"));
        assert!(message.contains("## AFFECTED_NODES"));
        assert!(message.contains("## CANDIDATE_MEMORY_UPDATES"));
        assert!(message.contains("\"confidence\""));
        assert!(message.contains("\"affects\""));
        assert!(message.contains("status: \"candidate\""));
        assert!(message.contains("\"status\":\"candidate\""));

        let result = execute("/revise 12 保留对白锋利度", &mut app);
        assert!(!result.is_error);
        let Some(AppAction::SendMessage(message)) = result.action else {
            panic!("expected SendMessage action");
        };
        assert!(message.contains("Revise chapter 012"));
        assert!(message.contains("exact change scope"));
        assert!(message.contains("chapters/012/final.md"));
        assert!(message.contains("chapters/012/.versions/"));
        assert!(message.contains("Do not modify `draft.md`, `audit.md`, memory ledgers"));
        assert!(message.contains("top 3 actionable issues"));
        assert!(message.contains("revise only those targets"));
        assert!(message.contains("/chapter-diff 12"));
        assert!(message.contains("/remember 12"));
        assert!(message.contains("保留对白锋利度"));

        let result = execute("/remember 12", &mut app);
        assert!(!result.is_error);
        let Some(AppAction::SendMessage(message)) = result.action else {
            panic!("expected SendMessage action");
        };
        assert!(message.contains("Extract durable long-form memory from chapter 012"));
        assert!(message.contains("memory/summaries/012.md"));
        assert!(message.contains("memory/candidates/012.json"));
        assert!(message.contains("Do not directly edit `memory/facts.jsonl`"));
        assert!(message.contains("/memory candidates 12"));
        assert!(message.contains("/memory apply 12"));
        assert!(message.contains("status: \"candidate\""));
    }

    #[test]
    fn novel_slash_commands_validate_chapter_arguments() {
        let mut app = create_test_app();
        let result = execute("/brief", &mut app);
        assert!(result.is_error);
        assert!(result.message.unwrap().contains("Usage: /brief <chapter>"));
    }

    #[test]
    fn relay_slash_command_routes_to_session_relay_instruction() {
        let mut app = create_test_app();
        app.goal.goal_objective = Some("Unify the work surface".to_string());
        app.goal.goal_token_budget = Some(12_000);
        app.cycle_count = 2;
        {
            let mut todos = app.todos.try_lock().expect("todo lock");
            todos.add("inspect workspace".to_string(), TodoStatus::Completed);
            todos.add("patch relay command".to_string(), TodoStatus::InProgress);
        }
        {
            let mut plan = app.plan_state.try_lock().expect("plan lock");
            plan.update(UpdatePlanArgs {
                explanation: Some("RLM-style strategy".to_string()),
                plan: vec![PlanItemArg {
                    step: "keep checklist primary".to_string(),
                    status: StepStatus::InProgress,
                }],
            });
        }

        let result = execute("/relay verify install", &mut app);
        assert!(!result.is_error);
        assert!(
            result
                .message
                .as_deref()
                .unwrap_or_default()
                .contains(".deepseek/handoff.md")
        );
        let Some(AppAction::SendMessage(message)) = result.action else {
            panic!("expected SendMessage action");
        };
        assert!(message.contains("session relay"));
        assert!(message.contains("接力"));
        assert!(message.contains("Write or update `.deepseek/handoff.md`"));
        assert!(message.contains("# Session relay"));
        assert!(message.contains("Requested relay focus: verify install"));
        assert!(message.contains("Goal: Unify the work surface"));
        assert!(message.contains("Goal token budget: 12000"));
        assert!(message.contains("Cycle count: 2"));
        assert!(message.contains("Work checklist (primary progress surface, 50% complete)"));
        assert!(message.contains("#1 [completed] inspect workspace"));
        assert!(message.contains("#2 [in_progress] patch relay command"));
        assert!(message.contains("Optional strategy metadata from update_plan"));
        assert!(message.contains("Explanation: RLM-style strategy"));
        assert!(message.contains("[in_progress] keep checklist primary"));
    }

    #[test]
    fn relay_slash_command_includes_novel_state_when_workspace_is_a_book() {
        let (mut app, tmpdir, _guard) = create_isolated_test_app();
        novel::initialize_project(
            tmpdir.path(),
            novel::NovelInitOptions {
                title: Some("接力小说".to_string()),
                genre: Some("悬疑".to_string()),
                premise: None,
                target_words: 80_000,
                language: "zh-CN".to_string(),
                force: false,
            },
        )
        .expect("init project");
        std::fs::write(
            tmpdir.path().join("cards/characters/lin_mo.yaml"),
            "name: 林墨\nstate: 隐瞒事故记忆\n",
        )
        .expect("write character");
        std::fs::create_dir_all(tmpdir.path().join("chapters/001")).expect("chapter dir");
        std::fs::write(
            tmpdir.path().join("chapters/001/final.md"),
            "# 第 001 章\n\n林墨没有说出事故记忆。",
        )
        .expect("write final");
        std::fs::write(
            tmpdir.path().join("memory/foreshadowing.jsonl"),
            "{\"promise\":\"事故真相\",\"status\":\"open\",\"first_chapter\":1}\n",
        )
        .expect("write promise");
        novel::rebuild_memory_graph_for_test(tmpdir.path()).expect("save graph");

        let result = execute("/relay", &mut app);

        assert!(!result.is_error);
        let Some(AppAction::SendMessage(message)) = result.action else {
            panic!("expected SendMessage action");
        };
        assert!(message.contains("Novel creative relay context"));
        assert!(message.contains("Book: 接力小说"));
        assert!(message.contains("Current volume: 1"));
        assert!(message.contains("Latest chapter artifact: Chapter 001"));
        assert!(message.contains("Character states to preserve"));
        assert!(message.contains("林墨"));
        assert!(message.contains("Open promises / foreshadowing to carry"));
        assert!(message.contains("事故真相 | open | chapter 001"));
        assert!(message.contains("Next writing pressure"));
        assert!(message.contains("## Novel state"));
    }

    #[test]
    fn relay_command_has_bilingual_aliases() {
        let relay = COMMANDS
            .iter()
            .find(|cmd| cmd.name == "relay")
            .expect("relay command should exist");
        assert_eq!(relay.aliases, &["batonpass", "接力"]);
        assert!(relay.description_for(Locale::ZhHans).contains("接力"));
        assert!(relay.description_for(Locale::ZhHant).contains("接力"));

        let mut app = create_test_app();
        let result = execute("/接力 next hand", &mut app);
        assert!(!result.is_error);
        let Some(AppAction::SendMessage(message)) = result.action else {
            panic!("expected SendMessage action");
        };
        assert!(message.contains("Requested relay focus: next hand"));
    }

    #[test]
    fn command_registry_has_unique_names_and_aliases() {
        let mut names = std::collections::BTreeSet::new();
        for command in COMMANDS {
            assert!(
                names.insert(command.name),
                "duplicate command name /{}",
                command.name
            );
        }

        let mut aliases = std::collections::BTreeSet::new();
        for command in COMMANDS {
            for alias in command.aliases {
                assert!(
                    !names.contains(alias),
                    "alias /{} collides with a command name",
                    alias
                );
                assert!(aliases.insert(*alias), "duplicate command alias /{alias}");
            }
        }
    }

    #[test]
    fn context_command_opens_inspector_and_keeps_ctx_alias() {
        let context = COMMANDS
            .iter()
            .find(|cmd| cmd.name == "context")
            .expect("context command should exist");
        assert_eq!(context.aliases, &["ctx"]);
        assert!(context.description_for(Locale::En).contains("inspector"));

        let mut app = create_test_app();
        let result = execute("/ctx", &mut app);
        assert!(matches!(
            result.action,
            Some(AppAction::OpenContextInspector)
        ));
    }

    #[test]
    fn cache_inspect_dispatches_through_cache_command() {
        let mut app = create_test_app();
        let result = execute("/cache inspect", &mut app);
        let msg = result.message.expect("cache inspect should return text");
        assert!(msg.contains("Cache Inspect"));
        assert!(msg.contains("Base static prefix hash:"));
        assert!(msg.contains("Full request prefix hash:"));
        assert!(result.action.is_none());
    }

    #[test]
    fn cache_warmup_dispatches_action() {
        let mut app = create_test_app();
        let result = execute("/cache warmup", &mut app);
        assert!(result.message.is_none());
        assert!(matches!(result.action, Some(AppAction::CacheWarmup)));
    }

    #[test]
    fn execute_config_opens_config_view_action() {
        let mut app = create_test_app();
        let result = execute("/config", &mut app);
        assert!(result.message.is_none());
        assert!(matches!(result.action, Some(AppAction::OpenConfigView)));
    }

    #[test]
    fn execute_verbose_toggles_live_transcript_detail() {
        let mut app = create_test_app();
        assert!(!app.verbose_transcript);

        let result = execute("/verbose on", &mut app);
        assert!(!result.is_error);
        assert!(app.verbose_transcript);
        assert!(result.message.unwrap().contains("on"));

        let result = execute("/verbose off", &mut app);
        assert!(!result.is_error);
        assert!(!app.verbose_transcript);
        assert!(result.message.unwrap().contains("off"));
    }

    #[test]
    fn execute_links_and_aliases_return_links_message() {
        let mut app = create_test_app();
        for cmd in ["/links", "/dashboard", "/api"] {
            let result = execute(cmd, &mut app);
            let msg = result.message.expect("links commands should return text");
            assert!(msg.contains("https://platform.deepseek.com"));
            assert!(result.action.is_none());
        }
    }

    #[test]
    fn removed_set_and_deepseek_commands_show_migration_hints() {
        let mut app = create_test_app();
        let set_result = execute("/set model deepseek-v4-pro", &mut app);
        let set_msg = set_result
            .message
            .expect("legacy command should return an error message");
        assert!(set_msg.contains("The /set command was retired"));
        assert!(set_msg.contains("/config"));
        assert!(set_msg.contains("/settings"));
        assert!(set_result.action.is_none());

        let deepseek_result = execute("/deepseek", &mut app);
        let deepseek_msg = deepseek_result
            .message
            .expect("legacy command should return an error message");
        assert!(deepseek_msg.contains("The /deepseek command was renamed"));
        assert!(deepseek_msg.contains("/links"));
        assert!(deepseek_msg.contains("/dashboard"));
        assert!(deepseek_msg.contains("/api"));
        assert!(deepseek_result.action.is_none());
    }

    struct ConfigPathGuard {
        previous: Option<OsString>,
        _lock: MutexGuard<'static, ()>,
    }

    impl ConfigPathGuard {
        fn new(config_path: &Path) -> Self {
            let lock = crate::test_support::lock_test_env();
            let previous = std::env::var_os("DEEPSEEK_CONFIG_PATH");
            // Safety: test-only environment mutation guarded by a global mutex.
            unsafe {
                std::env::set_var("DEEPSEEK_CONFIG_PATH", config_path);
            }
            Self {
                previous,
                _lock: lock,
            }
        }
    }

    impl Drop for ConfigPathGuard {
        fn drop(&mut self) {
            // Safety: test-only environment mutation guarded by a global mutex.
            unsafe {
                if let Some(previous) = self.previous.take() {
                    std::env::set_var("DEEPSEEK_CONFIG_PATH", previous);
                } else {
                    std::env::remove_var("DEEPSEEK_CONFIG_PATH");
                }
            }
        }
    }

    /// Build an App scoped to an isolated tempdir so dispatch-side-effects
    /// (e.g. `/init` writing AGENTS.md, `/export` writing chat transcripts,
    /// `/logout` clearing credentials) don't pollute the repo working tree or
    /// the developer's real config when the smoke tests run.
    fn create_isolated_test_app() -> (App, tempfile::TempDir, ConfigPathGuard) {
        let tmpdir = tempfile::TempDir::new().expect("tempdir for smoke test");
        let workspace = tmpdir.path().to_path_buf();
        let config_path = workspace.join(".deepseek").join("config.toml");
        std::fs::create_dir_all(config_path.parent().expect("config parent")).expect("config dir");
        let guard = ConfigPathGuard::new(&config_path);
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: workspace.clone(),
            config_path: Some(config_path),
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: workspace.join("skills"),
            memory_path: workspace.join("memory.md"),
            notes_path: workspace.join("notes.txt"),
            mcp_config_path: workspace.join("mcp.json"),
            use_memory: false,
            start_in_agent_mode: false,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        let app = App::new(options, &Config::default());
        (app, tmpdir, guard)
    }

    /// Smoke test: every entry in `COMMANDS` must dispatch to a real handler.
    /// A dispatch miss surfaces as the fall-through `Unknown command:` error
    /// message in `execute`. This catches the case where a new command is
    /// added to `COMMANDS` (so it shows up in `/help` and the palette) but
    /// the matching arm in `execute` is forgotten — the user would type the
    /// command, see it autocomplete, and then get an unhelpful "did you
    /// mean" suggestion. Also catches panics in handlers because the test
    /// runner unwinds the panic and reports the offending command.
    /// `/save` and `/export` default their output paths to `cwd`-relative
    /// filenames when no arg is supplied, which would scribble files into
    /// `crates/tui/` when CI runs from there. Pass an explicit tempdir-
    /// relative path for those two so the dispatch test stays sandboxed.
    fn invocation_for(command_name: &str, alias_or_name: &str, tmpdir: &std::path::Path) -> String {
        match command_name {
            "save" => format!("/{alias_or_name} {}", tmpdir.join("session.json").display()),
            "export" => format!("/{alias_or_name} {}", tmpdir.join("chat.md").display()),
            _ => format!("/{alias_or_name}"),
        }
    }

    /// `/restore` is covered by its own dedicated tests in
    /// `commands/restore.rs` that serialize on the global env mutex via
    /// `scoped_home` (snapshot repo init shells out to git, which races
    /// against parallel-running tests). Skip it here so this smoke test
    /// stays parallel-safe.
    fn skip_in_dispatch_smoke(name: &str) -> bool {
        name == "restore"
    }

    /// Smoke test: every entry in `COMMANDS` must dispatch to a real handler.
    /// A dispatch miss surfaces as the fall-through `Unknown command:` error
    /// message in `execute`. This catches the case where a new command is
    /// added to `COMMANDS` (so it shows up in `/help` and the palette) but
    /// the matching arm in `execute` is forgotten — the user would type the
    /// command, see it autocomplete, and then get an unhelpful "did you
    /// mean" suggestion. Also catches panics in handlers because the test
    /// runner unwinds the panic and reports the offending command.
    #[test]
    fn every_registered_command_dispatches_to_a_handler() {
        for command in COMMANDS {
            if skip_in_dispatch_smoke(command.name) {
                continue;
            }
            let (mut app, tmpdir, _guard) = create_isolated_test_app();
            let invocation = invocation_for(command.name, command.name, tmpdir.path());
            let result = execute(&invocation, &mut app);
            if let Some(msg) = &result.message {
                assert!(
                    !msg.contains("Unknown command"),
                    "/{} fell through to the unknown-command branch: {msg}",
                    command.name,
                );
            }
        }
    }

    /// Same check, but for declared aliases — `/q` should not fall through
    /// just because the registry lists it as an alias of `/exit`.
    #[test]
    fn every_command_alias_dispatches_to_a_handler() {
        for command in COMMANDS {
            if skip_in_dispatch_smoke(command.name) {
                continue;
            }
            for alias in command.aliases {
                let (mut app, tmpdir, _guard) = create_isolated_test_app();
                let invocation = invocation_for(command.name, alias, tmpdir.path());
                let result = execute(&invocation, &mut app);
                if let Some(msg) = &result.message {
                    assert!(
                        !msg.contains("Unknown command"),
                        "/{alias} (alias of /{}) fell through to unknown: {msg}",
                        command.name,
                    );
                }
            }
        }
    }

    #[test]
    fn unknown_command_suggests_nearest_match() {
        let mut app = create_test_app();
        let result = execute("/modle", &mut app);
        let msg = result
            .message
            .expect("unknown command should return an error message");
        assert!(msg.contains("Unknown command: /modle"));
        assert!(msg.contains("Did you mean:"));
        assert!(msg.contains("/model"));
    }

    #[test]
    fn unknown_command_without_close_match_keeps_help_guidance() {
        let mut app = create_test_app();
        let result = execute("/zzzzzz", &mut app);
        let msg = result
            .message
            .expect("unknown command should return an error message");
        assert!(msg.contains("Unknown command: /zzzzzz"));
        assert!(msg.contains("Type /help for available commands."));
    }
}
