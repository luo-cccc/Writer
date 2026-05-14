//! Welcome screen content for onboarding.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::palette;

pub fn lines() -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(
            "DeepSeek Novel Studio",
            Style::default()
                .fg(palette::DEEPSEEK_BLUE)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!("Version {}", env!("CARGO_PKG_VERSION")),
            Style::default().fg(palette::TEXT_MUTED),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "A local-first workspace for long-form fiction.",
            Style::default().fg(palette::TEXT_PRIMARY),
        )),
        Line::from(Span::styled(
            "You'll add an API key, trust the book directory, and then land in a novel workspace.",
            Style::default().fg(palette::TEXT_MUTED),
        )),
        Line::from(Span::styled(
            "Use it for story bibles, character cards, chapter briefs, drafts, revisions, and memory.",
            Style::default().fg(palette::TEXT_MUTED),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Press Enter to continue.",
            Style::default().fg(palette::TEXT_PRIMARY),
        )),
        Line::from(Span::styled(
            "Ctrl+C exits at any point.",
            Style::default().fg(palette::TEXT_MUTED),
        )),
    ]
}
