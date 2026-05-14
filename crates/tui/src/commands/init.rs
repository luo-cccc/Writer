//! /init command - create a local long-form novel workspace

use std::io::Read;
use std::path::Path;

use crate::novel::{self, NovelInitOptions};
use crate::tui::app::App;

use super::CommandResult;

/// Create a novel workspace in the current directory.
pub fn init(app: &mut App) -> CommandResult {
    let workspace = &app.workspace;

    // Ensure .deepseek/ is gitignored if we're inside a git repo.
    ensure_deepseek_gitignored(workspace);

    let manifest_path = workspace.join("book.toml");
    if manifest_path.exists() {
        return CommandResult::message(format!(
            "Novel workspace already exists at {}. Use `deepseek init --force` from the shell to refresh templates.",
            manifest_path.display()
        ));
    }

    match novel::initialize_project(
        workspace,
        NovelInitOptions {
            title: None,
            genre: None,
            premise: None,
            target_words: 800_000,
            language: "zh-CN".to_string(),
            force: false,
        },
    ) {
        Ok(outcome) => CommandResult::message(format!(
            "Created novel workspace at {}\n\nTitle: {}\nManifest: {}\nMemory graph: {}\nNext: deepseek plan",
            outcome.root.display(),
            outcome.title,
            outcome.manifest_path.display(),
            outcome.memory_graph_path.display()
        )),
        Err(e) => CommandResult::error(format!("Failed to create novel workspace: {e}")),
    }
}

/// If `workspace` is inside a git repository, ensure `.deepseek/` is listed
/// in the nearest `.gitignore` so that snapshots, instructions, and other
/// workspace-local state are not accidentally committed.
fn ensure_deepseek_gitignored(workspace: &Path) {
    // Only act if this workspace is a git repo.
    if !workspace.join(".git").exists() {
        return;
    }

    let gitignore = workspace.join(".gitignore");
    let entry = ".deepseek/";

    // Read existing contents (if any) and check whether the entry is already present.
    // Check both with and without trailing slash to catch variants like
    // ".deepseek" and ".deepseek/".
    if let Ok(existing) = std::fs::read_to_string(&gitignore) {
        let entry_no_slash = entry.trim_end_matches('/');
        if existing.lines().any(|line| {
            let trimmed = line.trim();
            trimmed == entry || trimmed == entry_no_slash
        }) {
            return; // already ignored
        }
    }

    // Append the entry. If .gitignore doesn't exist yet, create it with a header.
    // Ensure there's a trailing newline before our entry to avoid joining with
    // a previous unterminated line.
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&gitignore)
    {
        // If the file is non-empty and doesn't end with a newline, add one first.
        if let Ok(meta) = file.metadata()
            && meta.len() > 0
        {
            // Read last byte to check for trailing newline.
            if let Ok(mut f) = std::fs::File::open(&gitignore) {
                use std::io::Seek;
                if f.seek(std::io::SeekFrom::End(-1)).is_ok() {
                    let mut buf = [0u8; 1];
                    if f.read_exact(&mut buf).is_ok() && buf[0] != b'\n' {
                        let _ = writeln!(file);
                    }
                }
            }
        }
        let _ = writeln!(file, "{entry}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::{App, TuiOptions};
    use tempfile::TempDir;

    fn create_test_app_with_tmpdir(tmpdir: &TempDir) -> App {
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: tmpdir.path().to_path_buf(),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: tmpdir.path().join("skills"),
            memory_path: tmpdir.path().join("memory.md"),
            notes_path: tmpdir.path().join("notes.txt"),
            mcp_config_path: tmpdir.path().join("mcp.json"),
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
    fn test_init_creates_novel_workspace() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = init(&mut app);
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("Created novel workspace"));
        assert!(tmpdir.path().join("book.toml").exists());
        assert!(tmpdir.path().join("bible/premise.md").exists());
        assert!(tmpdir.path().join("memory/graph.json").exists());
    }

    #[test]
    fn test_init_is_noop_if_exists() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        // Create file first
        std::fs::write(tmpdir.path().join("book.toml"), "existing").unwrap();
        let result = init(&mut app);
        assert!(
            !result.is_error,
            "existing book.toml is an idempotent no-op, not an error"
        );
        assert!(result.message.is_some());
        assert!(result.message.unwrap().contains("already exists"));
    }

    #[test]
    fn ensure_deepseek_gitignored_creates_gitignore() {
        let tmpdir = TempDir::new().unwrap();
        // Simulate a git repo.
        std::fs::create_dir_all(tmpdir.path().join(".git")).unwrap();

        ensure_deepseek_gitignored(tmpdir.path());

        let content = std::fs::read_to_string(tmpdir.path().join(".gitignore")).unwrap();
        assert!(content.contains(".deepseek/"));
    }

    #[test]
    fn ensure_deepseek_gitignored_appends_to_existing() {
        let tmpdir = TempDir::new().unwrap();
        std::fs::create_dir_all(tmpdir.path().join(".git")).unwrap();
        std::fs::write(tmpdir.path().join(".gitignore"), "target/\n").unwrap();

        ensure_deepseek_gitignored(tmpdir.path());

        let content = std::fs::read_to_string(tmpdir.path().join(".gitignore")).unwrap();
        assert!(content.contains("target/"));
        assert!(content.contains(".deepseek/"));
    }

    #[test]
    fn ensure_deepseek_gitignored_idempotent() {
        let tmpdir = TempDir::new().unwrap();
        std::fs::create_dir_all(tmpdir.path().join(".git")).unwrap();

        ensure_deepseek_gitignored(tmpdir.path());
        ensure_deepseek_gitignored(tmpdir.path());

        let content = std::fs::read_to_string(tmpdir.path().join(".gitignore")).unwrap();
        assert_eq!(content.matches(".deepseek/").count(), 1);
    }

    #[test]
    fn ensure_deepseek_gitignored_skips_non_git_repo() {
        let tmpdir = TempDir::new().unwrap();
        // No .git directory — not a git repo.

        ensure_deepseek_gitignored(tmpdir.path());

        assert!(!tmpdir.path().join(".gitignore").exists());
    }

    #[test]
    fn ensure_deepseek_gitignored_handles_no_trailing_newline() {
        let tmpdir = TempDir::new().unwrap();
        std::fs::create_dir_all(tmpdir.path().join(".git")).unwrap();
        // Write a file that does NOT end with a newline.
        std::fs::write(tmpdir.path().join(".gitignore"), "target/").unwrap();

        ensure_deepseek_gitignored(tmpdir.path());

        let content = std::fs::read_to_string(tmpdir.path().join(".gitignore")).unwrap();
        // Must have both entries on separate lines.
        assert!(content.contains("target/"));
        assert!(content.contains(".deepseek/"));
        // The entries should be on different lines.
        let lines: Vec<&str> = content.lines().collect();
        assert!(lines.len() >= 2);
    }

    #[test]
    fn ensure_deepseek_gitignored_detects_variant_without_slash() {
        let tmpdir = TempDir::new().unwrap();
        std::fs::create_dir_all(tmpdir.path().join(".git")).unwrap();
        // Write .deepseek without trailing slash.
        std::fs::write(tmpdir.path().join(".gitignore"), ".deepseek\n").unwrap();

        ensure_deepseek_gitignored(tmpdir.path());

        let content = std::fs::read_to_string(tmpdir.path().join(".gitignore")).unwrap();
        // Should NOT add a duplicate entry.
        assert_eq!(content.matches(".deepseek").count(), 1);
    }
}
