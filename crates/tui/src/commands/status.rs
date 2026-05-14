//! Runtime status command.

use std::fmt::Write as _;
use std::path::Path;

use super::CommandResult;
use crate::compaction::estimate_input_tokens_conservative;
use crate::models::{LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS, context_window_for_model};
use crate::tui::app::App;
use crate::utils::{display_path, estimate_message_chars};

/// Show a compact runtime status report for the current TUI session.
pub fn status(app: &mut App) -> CommandResult {
    CommandResult::message(format_status(app))
}

fn format_status(app: &App) -> String {
    let mut out = String::new();
    let (context_used, context_max, context_percent) = context_usage(app);

    let novel = crate::novel::workspace_summary(&app.workspace).ok();

    let _ = writeln!(out, "DeepSeek Novel Studio Status");
    let _ = writeln!(out, "============================");
    let _ = writeln!(out);
    if let Some(novel) = &novel {
        let audit_risks = crate::novel::audit_risk_summary(&app.workspace, 3).unwrap_or_default();
        push_row(&mut out, "Book:", &novel.title);
        push_row(&mut out, "Genre:", &novel.genre);
        push_row(
            &mut out,
            "Manuscript:",
            &format!(
                "{} chapters, {} drafts, {} finals, {} audits",
                novel.chapters, novel.drafts, novel.finals, novel.audits
            ),
        );
        push_row(
            &mut out,
            "Book memory:",
            &format!(
                "{} summaries, graph {}, {} nodes / {} edges",
                novel.summaries,
                if novel.memory_graph_ready {
                    "ready"
                } else {
                    "missing"
                },
                novel.memory_nodes,
                novel.memory_edges
            ),
        );
        push_row(
            &mut out,
            "Memory updates:",
            &format!("{} candidates", novel.candidate_updates),
        );
        if !novel.promise_statuses.is_empty() {
            push_row(
                &mut out,
                "Promises:",
                &crate::novel::format_promise_status_counts(&novel.promise_statuses),
            );
        }
        push_row(
            &mut out,
            "Continuity:",
            &format!(
                "{} relationship changes, {} state changes",
                novel.relationship_changes, novel.state_changes
            ),
        );
        for preview in &novel.relationship_previews {
            push_row(&mut out, "Relationship:", preview);
        }
        for preview in &novel.state_change_previews {
            push_row(&mut out, "State change:", preview);
        }
        push_row(
            &mut out,
            "Local materials:",
            &format!("{} reference files", novel.materials),
        );
        push_row(
            &mut out,
            "Audit risks:",
            &format!(
                "{} blockers, {} majors, {} affected nodes, {} pending candidates",
                audit_risks.blockers,
                audit_risks.majors,
                audit_risks.affected_nodes,
                audit_risks.pending_candidates
            ),
        );
        for risk in &audit_risks.risk_previews {
            push_row(&mut out, "Risk:", risk);
        }
        if !audit_risks.layered_counts.is_empty() {
            push_row(
                &mut out,
                "Audit categories:",
                &crate::novel::format_audit_layer_counts(&audit_risks.layered_counts),
            );
        }
        for risk in &audit_risks.layered_previews {
            push_row(&mut out, "Layered risk:", risk);
        }
        for affected in &audit_risks.affected_previews {
            push_row(&mut out, "Affected:", affected);
        }
        for candidate in &audit_risks.candidate_previews {
            push_row(&mut out, "Candidate:", candidate);
        }
    } else {
        push_row(&mut out, "Book:", "not initialized (run /init)");
    }
    push_row(&mut out, "Version:", env!("CARGO_PKG_VERSION"));
    push_row(&mut out, "Provider:", app.api_provider.as_str());
    push_row(
        &mut out,
        "Model:",
        &format!(
            "{} (reasoning {})",
            app.model_display_label(),
            app.reasoning_effort_display_label()
        ),
    );
    push_row(&mut out, "Directory:", &display_path(&app.workspace));
    push_row(&mut out, "Mode:", app.mode.label());
    push_row(&mut out, "Permissions:", &permission_summary(app));
    push_row(&mut out, "Book assets:", &book_assets(&app.workspace));
    push_row(
        &mut out,
        "Session:",
        app.current_session_id.as_deref().unwrap_or("not saved yet"),
    );
    push_row(
        &mut out,
        "MCP:",
        &format!("{} configured", app.mcp_configured_count),
    );
    push_row(&mut out, "Footer items:", &footer_items(app));
    let _ = writeln!(out);
    push_row(
        &mut out,
        "Context window:",
        &format!("{context_percent:.1}% used ({context_used} / {context_max} tokens)"),
    );
    push_row(
        &mut out,
        "Last API input:",
        &token_count(app.session.last_prompt_tokens),
    );
    push_row(
        &mut out,
        "Last API output:",
        &token_count(app.session.last_completion_tokens),
    );
    push_row(&mut out, "Cache hit/miss:", &cache_summary(app));
    push_row(
        &mut out,
        "Total tokens:",
        &app.session.total_tokens.to_string(),
    );
    push_row(
        &mut out,
        "Session cost:",
        &app.format_cost_amount_precise(app.session_cost_for_currency(app.cost_currency)),
    );
    push_row(
        &mut out,
        "Transcript:",
        &format!(
            "{} cells, {} API messages",
            app.history.len(),
            app.api_messages.len()
        ),
    );
    push_row(
        &mut out,
        "Rate limits:",
        "not available from provider telemetry",
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "Use /home for the writing dashboard and /memory for long-form memory."
    );

    out
}

fn push_row(out: &mut String, label: &str, value: &str) {
    let _ = writeln!(out, "  {label:<16} {value}");
}

fn permission_summary(app: &App) -> String {
    let trust = if app.trust_mode {
        "trusted workspace"
    } else {
        "workspace"
    };
    let shell = if app.allow_shell {
        "shell on"
    } else {
        "shell off"
    };
    format!(
        "{trust}, approvals {}, {shell}",
        app.approval_mode.label().to_ascii_lowercase()
    )
}

fn book_assets(workspace: &Path) -> String {
    let docs: Vec<&str> = [
        "book.toml",
        "bible",
        "cards",
        "outline",
        "chapters",
        "memory",
    ]
    .into_iter()
    .filter(|name| {
        let path = workspace.join(name);
        path.is_file() || path.is_dir()
    })
    .collect();
    if docs.is_empty() {
        "not found".to_string()
    } else {
        docs.join(", ")
    }
}

fn footer_items(app: &App) -> String {
    if app.status_items.is_empty() {
        return "none".to_string();
    }
    app.status_items
        .iter()
        .map(|item| item.key())
        .collect::<Vec<_>>()
        .join(", ")
}

fn context_usage(app: &App) -> (usize, u32, f64) {
    let max = context_window_for_model(&app.model).unwrap_or(LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS);
    let estimated =
        estimate_input_tokens_conservative(&app.api_messages, app.system_prompt.as_ref());
    let total_chars = estimate_message_chars(&app.api_messages);
    let used = estimated.max(total_chars / 4);
    let percent = ((used as f64 / f64::from(max)) * 100.0).clamp(0.0, 100.0);
    (used, max, percent)
}

fn token_count(value: Option<u32>) -> String {
    value.map_or_else(|| "not reported".to_string(), |tokens| tokens.to_string())
}

fn cache_summary(app: &App) -> String {
    match (
        app.session.last_prompt_cache_hit_tokens,
        app.session.last_prompt_cache_miss_tokens,
    ) {
        (Some(hit), Some(miss)) => format!("{hit} hit / {miss} miss"),
        (Some(hit), None) => format!("{hit} hit / miss not reported"),
        (None, Some(miss)) => format!("hit not reported / {miss} miss"),
        (None, None) => "not reported".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tempfile::TempDir;

    use super::*;
    use crate::config::{ApiProvider, Config};
    use crate::models::{ContentBlock, Message};
    use crate::tui::app::TuiOptions;
    use crate::tui::history::HistoryCell;

    fn create_test_app(workspace: PathBuf) -> App {
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace,
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
        app.api_provider = ApiProvider::Deepseek;
        app
    }

    #[test]
    fn status_report_includes_runtime_fields() {
        let tmpdir = TempDir::new().expect("temp dir");
        crate::novel::initialize_project(
            tmpdir.path(),
            crate::novel::NovelInitOptions {
                title: Some("状态测试".to_string()),
                genre: Some("悬疑".to_string()),
                premise: None,
                target_words: 100_000,
                language: "zh-CN".to_string(),
                force: false,
            },
        )
        .expect("init novel");
        std::fs::create_dir_all(tmpdir.path().join("chapters/001")).expect("chapter dir");
        std::fs::write(
            tmpdir.path().join("chapters/001/audit.md"),
            "## CONTINUITY_AUDIT\n- 人物不知道事故线索却提前行动\n\n## CRAFT_AUDIT\n- 对话声口重复\n\n## BLOCKER\n- 人物知识边界泄漏\n\n## MAJOR\n- 事故真相伏笔断裂\n\n## AFFECTED_NODES\n{\"characters\":[\"林墨\"],\"promises\":[\"事故真相\"]}\n\n## CANDIDATE_MEMORY_UPDATES\n{\"chapter\":1,\"kind\":\"knowledge\",\"target\":\"林墨\",\"change\":\"发现事故线索\",\"evidence\":\"chapters/001/final.md\",\"confidence\":0.9,\"affects\":[\"promise:accident\"]}\n",
        )
        .expect("audit");
        std::fs::write(
            tmpdir.path().join("materials/notes/case.md"),
            "素材只作参考。",
        )
        .expect("material");
        std::fs::write(
            tmpdir.path().join("memory/foreshadowing.jsonl"),
            "{\"kind\":\"promise\",\"promise\":\"事故真相\",\"status\":\"progress\",\"first_chapter\":1}\n",
        )
        .expect("promise");
        std::fs::write(
            tmpdir.path().join("memory/facts.jsonl"),
            "{\"kind\":\"relationship\",\"target\":\"林墨/陈岚\",\"change\":\"从合作转为互相隐瞒\",\"chapter\":1}\n{\"kind\":\"character_state\",\"target\":\"林墨\",\"change\":\"掌握事故线索但没有告诉陈岚\",\"chapter\":1}\n",
        )
        .expect("facts");
        crate::novel::rebuild_memory_graph(tmpdir.path()).expect("rebuild memory graph");
        let mut app = create_test_app(tmpdir.path().to_path_buf());
        app.current_session_id = Some("session-123".to_string());
        app.session.total_tokens = 1234;
        app.session.last_prompt_tokens = Some(100);
        app.session.last_completion_tokens = Some(25);
        app.session.last_prompt_cache_hit_tokens = Some(70);
        app.session.last_prompt_cache_miss_tokens = Some(30);
        app.api_messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: "hello".to_string(),
                cache_control: None,
            }],
        });
        app.history.push(HistoryCell::User {
            content: "hello".to_string(),
        });

        let result = status(&mut app);
        let msg = result.message.expect("status message");
        assert!(msg.contains("DeepSeek Novel Studio Status"));
        assert!(msg.contains("Book:"));
        assert!(msg.contains("状态测试"));
        assert!(msg.contains("Provider:"));
        assert!(msg.contains("Model:"));
        assert!(msg.contains("Directory:"));
        assert!(msg.contains("Permissions:"));
        assert!(msg.contains("Book assets:"));
        assert!(msg.contains("book.toml"));
        assert!(msg.contains("Local materials:"));
        assert!(msg.contains("1 reference files"));
        assert!(msg.contains("Promises:"));
        assert!(msg.contains("progress=1"));
        assert!(msg.contains("Continuity:"));
        assert!(msg.contains("1 relationship changes, 1 state changes"));
        assert!(msg.contains("Relationship:"));
        assert!(msg.contains("林墨/陈岚 -> 从合作转为互相隐瞒"));
        assert!(msg.contains("State change:"));
        assert!(msg.contains("林墨 -> 掌握事故线索但没有告诉陈岚 [character_state]"));
        assert!(msg.contains("Audit risks:"));
        assert!(msg.contains("1 blockers, 1 majors, 2 affected nodes, 1 pending candidates"));
        assert!(msg.contains("Audit categories:"));
        assert!(msg.contains("continuity=1, craft=1"));
        assert!(msg.contains("Layered risk:"));
        assert!(msg.contains("人物不知道事故线索却提前行动"));
        assert!(msg.contains("人物知识边界泄漏"));
        assert!(msg.contains("Affected:"));
        assert!(msg.contains("chapter 001: 林墨"));
        assert!(msg.contains("Candidate:"));
        assert!(msg.contains("[knowledge] 林墨 -> 发现事故线索"));
        assert!(msg.contains("Session:"));
        assert!(msg.contains("session-123"));
        assert!(msg.contains("Context window:"));
        assert!(msg.contains("Cache hit/miss:"));
        assert!(msg.contains("70 hit / 30 miss"));
        assert!(msg.contains("Use /home for the writing dashboard"));
    }

    #[test]
    fn book_assets_reports_missing_assets() {
        let tmpdir = TempDir::new().expect("temp dir");
        assert_eq!(book_assets(tmpdir.path()), "not found");
    }
}
