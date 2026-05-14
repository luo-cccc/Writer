//! Core commands: help, clear, exit, model

use std::fmt::Write;

use crate::config::{COMMON_DEEPSEEK_MODELS, normalize_model_name};
use crate::localization::{MessageId, tr};
use crate::tui::app::{App, AppAction, AppMode, ReasoningEffort};
use crate::tui::views::{HelpView, ModalKind, SubAgentsView, subagent_view_agents};

use super::CommandResult;

/// Show help information
pub fn help(app: &mut App, topic: Option<&str>) -> CommandResult {
    if let Some(topic) = topic {
        // Show help for specific command
        if let Some(cmd) = super::get_command_info(topic) {
            let mut help = format!(
                "{}\n\n  {}\n\n  {} {}",
                cmd.name,
                cmd.description_for(app.ui_locale),
                tr(app.ui_locale, MessageId::HelpUsageLabel),
                cmd.usage
            );
            if !cmd.aliases.is_empty() {
                let _ = write!(
                    help,
                    "\n  {} {}",
                    tr(app.ui_locale, MessageId::HelpAliasesLabel),
                    cmd.aliases.join(", ")
                );
            }
            return CommandResult::message(help);
        }
        return CommandResult::error(
            tr(app.ui_locale, MessageId::HelpUnknownCommand).replace("{topic}", topic),
        );
    }

    // Show help overlay
    if app.view_stack.top_kind() != Some(ModalKind::Help) {
        app.view_stack.push(HelpView::new_for_locale(app.ui_locale));
    }
    CommandResult::ok()
}

/// Clear conversation history
pub fn clear(app: &mut App) -> CommandResult {
    app.clear_history();
    app.mark_history_updated();
    app.api_messages.clear();
    app.system_prompt = None;
    app.viewport.transcript_selection.clear();
    app.queued_messages.clear();
    app.queued_draft = None;
    app.session.total_tokens = 0;
    app.session.total_conversation_tokens = 0;
    app.session.session_cost = 0.0;
    app.session.session_cost_cny = 0.0;
    let todos_cleared = app.clear_todos();
    app.tool_log.clear();
    app.tool_cells.clear();
    app.tool_details_by_cell.clear();
    app.exploring_entries.clear();
    app.ignored_tool_calls.clear();
    app.pending_tool_uses.clear();
    app.last_exec_wait_command = None;
    app.session.last_prompt_tokens = None;
    app.session.last_completion_tokens = None;
    app.session.last_prompt_cache_hit_tokens = None;
    app.session.last_prompt_cache_miss_tokens = None;
    app.session.turn_cache_history.clear();
    app.current_session_id = None;
    let locale = app.ui_locale;
    let message = if todos_cleared {
        tr(locale, MessageId::ClearConversation).to_string()
    } else {
        tr(locale, MessageId::ClearConversationBusy).to_string()
    };
    CommandResult::with_message_and_action(
        message,
        AppAction::SyncSession {
            session_id: None,
            messages: Vec::new(),
            system_prompt: None,
            model: app.model.clone(),
            workspace: app.workspace.clone(),
        },
    )
}

/// Exit the application
pub fn exit() -> CommandResult {
    CommandResult::action(AppAction::Quit)
}

/// Switch or view current model. With no argument, open the two-pane
/// picker (Pro/Flash + thinking effort) per #39 — gives users a discoverable
/// way to flip both knobs without memorising the docs.
pub fn model(app: &mut App, model_name: Option<&str>) -> CommandResult {
    if let Some(name) = model_name {
        if name.trim().eq_ignore_ascii_case("auto") {
            let old_model = app.model_display_label();
            let model_changed = !app.auto_model || app.model != "auto";
            app.auto_model = true;
            app.model = "auto".to_string();
            app.last_effective_model = None;
            app.reasoning_effort = ReasoningEffort::Auto;
            app.last_effective_reasoning_effort = None;
            app.update_model_compaction_budget();
            if model_changed {
                app.clear_model_scoped_telemetry();
            } else {
                app.session.last_prompt_tokens = None;
                app.session.last_completion_tokens = None;
            }
            return CommandResult::with_message_and_action(
                tr(app.ui_locale, MessageId::ModelChanged)
                    .replace("{old}", &old_model)
                    .replace("{new}", "auto"),
                AppAction::UpdateCompaction(app.compaction_config()),
            );
        }
        let Some(model_id) = normalize_model_name(name) else {
            return CommandResult::error(format!(
                "Invalid model '{name}'. Expected auto or a DeepSeek model ID. Common models: {}",
                COMMON_DEEPSEEK_MODELS.join(", ")
            ));
        };
        let old_model = app.model_display_label();
        let model_changed = app.auto_model || app.model != model_id;
        app.auto_model = false;
        app.model = model_id.clone();
        app.last_effective_model = None;
        app.update_model_compaction_budget();
        if model_changed {
            app.clear_model_scoped_telemetry();
        } else {
            app.session.last_prompt_tokens = None;
            app.session.last_completion_tokens = None;
        }
        CommandResult::with_message_and_action(
            tr(app.ui_locale, MessageId::ModelChanged)
                .replace("{old}", &old_model)
                .replace("{new}", &model_id),
            AppAction::UpdateCompaction(app.compaction_config()),
        )
    } else {
        CommandResult::action(AppAction::OpenModelPicker)
    }
}

/// Fetch and list available models from the configured API endpoint.
pub fn models(_app: &mut App) -> CommandResult {
    CommandResult::action(AppAction::FetchModels)
}

/// List sub-agent status from the engine
pub fn subagents(app: &mut App) -> CommandResult {
    if app.view_stack.top_kind() != Some(ModalKind::SubAgents) {
        let agents = subagent_view_agents(app, &app.subagent_cache);
        app.view_stack.push(SubAgentsView::new(agents));
    }
    app.status_message = Some(tr(app.ui_locale, MessageId::SubagentsFetching).to_string());
    CommandResult::action(AppAction::ListSubAgents)
}

/// Switch to a configured profile.
pub fn profile_switch(_app: &mut App, arg: Option<&str>) -> CommandResult {
    let profile_name = match arg {
        Some(name) if !name.trim().is_empty() => name.trim().to_string(),
        _ => {
            return CommandResult::error(
                "Usage: /profile <name>\n\nSwitch to a named config profile. Profiles are defined in ~/.deepseek/config.toml under [profiles] sections.",
            );
        }
    };
    CommandResult::with_message_and_action(
        format!("Switching to profile '{profile_name}'..."),
        AppAction::SwitchProfile {
            profile: profile_name,
        },
    )
}

/// Show `DeepSeek` dashboard and docs links
pub fn deepseek_links(app: &mut App) -> CommandResult {
    let locale = app.ui_locale;
    CommandResult::message(format!(
        "{}\n\
─────────────────────────────\n\
{} https://platform.deepseek.com\n\
{}      https://platform.deepseek.com/docs\n\n\
{}",
        tr(locale, MessageId::LinksTitle),
        tr(locale, MessageId::LinksDashboard),
        tr(locale, MessageId::LinksDocs),
        tr(locale, MessageId::LinksTip),
    ))
}

/// Show home dashboard with stats and quick actions
pub fn home_dashboard(app: &mut App) -> CommandResult {
    let locale = app.ui_locale;
    let mut stats = String::new();
    let novel = crate::novel::workspace_summary(&app.workspace).ok();

    // Basic info
    let _ = writeln!(stats, "{}", tr(locale, MessageId::HomeDashboardTitle));
    let _ = writeln!(stats, "============================================");
    if let Some(novel) = novel {
        let audit_risks = crate::novel::audit_risk_summary(&app.workspace, 3).unwrap_or_default();
        let _ = writeln!(stats, "Book:       {}", novel.title);
        let _ = writeln!(stats, "Genre:      {}", novel.genre);
        let _ = writeln!(
            stats,
            "Assets:     {} chapters, {} drafts, {} finals, {} audits",
            novel.chapters, novel.drafts, novel.finals, novel.audits
        );
        let _ = writeln!(
            stats,
            "Frontier:   volume {}, current chapter {}, next chapter {:03}",
            novel.current_volume, novel.current_chapter, novel.next_chapter
        );
        let _ = writeln!(
            stats,
            "Canon:      {} memory nodes, {} memory edges, {} candidates, {} materials",
            novel.memory_nodes, novel.memory_edges, novel.candidate_updates, novel.materials
        );
        let _ = writeln!(
            stats,
            "Memory:     graph {}, {} candidates, {} summaries",
            if novel.memory_graph_ready {
                "ready"
            } else {
                "missing"
            },
            novel.candidate_updates,
            novel.summaries
        );
        let _ = writeln!(
            stats,
            "Gate:       {} | score {} | next {}",
            novel.readiness.quality_gate,
            novel
                .readiness
                .context_score
                .map(|score| format!("{score}/100"))
                .unwrap_or_else(|| "n/a".to_string()),
            novel.readiness.next_action
        );
        let _ = writeln!(
            stats,
            "Noise:      pending {}, recent {}, max/chapter {}, summary avg {}, max {}, overlong {}, canon sparse {}",
            novel.readiness.pending_candidates,
            novel.readiness.candidate_pressure_total,
            novel.readiness.candidate_pressure_max_per_chapter,
            novel.readiness.recent_summary_avg_chars,
            novel.readiness.recent_summary_max_chars,
            novel.readiness.recent_summary_overweight,
            novel.readiness.recent_summary_canon_sparse
        );
        for blocker in &novel.readiness.blockers {
            let _ = writeln!(stats, "  - blocker: {blocker}");
        }
        for warning in novel.readiness.warnings.iter().take(5) {
            let _ = writeln!(stats, "  - warning: {warning}");
        }
        if !novel.promise_statuses.is_empty() {
            let _ = writeln!(
                stats,
                "Promises:   {}",
                crate::novel::format_promise_status_counts(&novel.promise_statuses)
            );
        }
        if !novel.open_promises.is_empty() {
            let _ = writeln!(stats, "Open promises:");
            for promise in &novel.open_promises {
                let _ = writeln!(stats, "  - {promise}");
            }
        }
        if novel.relationship_changes > 0 || novel.state_changes > 0 {
            let _ = writeln!(
                stats,
                "Continuity: {} relationship changes, {} state changes",
                novel.relationship_changes, novel.state_changes
            );
            for preview in &novel.relationship_previews {
                let _ = writeln!(stats, "  - relationship: {preview}");
            }
            for preview in &novel.state_change_previews {
                let _ = writeln!(stats, "  - state: {preview}");
            }
        }
        if audit_risks.blockers > 0
            || audit_risks.majors > 0
            || audit_risks.affected_nodes > 0
            || audit_risks.pending_candidates > 0
        {
            let _ = writeln!(
                stats,
                "Risks:      {} blockers, {} majors, {} affected nodes, {} pending memory candidates",
                audit_risks.blockers,
                audit_risks.majors,
                audit_risks.affected_nodes,
                audit_risks.pending_candidates
            );
            for risk in &audit_risks.risk_previews {
                let _ = writeln!(stats, "  - {risk}");
            }
            if !audit_risks.layered_counts.is_empty() {
                let _ = writeln!(
                    stats,
                    "Risk categories: {}",
                    crate::novel::format_audit_layer_counts(&audit_risks.layered_counts)
                );
            }
            for risk in &audit_risks.layered_previews {
                let _ = writeln!(stats, "  - layered: {risk}");
            }
            for affected in &audit_risks.affected_previews {
                let _ = writeln!(stats, "  - affected: {affected}");
            }
            for candidate in &audit_risks.candidate_previews {
                let _ = writeln!(stats, "  - pending memory: {candidate}");
            }
        }
        let _ = writeln!(stats, "Next:       {}", novel.readiness.next_action);
    } else {
        let _ = writeln!(stats, "Book:       not initialized (run /init)");
    }

    // Model & mode
    let _ = writeln!(
        stats,
        "{}      {}",
        tr(locale, MessageId::HomeModel),
        app.model
    );
    let _ = writeln!(
        stats,
        "{}       {}",
        tr(locale, MessageId::HomeMode),
        app.mode.label()
    );
    let _ = writeln!(
        stats,
        "{}  {}",
        tr(locale, MessageId::HomeWorkspace),
        app.workspace.display()
    );

    // Session stats
    let history_count = app.history.len();
    let total_tokens = app.session.total_conversation_tokens;
    let queued_messages = app.queued_messages.len();
    let _ = writeln!(
        stats,
        "{}    {} messages",
        tr(locale, MessageId::HomeHistory),
        history_count
    );
    let _ = writeln!(
        stats,
        "{}     {} (session)",
        tr(locale, MessageId::HomeTokens),
        total_tokens
    );
    if queued_messages > 0 {
        let _ = writeln!(
            stats,
            "{}     {} messages",
            tr(locale, MessageId::HomeQueued),
            queued_messages
        );
    }

    // Sub-agents
    let subagent_count = app.subagent_cache.len();
    if subagent_count > 0 {
        let _ = writeln!(
            stats,
            "{} {} active",
            tr(locale, MessageId::HomeSubagents),
            subagent_count
        );
    }

    // Active skill
    if let Some(skill) = &app.active_skill {
        let _ = writeln!(
            stats,
            "{}      {} (active)",
            tr(locale, MessageId::HomeSkill),
            skill
        );
    }

    // Quick actions section
    let _ = writeln!(stats, "\n{}", tr(locale, MessageId::HomeQuickActions));
    let _ = writeln!(stats, "--------------------------------------------");
    let _ = writeln!(stats, "/status              show book and memory status");
    let _ = writeln!(stats, "/map                 show the novel project map");
    let _ = writeln!(stats, "/memory context N    prepare chapter context");
    let _ = writeln!(stats, "/brief N             build a chapter brief");
    let _ = writeln!(stats, "/write N             draft a chapter");
    let _ = writeln!(stats, "/audit N             diagnose continuity risks");
    let _ = writeln!(stats, "/revise N            revise with version protection");
    let _ = writeln!(stats, "/chapter-diff N      inspect revision changes");
    let _ = writeln!(
        stats,
        "/chapter-undo N      restore the latest chapter version"
    );
    let _ = writeln!(
        stats,
        "/remember N          extract reviewable memory candidates"
    );
    let _ = writeln!(
        stats,
        "Runtime:             /model, /settings, /subagents, /task list, /help"
    );

    // Mode-specific tips
    let _ = writeln!(stats, "\n{}", tr(locale, MessageId::HomeModeTips));
    let _ = writeln!(stats, "--------------------------------------------");
    match app.mode {
        AppMode::Agent => {
            let _ = writeln!(stats, "{}", tr(locale, MessageId::HomeAgentModeTip));
            let _ = writeln!(stats, "{}", tr(locale, MessageId::HomeAgentModeReviewTip));
            let _ = writeln!(stats, "{}", tr(locale, MessageId::HomeAgentModeYoloTip));
        }
        AppMode::Yolo => {
            let _ = writeln!(stats, "{}", tr(locale, MessageId::HomeYoloModeTip));
            let _ = writeln!(stats, "{}", tr(locale, MessageId::HomeYoloModeCaution));
        }
        AppMode::Plan => {
            let _ = writeln!(stats, "{}", tr(locale, MessageId::HomePlanModeTip));
            let _ = writeln!(stats, "{}", tr(locale, MessageId::HomePlanModeChecklistTip));
        }
    }

    CommandResult::message(stats)
}

/// Toggle output translation to the current system language on/off.
///
/// When enabled, the model is instructed to respond in the current locale and an
/// interception layer translates any remaining English output before it
/// reaches the user.
pub fn translate(app: &mut App) -> CommandResult {
    app.translation_enabled = !app.translation_enabled;
    let locale = app.ui_locale;
    if app.translation_enabled {
        CommandResult::message(tr(locale, MessageId::CmdTranslateOn))
    } else {
        CommandResult::message(tr(locale, MessageId::CmdTranslateOff))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::models::Message;
    use crate::tui::app::{App, AppMode, TuiOptions, TurnCacheRecord};
    use crate::tui::history::HistoryCell;
    use std::path::PathBuf;
    use std::time::Instant;

    fn create_test_app() -> App {
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: PathBuf::from("/tmp/test-workspace"),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: PathBuf::from("/tmp/test-skills"),
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
        let mut app = App::new(options, &Config::default());
        app.ui_locale = crate::localization::Locale::En;
        app.api_provider = crate::config::ApiProvider::Deepseek;
        app
    }

    #[test]
    fn test_help_unknown_command() {
        let mut app = create_test_app();
        let result = help(&mut app, Some("nonexistent"));
        assert!(result.message.is_some());
        assert!(result.message.unwrap().contains("Unknown command"));
        assert!(result.action.is_none());
    }

    #[test]
    fn test_help_known_command() {
        let mut app = create_test_app();
        let result = help(&mut app, Some("clear"));
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("clear"));
        assert!(msg.contains("Clear the current workspace conversation"));
        assert!(msg.contains("Usage: /clear"));
    }

    #[test]
    fn test_help_config_topic_uses_interactive_editor_text() {
        let mut app = create_test_app();
        let result = help(&mut app, Some("config"));
        let msg = result.message.expect("help topic should return message");
        assert!(msg.contains("config"));
        assert!(msg.contains("Open interactive configuration editor"));
        assert!(msg.contains("Usage: /config"));
    }

    #[test]
    fn test_help_links_topic_shows_aliases() {
        let mut app = create_test_app();
        let result = help(&mut app, Some("links"));
        let msg = result.message.expect("help topic should return message");
        assert!(msg.contains("links"));
        assert!(msg.contains("Show DeepSeek dashboard and docs links"));
        assert!(msg.contains("Usage: /links"));
        assert!(msg.contains("Aliases: dashboard, api"));
    }

    #[test]
    fn test_help_memory_topic_shows_usage_and_description() {
        let mut app = create_test_app();
        let result = help(&mut app, Some("memory"));
        let msg = result.message.expect("help topic should return message");
        assert!(msg.contains("memory"));
        assert!(msg.contains("long-form writing memory"));
        assert!(msg.contains("Usage: /memory [status|build|reports|promises|archive|regression"));
        assert!(msg.contains("context <chapter>"));
    }

    #[test]
    fn test_help_pushes_overlay() {
        let mut app = create_test_app();
        assert_ne!(app.view_stack.top_kind(), Some(ModalKind::Help));
        let result = help(&mut app, None);
        assert_eq!(result.message, None);
        assert_eq!(result.action, None);
        assert_eq!(app.view_stack.top_kind(), Some(ModalKind::Help));
    }

    #[test]
    fn test_help_does_not_duplicate_overlay() {
        let mut app = create_test_app();
        help(&mut app, None);
        let initial_kind = app.view_stack.top_kind();
        help(&mut app, None);
        assert_eq!(app.view_stack.top_kind(), initial_kind);
    }

    #[test]
    fn test_clear_resets_all_state() {
        let mut app = create_test_app();
        // Set up some state
        app.history.push(HistoryCell::User {
            content: "test".to_string(),
        });
        app.api_messages.push(Message {
            role: "user".to_string(),
            content: vec![],
        });
        app.session.total_conversation_tokens = 100;
        app.tool_log.push("test".to_string());
        app.current_session_id = Some("existing-session".to_string());
        app.session_artifacts
            .push(crate::artifacts::ArtifactRecord {
                id: "art_call_big".to_string(),
                kind: crate::artifacts::ArtifactKind::ToolOutput,
                session_id: "existing-session".to_string(),
                tool_call_id: "call-big".to_string(),
                tool_name: "exec_shell".to_string(),
                created_at: chrono::Utc::now(),
                byte_size: 128,
                preview: "tool output".to_string(),
                storage_path: PathBuf::from("/tmp/tool_outputs/call-big.txt"),
            });

        let result = clear(&mut app);
        assert!(result.message.is_some());
        assert!(app.history.is_empty());
        assert!(app.api_messages.is_empty());
        assert_eq!(app.session.total_conversation_tokens, 0);
        assert!(app.tool_log.is_empty());
        assert!(app.tool_cells.is_empty());
        assert!(app.tool_details_by_cell.is_empty());
        assert!(app.session_artifacts.is_empty());
        assert!(app.current_session_id.is_none());
        assert!(matches!(result.action, Some(AppAction::SyncSession { .. })));
    }

    #[test]
    fn clear_resets_session_telemetry() {
        let mut app = create_test_app();
        app.session.total_tokens = 234;
        app.session.total_conversation_tokens = 123;
        app.session.session_cost = 0.42;
        app.session.session_cost_cny = 3.05;
        app.session.last_prompt_cache_hit_tokens = Some(70);
        app.session.last_prompt_cache_miss_tokens = Some(30);
        app.push_turn_cache_record(TurnCacheRecord {
            input_tokens: 100,
            output_tokens: 25,
            cache_hit_tokens: Some(70),
            cache_miss_tokens: Some(30),
            reasoning_replay_tokens: Some(12),
            recorded_at: Instant::now(),
        });

        clear(&mut app);

        assert_eq!(app.session.total_tokens, 0);
        assert_eq!(app.session.total_conversation_tokens, 0);
        assert_eq!(app.session.session_cost, 0.0);
        assert_eq!(app.session.session_cost_cny, 0.0);
        assert_eq!(app.session.last_prompt_cache_hit_tokens, None);
        assert_eq!(app.session.last_prompt_cache_miss_tokens, None);
        assert!(app.session.turn_cache_history.is_empty());
    }

    #[test]
    fn test_exit_returns_quit_action() {
        let result = exit();
        assert!(result.message.is_none());
        assert!(matches!(result.action, Some(AppAction::Quit)));
    }

    #[test]
    fn test_model_change_updates_state() {
        let mut app = create_test_app();
        let old_model = app.model.clone();
        let result = model(&mut app, Some("deepseek-v4-flash"));
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains(&old_model));
        assert!(msg.contains("deepseek-v4-flash"));
        assert!(matches!(
            result.action,
            Some(AppAction::UpdateCompaction(_))
        ));
        assert_eq!(app.model, "deepseek-v4-flash");
        assert_eq!(app.session.last_prompt_tokens, None);
        assert_eq!(app.session.last_completion_tokens, None);
    }

    #[test]
    fn model_switch_clears_turn_cache_history() {
        let mut app = create_test_app();
        app.push_turn_cache_record(TurnCacheRecord {
            input_tokens: 100,
            output_tokens: 25,
            cache_hit_tokens: Some(70),
            cache_miss_tokens: Some(30),
            reasoning_replay_tokens: Some(12),
            recorded_at: Instant::now(),
        });

        let result = model(&mut app, Some("deepseek-v4-flash"));

        assert!(result.message.is_some());
        assert!(app.session.turn_cache_history.is_empty());
    }

    #[test]
    fn model_reset_same_model_keeps_turn_cache_history() {
        let mut app = create_test_app();
        app.auto_model = false;
        app.model = "deepseek-v4-pro".to_string();
        app.push_turn_cache_record(TurnCacheRecord {
            input_tokens: 100,
            output_tokens: 25,
            cache_hit_tokens: Some(70),
            cache_miss_tokens: Some(30),
            reasoning_replay_tokens: Some(12),
            recorded_at: Instant::now(),
        });

        let result = model(&mut app, Some("deepseek-v4-pro"));

        assert!(result.message.is_some());
        assert_eq!(app.session.turn_cache_history.len(), 1);
    }

    #[test]
    fn test_model_auto_enables_auto_thinking() {
        let mut app = create_test_app();
        app.reasoning_effort = ReasoningEffort::Off;

        let result = model(&mut app, Some("auto"));

        assert!(result.message.is_some());
        assert!(app.auto_model);
        assert_eq!(app.model, "auto");
        assert_eq!(app.reasoning_effort, ReasoningEffort::Auto);
        assert!(app.last_effective_model.is_none());
        assert!(app.last_effective_reasoning_effort.is_none());
    }

    #[test]
    fn test_model_change_accepts_future_deepseek_model() {
        let mut app = create_test_app();
        let result = model(&mut app, Some("deepseek-v4"));
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("deepseek-v4"));
        assert_eq!(app.model, "deepseek-v4");
        assert!(matches!(
            result.action,
            Some(AppAction::UpdateCompaction(_))
        ));
    }

    #[test]
    fn test_model_change_rejects_invalid_model() {
        let mut app = create_test_app();
        let result = model(&mut app, Some("gpt-4"));
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("Invalid model"));
        assert!(msg.contains("DeepSeek model ID"));
        assert!(msg.contains("deepseek-v4-pro"));
        assert!(msg.contains("deepseek-v4-flash"));
        assert!(result.action.is_none());
    }

    #[test]
    fn test_model_without_args_opens_picker() {
        let mut app = create_test_app();
        let result = model(&mut app, None);
        assert_eq!(result.message, None);
        assert_eq!(result.action, Some(AppAction::OpenModelPicker));
    }

    #[test]
    fn test_models_triggers_fetch_action() {
        let mut app = create_test_app();
        let result = models(&mut app);
        assert!(result.message.is_none());
        assert!(matches!(result.action, Some(AppAction::FetchModels)));
    }

    #[test]
    fn test_subagents_pushes_view_and_sets_status() {
        let mut app = create_test_app();
        let result = subagents(&mut app);
        assert!(result.message.is_none());
        assert!(matches!(result.action, Some(AppAction::ListSubAgents)));
        assert_eq!(app.view_stack.top_kind(), Some(ModalKind::SubAgents));
        assert_eq!(
            app.status_message,
            Some("Fetching sub-agent status...".to_string())
        );
    }

    #[test]
    fn test_deepseek_links() {
        let mut app = create_test_app();
        let result = deepseek_links(&mut app);
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("DeepSeek Links"));
        assert!(msg.contains("https://platform.deepseek.com"));
        assert!(result.action.is_none());
    }

    #[test]
    fn test_home_dashboard_includes_all_sections() {
        let mut app = create_test_app();
        app.session.total_conversation_tokens = 1234;
        let result = home_dashboard(&mut app);
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("Writer"));
        assert!(msg.contains("Book:"));
        assert!(msg.contains("Model:"));
        assert!(msg.contains("Mode:"));
        assert!(msg.contains("Workspace:"));
        assert!(msg.contains("Turns:"));
        assert!(msg.contains("Context:"));
        assert!(msg.contains("Quick Actions"));
        assert!(msg.contains("Mode Tips"));
        assert!(result.action.is_none());
    }

    #[test]
    fn test_home_dashboard_shows_queued_when_present() {
        let mut app = create_test_app();
        app.queued_messages
            .push_back(crate::tui::app::QueuedMessage::new(
                "test".to_string(),
                None,
            ));
        let result = home_dashboard(&mut app);
        let msg = result.message.unwrap();
        assert!(msg.contains("Queued:"));
    }

    #[test]
    fn test_home_dashboard_mode_tips_for_each_mode() {
        let modes = [AppMode::Agent, AppMode::Yolo, AppMode::Plan];
        for mode in modes {
            let mut app = create_test_app();
            app.mode = mode;
            let result = home_dashboard(&mut app);
            let msg = result.message.unwrap();
            assert!(msg.contains("Mode Tips"), "Missing tips for mode {mode:?}");
        }
    }

    #[test]
    fn test_home_dashboard_quick_actions_prioritize_novel_workflow_and_hide_removed_commands() {
        let mut app = create_test_app();
        let result = home_dashboard(&mut app);
        let msg = result
            .message
            .expect("home dashboard should return message");
        assert!(msg.contains("/write N"));
        assert!(msg.contains("/revise N"));
        assert!(msg.contains("/chapter-diff N"));
        assert!(msg.contains("/chapter-undo N"));
        assert!(msg.contains("/memory"));
        assert!(msg.contains("Runtime:"));
        assert!(
            !msg.lines()
                .any(|line| line.trim_start().starts_with("/set "))
        );
        assert!(
            !msg.lines()
                .any(|line| line.trim_start().starts_with("/init"))
        );
        assert!(
            !msg.lines()
                .any(|line| line.trim_start().starts_with("/links"))
        );
        assert!(
            !msg.lines()
                .any(|line| line.trim_start().starts_with("/config"))
        );
        assert!(!msg.contains("/deepseek"));
    }

    #[test]
    fn home_dashboard_shows_novel_frontier_promises_and_next_action() {
        let tmpdir = tempfile::TempDir::new().expect("temp dir");
        crate::novel::initialize_project(
            tmpdir.path(),
            crate::novel::NovelInitOptions {
                title: Some("首页测试".to_string()),
                genre: Some("悬疑".to_string()),
                premise: None,
                target_words: 80_000,
                language: "zh-CN".to_string(),
                force: false,
            },
        )
        .expect("init project");
        std::fs::create_dir_all(tmpdir.path().join("chapters/001")).expect("chapter dir");
        std::fs::write(tmpdir.path().join("chapters/001/draft.md"), "草稿").expect("draft");
        std::fs::write(
            tmpdir.path().join("materials/notes/case.md"),
            "素材只作参考。",
        )
        .expect("material");
        std::fs::write(
            tmpdir.path().join("chapters/001/audit.md"),
            "## BLOCKER\n- 林墨知道了尚未揭露的事故真相\n\n## MAJOR\n- 事故真相伏笔被提前回收\n\n## AFFECTED_NODES\n- character:lin_mo\n- promise:accident_truth\n\n## CANDIDATE_MEMORY_UPDATES\n{\"chapter\":1,\"kind\":\"promise\",\"target\":\"事故真相\",\"change\":\"推进为林墨主动追查\",\"evidence\":\"chapters/001/draft.md:12\",\"confidence\":0.87,\"affects\":[\"promise:accident_truth\"]}\n",
        )
        .expect("audit");
        std::fs::write(
            tmpdir.path().join("memory/foreshadowing.jsonl"),
            "{\"promise\":\"事故真相\",\"status\":\"open\",\"first_chapter\":1}\n",
        )
        .expect("promise");
        std::fs::write(
            tmpdir.path().join("memory/facts.jsonl"),
            "{\"kind\":\"relationship\",\"target\":\"林墨/陈岚\",\"change\":\"从互相试探升级为公开对峙\",\"chapter\":1}\n{\"kind\":\"character_state\",\"target\":\"林墨\",\"change\":\"开始主动追查事故真相\",\"chapter\":1}\n",
        )
        .expect("facts");
        crate::novel::rebuild_memory_graph(tmpdir.path()).expect("rebuild memory graph");
        let mut app = create_test_app();
        app.workspace = tmpdir.path().to_path_buf();

        let result = home_dashboard(&mut app);
        let msg = result.message.expect("home dashboard");

        assert!(msg.contains("Book:       首页测试"));
        assert!(msg.contains("Frontier:   volume 1, current chapter 1, next chapter 002"));
        assert!(msg.contains("1 materials"));
        assert!(msg.contains("Promises:   open=1"));
        assert!(msg.contains("Continuity: 1 relationship changes, 1 state changes"));
        assert!(msg.contains("relationship: chapter 001: 林墨/陈岚 -> 从互相试探升级为公开对峙"));
        assert!(msg.contains("state: chapter 001: 林墨 -> 开始主动追查事故真相 [character_state]"));
        assert!(msg.contains("Open promises:"));
        assert!(msg.contains("事故真相 | open | chapter 001"));
        assert!(msg.contains(
            "Risks:      1 blockers, 1 majors, 2 affected nodes, 1 pending memory candidates"
        ));
        assert!(msg.contains("林墨知道了尚未揭露的事故真相"));
        assert!(msg.contains("affected: chapter 001: character:lin_mo"));
        assert!(
            msg.contains("pending memory: chapter 001 [promise] 事故真相 -> 推进为林墨主动追查")
        );
        assert!(msg.contains("Gate:"));
        assert!(msg.contains("Noise:"));
        assert!(msg.contains("Next:       deepseek plan"));
        assert!(msg.contains("/map"));
    }

    #[test]
    fn home_dashboard_localizes_in_zh_hans() {
        use crate::localization::Locale;
        let mut app = create_test_app();
        app.ui_locale = Locale::ZhHans;
        let result = home_dashboard(&mut app);
        let msg = result
            .message
            .expect("home dashboard should return message");
        assert!(msg.contains("Writer"), "missing product title:\n{msg}");
        assert!(msg.contains("模型"), "missing zh-Hans model label:\n{msg}");
        assert!(
            msg.contains("快捷操作"),
            "missing zh-Hans quick actions:\n{msg}"
        );
        assert!(
            msg.contains("模式提示"),
            "missing zh-Hans mode tips:\n{msg}"
        );
    }
}
