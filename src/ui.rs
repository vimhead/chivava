use std::time::Duration;

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Style, Stylize},
    text::{Line, Span},
    widgets::Paragraph,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    app::{Application, HELP_PAGES, Screen, SettingAction},
    chart_layout::{ChartLayout, ChartMeasure},
    chart_plot::{CellPixels, ChartImage},
    engine::EditMode,
    result_chart::{ChartPreview, ResultChart},
    selection::SelectionMode,
    syntax::SyntaxRole,
    theme::Theme,
    typing_view::{PreviewWindowInput, TypingPreview},
};

pub struct View<'a> {
    pub application: &'a Application,
    pub now: Duration,
}

#[derive(Clone, Copy)]
pub struct ViewOutput {
    pub width: u16,
    pub cell_pixels: Option<CellPixels>,
}

impl View<'_> {
    fn resolve_visible_theme(&self) -> &Theme {
        if self.application.screen == Screen::Select
            && let Some(selection) = &self.application.selection
            && selection.action == SettingAction::Theme
            && let Some(theme) = self.application.themes.get(selection.prompt.focused)
        {
            return theme;
        }
        self.application.current_theme()
    }

    pub fn measure_height(&self, terminal_width: u16) -> u16 {
        self.measure_output_height(ViewOutput {
            width: terminal_width,
            cell_pixels: None,
        })
    }

    pub fn measure_output_height(&self, output: ViewOutput) -> u16 {
        let body_height = match self.application.screen {
            Screen::Typing => 1 + TypingPreview::line_limit(&self.application.session),
            Screen::Settings => 1 + self.application.build_setting_rows().len().min(6),
            Screen::Select => {
                1 + self
                    .application
                    .selection
                    .as_ref()
                    .map(|selection| selection.prompt.choices.len().min(6))
                    .unwrap_or(1)
            }
            Screen::Results => {
                (self.measure_result_height() - ResultChart::HEIGHT
                    + self.measure_chart_height(output)) as usize
            }
            Screen::Help => 5,
        };
        (body_height
            + self.build_footer_lines(output).len()
            + usize::from(self.application.notice.is_some())) as u16
    }

    pub fn measure_result_height(&self) -> u16 {
        1 + u16::from(self.application.passage.format_result_source().is_some())
            + ResultChart::HEIGHT
    }

    pub fn measure_visible_result_height(&self, preview: ChartPreview) -> u16 {
        if preview.area.width < 26 || preview.area.height < 3 {
            return 0;
        }
        self.split_sections(preview)[0].height.min(
            self.measure_result_height() - ResultChart::HEIGHT
                + self.measure_chart_height(ViewOutput {
                    width: preview.area.width,
                    cell_pixels: preview.cell_pixels,
                }),
        )
    }

    fn split_sections(&self, preview: ChartPreview) -> [Rect; 3] {
        let area = preview.area;
        let footer_height = self.build_visible_footer_lines(preview).len() as u16;
        let content = Rect {
            x: area.x + 1,
            width: measure_content_width(area.width),
            ..area
        };
        Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(u16::from(self.application.notice.is_some())),
            Constraint::Length(footer_height),
        ])
        .areas(content)
    }

    #[cfg(test)]
    pub fn render(&self, frame: &mut Frame) {
        self.render_preview(frame, None);
    }

    pub fn render_preview(
        &self,
        frame: &mut Frame,
        cell_pixels: Option<CellPixels>,
    ) -> Option<ChartImage> {
        let area = frame.area();
        if area.width < 26 || area.height < 3 {
            frame.render_widget(Paragraph::new("Resize to 26 x 4\nC-c quit"), area);
            return None;
        }
        let footer = self.build_visible_footer_lines(ChartPreview { area, cell_pixels });
        let sections = self.split_sections(ChartPreview { area, cell_pixels });
        let mut image = None;
        match self.application.screen {
            Screen::Typing => self.render_typing(frame, sections[0]),
            Screen::Settings => self.render_settings(frame, sections[0]),
            Screen::Select => self.render_selection(frame, sections[0]),
            Screen::Results => {
                image = self.render_results(
                    frame,
                    ChartPreview {
                        area: sections[0],
                        cell_pixels,
                    },
                )
            }
            Screen::Help => self.render_help(frame, sections[0]),
        }
        let theme = self.resolve_visible_theme();
        if let Some(notice) = &self.application.notice {
            let color = if notice.starts_with("Error:") || notice.starts_with("Select at least") {
                theme.error
            } else {
                theme.muted
            };
            frame.render_widget(
                Paragraph::new(notice.as_str()).style(Style::default().fg(color)),
                sections[1],
            );
        }
        let hidden_footer_rows = footer.len().saturating_sub(usize::from(sections[2].height));
        frame.render_widget(
            Paragraph::new(
                footer
                    .into_iter()
                    .skip(hidden_footer_rows)
                    .map(|line| line.style(Style::default().fg(theme.muted)))
                    .collect::<Vec<_>>(),
            ),
            sections[2],
        );
        image
    }

    fn render_typing(&self, frame: &mut Frame, area: Rect) {
        let session = &self.application.session;
        let theme = self.application.current_theme();
        let timer = session.duration.map_or_else(
            || "untimed".into(),
            |limit| {
                let remaining = limit
                    .saturating_sub(session.elapsed(self.now))
                    .as_secs_f64()
                    .ceil() as u64;
                format!("{remaining}s")
            },
        );
        let status = if session.mode == EditMode::Normal {
            format!("{timer} · normal {}", session.pending)
        } else if session.started_at.is_none() {
            format!("{timer} · start typing")
        } else {
            timer
        };
        let sections = Layout::vertical([
            Constraint::Length(u16::from(area.height > 1)),
            Constraint::Min(0),
        ])
        .split(area);
        frame.render_widget(
            Paragraph::new(Line::styled(status, Style::default().fg(theme.muted))),
            sections[0],
        );
        let preview = TypingPreview {
            session,
            theme,
            width: area.width,
        }
        .build_window(PreviewWindowInput {
            highlights: &self.application.code_highlights,
            available_rows: sections[1].height,
        });
        frame.render_widget(Paragraph::new(preview.lines), sections[1]);
        if sections[1].height > 0 {
            frame.set_cursor_position((
                sections[1].x + preview.cursor_column.min(area.width.saturating_sub(1)),
                sections[1].y + preview.cursor_row,
            ));
        }
    }

    fn render_settings(&self, frame: &mut Frame, area: Rect) {
        let rows = self.application.build_setting_rows();
        let focused = self.application.selected_setting;
        let heading_height = u16::from(area.height > 1);
        let budget = area.height.saturating_sub(heading_height) as usize;
        let offset = focused.saturating_sub(budget.saturating_sub(1));
        let theme = self.application.current_theme();
        let heading = "Settings".to_owned();
        let title = if rows.len() > budget {
            format!("{heading} · {}/{}", focused + 1, rows.len())
        } else {
            heading
        };
        let name_width = if area.width < 40 { 12 } else { 15 };
        let mut lines = if heading_height > 0 {
            vec![Line::styled(title, Style::default().fg(theme.text).bold())]
        } else {
            vec![]
        };
        for (index, row) in rows.iter().enumerate().skip(offset).take(budget) {
            let is_focused = index == focused;
            let name = truncate_text(&row.name, name_width);
            let padding = " ".repeat(name_width.saturating_sub(name.width()));
            let style = Style::default().fg(if is_focused { theme.accent } else { theme.text });
            lines.push(Line::from(vec![
                Span::styled(if is_focused { "▸ " } else { "  " }, style),
                Span::styled(format!("{name}{padding} "), style),
                Span::styled(row.value.clone(), Style::default().fg(theme.muted)),
            ]));
        }
        frame.render_widget(Paragraph::new(lines), area);
    }

    fn render_selection(&self, frame: &mut Frame, area: Rect) {
        let Some(selection) = &self.application.selection else {
            return;
        };
        let prompt = &selection.prompt;
        let theme = self.resolve_visible_theme();
        let is_theme_preview = selection.action == SettingAction::Theme;
        let preview_height = if is_theme_preview {
            area.height.saturating_sub(3).min(2)
        } else {
            0
        };
        let list_area = Rect {
            height: area.height - preview_height,
            ..area
        };
        let preview_area = Rect {
            y: list_area.bottom(),
            height: preview_height,
            ..area
        };
        let heading_height = u16::from(list_area.height > 1);
        let budget = list_area.height.saturating_sub(heading_height) as usize;
        let offset = prompt.focused.saturating_sub(budget.saturating_sub(1));
        let heading = if is_theme_preview {
            "Theme preview"
        } else {
            prompt.title.as_str()
        };
        let title = if prompt.choices.len() > budget {
            format!(
                "{heading} · {}/{}",
                prompt.focused + 1,
                prompt.choices.len()
            )
        } else {
            heading.to_owned()
        };
        let mut lines = if heading_height > 0 {
            vec![Line::styled(title, Style::default().fg(theme.text).bold())]
        } else {
            vec![]
        };
        for (index, choice) in prompt.choices.iter().enumerate().skip(offset).take(budget) {
            let is_focused = index == prompt.focused;
            let color = if is_focused {
                theme.accent
            } else if choice.is_selected {
                theme.text
            } else {
                theme.muted
            };
            lines.push(Line::styled(
                format!(
                    "{} {} {}",
                    if is_focused { "▸" } else { " " },
                    if choice.is_selected { "●" } else { "○" },
                    choice.label
                ),
                Style::default().fg(color),
            ));
        }
        frame.render_widget(Paragraph::new(lines), list_area);
        if preview_height > 0 {
            frame.render_widget(
                Paragraph::new(self.build_theme_preview_lines()),
                preview_area,
            );
        }
    }

    fn build_theme_preview_lines(&self) -> Vec<Line<'static>> {
        let theme = self.resolve_visible_theme();
        let typing = Line::from(vec![
            Span::styled("correct ", Style::default().fg(theme.success)),
            Span::styled("error", Style::default().fg(theme.error).underlined()),
            Span::styled(" pending", Style::default().fg(theme.dim)),
        ]);
        let syntax = Line::from(
            [
                ("let", SyntaxRole::Keyword),
                (" ", SyntaxRole::Plain),
                ("n", SyntaxRole::Variable),
                (" ", SyntaxRole::Plain),
                ("=", SyntaxRole::Operator),
                (" ", SyntaxRole::Plain),
                ("\"hi\"", SyntaxRole::String),
                (".", SyntaxRole::Punctuation),
                ("len", SyntaxRole::Function),
                ("()", SyntaxRole::Punctuation),
                (" ", SyntaxRole::Plain),
                ("+", SyntaxRole::Operator),
                (" ", SyntaxRole::Plain),
                ("1", SyntaxRole::Number),
                (";", SyntaxRole::Punctuation),
            ]
            .into_iter()
            .map(|(text, role)| {
                Span::styled(text, Style::default().fg(theme.resolve_syntax_color(role)))
            })
            .collect::<Vec<_>>(),
        );
        vec![typing, syntax]
    }

    fn render_results(&self, frame: &mut Frame, preview: ChartPreview) -> Option<ChartImage> {
        let area = preview.area;
        let session = &self.application.session;
        let theme = self.application.current_theme();
        let summary = format!(
            "{:.1} wpm · {:.1}% · {:.0}s",
            session.calculate_wpm(self.now),
            session.calculate_accuracy(),
            session.elapsed(self.now).as_secs_f64()
        );
        let source = self.application.passage.format_result_source();
        let chart_height = self.measure_chart_height(ViewOutput {
            width: frame.area().width,
            cell_pixels: preview.cell_pixels,
        });
        let summary_area = Rect {
            height: area.height.min(1),
            ..area
        };
        let source_area = Rect {
            y: summary_area.bottom(),
            height: u16::from(source.is_some())
                .min(area.height.saturating_sub(summary_area.height)),
            ..area
        };
        let chart_area = Rect {
            y: source_area.bottom(),
            height: chart_height.min(area.bottom().saturating_sub(source_area.bottom())),
            ..area
        };
        frame.render_widget(
            Paragraph::new(Line::styled(
                summary,
                Style::default().fg(theme.text).bold(),
            )),
            summary_area,
        );
        if let Some(source) = source {
            let source_area = Rect {
                width: frame.area().width.saturating_sub(2),
                ..source_area
            };
            frame.render_widget(
                Paragraph::new(Line::styled(
                    abbreviate_source(&source, usize::from(source_area.width)),
                    Style::default().fg(theme.muted),
                )),
                source_area,
            );
        }
        ResultChart {
            samples: &session.samples,
            errors: &session.errors,
            final_wpm: session.calculate_wpm(self.now),
            final_raw_wpm: session.calculate_raw_wpm(self.now),
            elapsed: session.elapsed(self.now),
            theme,
            font: &self.application.chart_font,
            is_legend_visible: self.application.is_chart_legend_visible,
        }
        .render_image(ChartPreview {
            area: chart_area,
            ..preview
        })
    }

    fn measure_chart_height(&self, output: ViewOutput) -> u16 {
        output.cell_pixels.map_or(0, |cell| {
            ChartLayout::measure_rows(ChartMeasure {
                width: measure_content_width(output.width),
                cell,
                font: &self.application.chart_font,
                is_legend_visible: self.application.is_chart_legend_visible,
            })
        })
    }

    fn render_help(&self, frame: &mut Frame, area: Rect) {
        let theme = self.application.current_theme();
        let page = self.application.help_offset.min(HELP_PAGES.len() - 1);
        let mut lines = vec![Line::styled(
            format!("Keys · {}/{}", page + 1, HELP_PAGES.len()),
            Style::default().fg(theme.text).bold(),
        )];
        lines.extend(
            HELP_PAGES[page]
                .iter()
                .map(|line| Line::styled(*line, Style::default().fg(theme.muted))),
        );
        frame.render_widget(Paragraph::new(lines), area);
    }

    fn build_visible_footer_lines(&self, preview: ChartPreview) -> Vec<Line<'static>> {
        let footer = self.build_footer_lines(ViewOutput {
            width: preview.area.width,
            cell_pixels: preview.cell_pixels,
        });
        if preview.area.height <= 4
            && footer.len() > 2
            && self.application.screen == Screen::Select
            && self
                .application
                .selection
                .as_ref()
                .is_some_and(|selection| selection.prompt.mode == SelectionMode::Multiple)
        {
            return vec![
                Line::from("j/k move · Space toggle"),
                Line::from("Enter apply · Esc cancel"),
            ];
        }
        footer
    }

    fn build_footer_lines(&self, output: ViewOutput) -> Vec<Line<'static>> {
        let width = measure_content_width(output.width) as usize;
        let actions: &[&str] = match self.application.screen {
            Screen::Typing if self.application.session.mode == EditMode::Insert => {
                &["F1 keys", "F2 settings", "C-c quit"]
            }
            Screen::Typing => &["i insert", "F1 keys", "F2 settings", "C-c quit"],
            Screen::Settings => &["j/k move", "Enter select", "Esc back"],
            Screen::Select
                if self
                    .application
                    .selection
                    .as_ref()
                    .is_some_and(|selection| selection.prompt.mode == SelectionMode::Multiple) =>
            {
                &[
                    "j/k move",
                    "Space toggle",
                    "a all",
                    "Enter apply",
                    "Esc cancel",
                ]
            }
            Screen::Select => &["j/k move", "Enter apply", "Esc cancel"],
            Screen::Results => &[
                "Enter new",
                "F5 restart",
                "F2 settings",
                "Tab legend",
                "C-c quit",
            ],
            Screen::Help if self.application.can_use_practice_shortcuts() => {
                &["j/k page", "F2 settings", "Esc back"]
            }
            Screen::Help => &["j/k page", "Esc back"],
        };
        let mut lines = Vec::new();
        let mut current = String::new();
        for action in actions {
            if *action == "Tab legend" && output.cell_pixels.is_none() {
                continue;
            }
            if !current.is_empty() && current.width() + 3 + action.width() > width {
                lines.push(Line::from(std::mem::take(&mut current)));
            }
            if !current.is_empty() {
                current.push_str(" · ");
            }
            current.push_str(action);
        }
        lines.push(Line::from(current));
        if self.application.is_results_cooling_down(self.now) {
            lines.fill(Line::default());
            lines[0] = Line::from("Wait 1s · C-c quit");
        }
        lines
    }
}

fn measure_content_width(terminal_width: u16) -> u16 {
    terminal_width.saturating_sub(2).min(72)
}

fn truncate_text(text: &str, width: usize) -> String {
    let mut used = 0;
    text.chars()
        .take_while(|character| {
            used += character.width().unwrap_or(0);
            used <= width
        })
        .collect()
}

fn abbreviate_source(source: &str, width: usize) -> String {
    if source.width() <= width {
        return source.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let prefix = truncate_text(source, width - 1);
    let complete_words = if source[prefix.len()..].starts_with(char::is_whitespace)
        || prefix.ends_with(char::is_whitespace)
    {
        prefix.trim_end()
    } else {
        prefix
            .rfind(char::is_whitespace)
            .map(|boundary| &prefix[..boundary])
            .unwrap_or("")
    };
    format!("{}…", complete_words.trim_end_matches([' ', '·', ';', ',']))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::{ActiveSelection, ApplicationInput, SettingAction},
        content::Corpus,
        engine::{Sample, TypingSession},
        selection::{Choice, SelectionPrompt},
        settings::{Configuration, TextType},
        theme::Theme,
    };
    use ratatui::{Terminal, TerminalOptions, Viewport, backend::TestBackend};

    fn create_application(directory: &std::path::Path) -> Application {
        Application::create(ApplicationInput {
            configuration: Configuration::create_initial(),
            directory: directory.into(),
            corpus: Corpus::load(directory).unwrap(),
            themes: Theme::load_bundled().unwrap(),
        })
        .unwrap()
    }

    fn collect_rows(terminal: &Terminal<TestBackend>) -> Vec<String> {
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|row| {
                (0..buffer.area.width)
                    .map(|column| buffer[(column, row)].symbol())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn theme_picker_previews_every_palette_without_changing_the_applied_theme() {
        use ratatui::style::{Color, Modifier};
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        let mut custom = application.themes[0].clone();
        custom.name = "Custom preview".into();
        custom.selector = "./custom.json".into();
        custom.accent = Color::Rgb(12, 34, 56);
        application.themes.push(custom);
        let applied = application.configuration.settings.theme.clone();
        application.screen = Screen::Select;
        application.selection = Some(ActiveSelection {
            return_screen: Screen::Settings,
            action: SettingAction::Theme,
            prompt: SelectionPrompt {
                title: "Theme".into(),
                mode: SelectionMode::Single,
                focused: 0,
                choices: application
                    .themes
                    .iter()
                    .map(|theme| Choice {
                        label: theme.format_label(),
                        is_selected: theme.selector == applied,
                    })
                    .collect(),
            },
        });
        for width in [26, 36, 72] {
            for focused in 0..application.themes.len() {
                application.selection.as_mut().unwrap().prompt.focused = focused;
                let view = View {
                    application: &application,
                    now: Duration::ZERO,
                };
                let height = view.measure_height(width);
                assert!(height <= 10);
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal.draw(|frame| view.render(frame)).unwrap();
                let rows = collect_rows(&terminal);
                let theme = &application.themes[focused];
                let buffer = terminal.backend().buffer();
                assert!(rows[0].contains("Theme preview"));
                assert_eq!(buffer[(1, 0)].fg, theme.text);
                let focused_row = rows.iter().position(|row| row.contains('▸')).unwrap() as u16;
                assert_eq!(buffer[(1, focused_row)].fg, theme.accent);
                let typing_row = rows
                    .iter()
                    .position(|row| row.contains("correct error pending"))
                    .unwrap() as u16;
                assert_eq!(buffer[(1, typing_row)].fg, theme.success);
                assert_eq!(buffer[(9, typing_row)].fg, theme.error);
                assert!(
                    buffer[(9, typing_row)]
                        .modifier
                        .contains(Modifier::UNDERLINED)
                );
                assert_eq!(buffer[(15, typing_row)].fg, theme.dim);
                let code_row = rows
                    .iter()
                    .position(|row| row.contains("let n = \"hi\".len() + 1;"))
                    .unwrap() as u16;
                for (column, role) in [
                    (1, SyntaxRole::Keyword),
                    (5, SyntaxRole::Variable),
                    (7, SyntaxRole::Operator),
                    (9, SyntaxRole::String),
                    (14, SyntaxRole::Function),
                    (22, SyntaxRole::Number),
                ] {
                    assert_eq!(
                        buffer[(column, code_row)].fg,
                        theme.resolve_syntax_color(role)
                    );
                }
                assert_eq!(buffer[(1, height - 1)].fg, theme.muted);
                assert!(rows.last().unwrap().contains("Esc cancel"));
                assert_eq!(application.current_theme().selector, applied);
                assert_eq!(application.configuration.settings.theme, applied);
            }
        }
        for height in 4..=9 {
            let mut terminal = Terminal::new(TestBackend::new(26, height)).unwrap();
            terminal
                .draw(|frame| {
                    View {
                        application: &application,
                        now: Duration::ZERO,
                    }
                    .render(frame)
                })
                .unwrap();
            let rows = collect_rows(&terminal);
            assert!(rows.iter().any(|row| row.contains("▸")));
            assert!(rows.last().unwrap().contains("Esc cancel"));
        }
        application.selection.as_mut().unwrap().action = SettingAction::Timer;
        let view = View {
            application: &application,
            now: Duration::ZERO,
        };
        assert_eq!(view.resolve_visible_theme().selector, applied);
        let mut terminal = Terminal::new(TestBackend::new(72, view.measure_height(72))).unwrap();
        terminal.draw(|frame| view.render(frame)).unwrap();
        assert!(
            !collect_rows(&terminal)
                .join("\n")
                .contains("correct error pending")
        );
    }

    #[test]
    fn preview_confirmation_and_cancellation_preserve_progress_pause_and_saved_settings() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        for should_apply in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let mut application = create_application(directory.path());
            application.session =
                TypingSession::create("alpha beta", Some(Duration::from_secs(30)));
            application.configuration.save(directory.path()).unwrap();
            let config_path = directory.path().join("config.json");
            let saved = std::fs::read(&config_path).unwrap();
            for (code, seconds) in [
                (KeyCode::Char('a'), 10),
                (KeyCode::Char('x'), 11),
                (KeyCode::F(2), 12),
            ] {
                application
                    .handle_key(
                        KeyEvent::new(code, KeyModifiers::NONE),
                        Duration::from_secs(seconds),
                    )
                    .unwrap();
            }
            let samples = serde_json::to_string(&application.session.samples).unwrap();
            let errors = serde_json::to_string(&application.session.errors).unwrap();
            let attempts = application.session.attempts;
            let cursor = application.session.cursor;
            application.selected_setting = application
                .build_setting_rows()
                .iter()
                .position(|row| row.action == SettingAction::Theme)
                .unwrap();
            application
                .handle_key(
                    KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                    Duration::from_secs(13),
                )
                .unwrap();
            for code in [
                KeyCode::PageDown,
                KeyCode::PageUp,
                KeyCode::End,
                KeyCode::Down,
                KeyCode::Up,
                KeyCode::Home,
                KeyCode::Down,
            ] {
                application
                    .handle_key(
                        KeyEvent::new(code, KeyModifiers::NONE),
                        Duration::from_secs(100),
                    )
                    .unwrap();
                let view = View {
                    application: &application,
                    now: Duration::from_secs(100),
                };
                assert_eq!(
                    view.resolve_visible_theme().selector,
                    application.themes[application.selection.as_ref().unwrap().prompt.focused]
                        .selector
                );
                assert_eq!(application.configuration.settings.theme, "catppuccin-mocha");
            }
            application.tick(Duration::from_secs(101));
            assert_eq!(std::fs::read(&config_path).unwrap(), saved);
            assert_eq!(
                serde_json::to_string(&application.session.samples).unwrap(),
                samples
            );
            assert_eq!(
                serde_json::to_string(&application.session.errors).unwrap(),
                errors
            );
            let choice = if should_apply {
                KeyCode::Enter
            } else {
                KeyCode::Esc
            };
            application
                .handle_key(
                    KeyEvent::new(choice, KeyModifiers::NONE),
                    Duration::from_secs(111),
                )
                .unwrap();
            assert_eq!(application.screen, Screen::Settings);
            let expected = if should_apply {
                "catppuccin-latte"
            } else {
                "catppuccin-mocha"
            };
            assert_eq!(
                View {
                    application: &application,
                    now: Duration::from_secs(111)
                }
                .resolve_visible_theme()
                .selector,
                expected
            );
            assert_eq!(std::fs::read(&config_path).unwrap(), saved);
            application
                .handle_key(
                    KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                    Duration::from_secs(112),
                )
                .unwrap();
            assert_eq!(application.screen, Screen::Typing);
            assert_eq!(
                application.session.elapsed(Duration::from_secs(112)),
                Duration::from_secs(2)
            );
            assert_eq!(application.session.typed, vec!['a', 'x']);
            assert_eq!(application.session.cursor, cursor);
            assert_eq!(application.session.attempts, attempts);
            assert_eq!(
                serde_json::to_string(&application.session.samples).unwrap(),
                samples
            );
            assert_eq!(
                serde_json::to_string(&application.session.errors).unwrap(),
                errors
            );
            assert_eq!(
                Configuration::load(directory.path())
                    .unwrap()
                    .settings
                    .theme,
                expected
            );
        }
    }

    #[test]
    fn quitting_a_theme_preview_does_not_commit_the_highlighted_candidate() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        application
            .handle_key(
                KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE),
                Duration::ZERO,
            )
            .unwrap();
        application.selected_setting = 1;
        for code in [KeyCode::Enter, KeyCode::Down] {
            application
                .handle_key(KeyEvent::new(code, KeyModifiers::NONE), Duration::ZERO)
                .unwrap();
        }
        assert_ne!(
            View {
                application: &application,
                now: Duration::ZERO
            }
            .resolve_visible_theme()
            .selector,
            application.current_theme().selector
        );
        application
            .handle_key(
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                Duration::ZERO,
            )
            .unwrap();
        assert_eq!(
            Configuration::load(directory.path())
                .unwrap()
                .settings
                .theme,
            "catppuccin-mocha"
        );
    }

    #[test]
    fn results_cooldown_changes_the_footer_without_moving_results_or_resizing() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        application.session = TypingSession::create("a", None);
        application.session.insert_character('a', Duration::ZERO);
        application.tick(Duration::ZERO);
        for width in [26, 36, 72, 120] {
            let waiting = View {
                application: &application,
                now: Duration::from_millis(999),
            };
            let ready = View {
                application: &application,
                now: Duration::from_secs(1),
            };
            assert_eq!(waiting.measure_height(width), ready.measure_height(width));
            let mut terminal =
                Terminal::new(TestBackend::new(width, waiting.measure_height(width))).unwrap();
            terminal.draw(|frame| waiting.render(frame)).unwrap();
            let waiting_rows = collect_rows(&terminal);
            assert!(waiting_rows.join("\n").contains("Wait 1s · C-c quit"));
            assert!(!waiting_rows.join("\n").contains("Enter new"));
            terminal.draw(|frame| ready.render(frame)).unwrap();
            let ready_rows = collect_rows(&terminal);
            assert!(ready_rows.join("\n").contains("Enter new"));
            assert!(!ready_rows.join("\n").contains("Wait 1s"));
            let result_height = waiting.measure_visible_result_height(ChartPreview {
                area: Rect::new(0, 0, width, waiting.measure_height(width)),
                cell_pixels: None,
            }) as usize;
            assert_eq!(waiting_rows[..result_height], ready_rows[..result_height]);
        }
    }

    #[test]
    fn code_preview_adapts_to_short_terminals_without_hiding_the_input_line() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        application.session = TypingSession::create_code(
            "    one();\n  two();\n  three();\n  four();\n  five();\n  six();\n  seven();\n  eight();\n  nine();\n  hidden();",
            Some(Duration::from_secs(30)),
        );
        for width in [26, 36, 72, 120] {
            for height in [4, 6, 8, 10, 12, 20] {
                for has_notice in [false, true] {
                    application.notice = has_notice.then(|| "Sound unavailable".into());
                    let view = View {
                        application: &application,
                        now: Duration::ZERO,
                    };
                    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                    terminal.draw(|frame| view.render(frame)).unwrap();
                    let rows = collect_rows(&terminal);
                    assert!(
                        rows.iter().any(|row| row.contains("    one();")),
                        "{width}x{height}: {rows:?}"
                    );
                    assert!(rows.last().unwrap().contains("C-c quit"));
                    assert!(!rows.iter().any(|row| row.contains("hidden")));
                    assert_eq!(terminal.get_cursor_position().unwrap().x, 5);
                    if height >= view.measure_height(width) {
                        assert!(rows.iter().any(|row| row.contains("nine();")));
                    }
                }
            }
        }
    }

    #[test]
    fn scrolling_places_the_terminal_cursor_on_the_current_row_even_in_short_windows() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        let source = (0..20)
            .map(|index| format!("    row_{index:02}();"))
            .collect::<Vec<_>>()
            .join("\n");
        for session in [
            TypingSession::create(&source, None),
            TypingSession::create_code(&source, None),
        ] {
            application.session = session;
            let cursor = application
                .session
                .target
                .iter()
                .enumerate()
                .filter(|(_, character)| **character == '\n')
                .nth(8)
                .unwrap()
                .0
                + 1;
            application.session.cursor = cursor;
            application.session.typed = application.session.target[..=cursor].to_vec();
            for mode in [EditMode::Insert, EditMode::Normal] {
                application.session.mode = mode;
                for width in [26, 36, 72, 120] {
                    for height in [4, 6, 8, 12, 20] {
                        for has_notice in [false, true] {
                            application.notice = has_notice.then(|| "Sound unavailable".into());
                            let view = View {
                                application: &application,
                                now: Duration::ZERO,
                            };
                            let mut terminal =
                                Terminal::new(TestBackend::new(width, height)).unwrap();
                            terminal.draw(|frame| view.render(frame)).unwrap();
                            let position = terminal.get_cursor_position().unwrap();
                            assert!(position.x < width && position.y < height);
                            let rows = collect_rows(&terminal);
                            assert!(
                                rows[usize::from(position.y)].contains("row_09"),
                                "{width}x{height}: {rows:?}"
                            );
                            assert!(rows.last().unwrap().contains("C-c quit"));
                            if application.session.line_indentation.is_some() {
                                assert_eq!(position.x, 5);
                            } else {
                                assert_eq!(position.x, 1);
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn prose_shows_four_rows_without_chrome_or_live_statistics() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        application.session = TypingSession::create(
            "current line\nnext line\nthird line\nfourth line\nsecret fifth line",
            Some(Duration::from_secs(30)),
        );
        let view = View {
            application: &application,
            now: Duration::ZERO,
        };
        assert_eq!(view.measure_height(72), 6);
        let mut terminal = Terminal::new(TestBackend::new(72, 6)).unwrap();
        terminal.draw(|frame| view.render(frame)).unwrap();
        let rows = collect_rows(&terminal);
        assert!(rows[0].contains("30s"));
        assert!(rows[1].contains("current line"));
        assert!(rows[2].contains("next line"));
        assert!(rows[3].contains("third line"));
        assert!(rows[4].contains("fourth line"));
        assert!(rows[5].contains("F1 keys"));
        let text = rows.join("\n");
        for removed in ["secret", "INSERT", "accuracy", "wpm", "╭", "│", "chivava"] {
            assert!(!text.contains(removed), "{removed}");
        }
    }

    #[test]
    fn results_keep_attribution_and_omit_chart_space_and_shortcut_without_images() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        application.screen = Screen::Results;
        application.session.samples.push(Sample {
            seconds: 1.0,
            wpm: 42.0,
            raw_wpm: 48.0,
            burst_wpm: 60.0,
            errors: 1,
            counts: crate::statistics::CharacterCounts {
                correct_word: 0,
                raw: 0,
                insertions: 0,
                errors: 0,
            },
        });
        for text_type in [TextType::Books, TextType::Code] {
            application.configuration.settings.mode = match text_type {
                TextType::Books => crate::settings::PracticeMode::Prose,
                TextType::Code => crate::settings::PracticeMode::Code,
            };
            application.passage = application
                .corpus
                .choose_passage(&application.configuration.settings)
                .unwrap();
            let source = application.passage.format_result_source();
            for width in [26, 36, 48, 72, 120] {
                for cell_pixels in [
                    None,
                    Some(CellPixels {
                        width: 10,
                        height: 20,
                    }),
                ] {
                    let output = ViewOutput { width, cell_pixels };
                    let result_height = 1
                        + u16::from(source.is_some())
                        + if cell_pixels.is_some() {
                            ResultChart::HEIGHT
                        } else {
                            0
                        };
                    let view = View {
                        application: &application,
                        now: Duration::ZERO,
                    };
                    assert_eq!(
                        view.measure_output_height(output),
                        result_height + view.build_footer_lines(output).len() as u16
                    );
                    let mut terminal =
                        Terminal::new(TestBackend::new(width, view.measure_output_height(output)))
                            .unwrap();
                    terminal
                        .draw(|frame| {
                            view.render_preview(frame, cell_pixels);
                        })
                        .unwrap();
                    let rows = collect_rows(&terminal);
                    assert!(rows[0].contains("wpm"));
                    if let Some(source) = &source {
                        assert_eq!(
                            rows[1].trim(),
                            abbreviate_source(source, usize::from(width.saturating_sub(2))).trim()
                        );
                        assert_eq!(
                            terminal.backend().buffer()[(1, 1)].fg,
                            application.current_theme().muted
                        );
                    }
                    assert!(!rows.join("\n").contains("WPM"));
                    assert!(!rows.join("\n").contains('─'));
                    assert!(!rows.join("\n").contains("No pixels"));
                    if cell_pixels.is_some() {
                        let first_chart_row = 1 + usize::from(source.is_some());
                        assert!(
                            rows[first_chart_row..result_height as usize]
                                .iter()
                                .all(|row| row.trim().is_empty())
                        );
                    }
                    assert!(!rows.join("\n").contains("Corrected"));
                    assert!(!rows[0].contains("raw"));
                    assert_eq!(
                        rows.join("\n").contains("Tab legend"),
                        cell_pixels.is_some()
                    );
                    assert!(rows[result_height as usize].contains("Enter new"));
                    assert!(rows.join("\n").contains("F5 restart"));
                    assert!(rows.join("\n").contains("F2 settings"));
                    assert!(!rows.join("\n").contains("s settings"));
                    assert!(!rows.join("\n").contains("Enter retry"));
                    assert!(rows.last().unwrap().contains("C-c quit"));
                }
            }
        }
    }

    #[test]
    fn legend_toggles_without_changing_chart_pixels_and_stays_hidden_without_images() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        application.screen = Screen::Results;
        application.session.samples.push(Sample {
            seconds: 1.0,
            wpm: 42.0,
            raw_wpm: 48.0,
            burst_wpm: 60.0,
            errors: 1,
            counts: crate::statistics::CharacterCounts {
                correct_word: 0,
                raw: 0,
                insertions: 0,
                errors: 0,
            },
        });
        for width in [26, 36, 72, 120] {
            for cell_pixels in [
                None,
                Some(CellPixels {
                    width: 10,
                    height: 20,
                }),
            ] {
                let mut previous_image: Option<ChartImage> = None;
                let mut hidden_height = 0;
                for is_visible in [false, true, false] {
                    application.is_chart_legend_visible = is_visible;
                    let view = View {
                        application: &application,
                        now: Duration::from_secs(2),
                    };
                    let height = view.measure_output_height(ViewOutput { width, cell_pixels });
                    assert_eq!(
                        view.measure_visible_result_height(ChartPreview {
                            area: Rect::new(0, 0, width, height),
                            cell_pixels,
                        }),
                        height
                            - view
                                .build_footer_lines(ViewOutput { width, cell_pixels })
                                .len() as u16
                    );
                    if !is_visible {
                        hidden_height = height;
                    }
                    if cell_pixels.is_none() {
                        assert_eq!(height, hidden_height);
                    }
                    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                    let mut image = None;
                    terminal
                        .draw(|frame| {
                            image = view.render_preview(frame, cell_pixels);
                        })
                        .unwrap();
                    let text = collect_rows(&terminal).join("\n");
                    let should_show_legend = is_visible && cell_pixels.is_some();
                    for label in [
                        "WPM: correct words",
                        "Raw: with mistakes",
                        "Burst: recent, shaded",
                        "Uncorrected errors",
                        "All errors corrected",
                    ] {
                        assert!(
                            !text.contains(label),
                            "raster labels leaked into terminal cells: {width}: {text}"
                        );
                    }
                    if let Some(current) = &image {
                        assert_eq!(current.visible_height == current.height, should_show_legend);
                        if let Some(previous) = &previous_image {
                            assert_eq!(
                                (current.width, current.height),
                                (previous.width, previous.height)
                            );
                            assert_eq!(current.rgba, previous.rgba);
                        }
                    }
                    previous_image = image;
                    assert_eq!(text.contains("Tab legend"), cell_pixels.is_some());
                }
            }
        }
    }

    #[test]
    fn short_graphics_results_prioritize_statistics_and_quit_over_source_and_chart() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        application.screen = Screen::Results;
        for width in [26, 36, 72] {
            for height in [3, 4, 6, 8] {
                for is_visible in [false, true] {
                    application.is_chart_legend_visible = is_visible;
                    let view = View {
                        application: &application,
                        now: Duration::ZERO,
                    };
                    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                    let cell_pixels = Some(CellPixels {
                        width: 10,
                        height: 20,
                    });
                    terminal
                        .draw(|frame| {
                            view.render_preview(frame, cell_pixels);
                        })
                        .unwrap();
                    let rows = collect_rows(&terminal);
                    assert!(rows[0].contains("wpm"), "{width}x{height}: {rows:?}");
                    assert!(rows.last().unwrap().contains("C-c quit"));
                    let retained = view.measure_visible_result_height(ChartPreview {
                        area: Rect::new(0, 0, width, height),
                        cell_pixels,
                    });
                    assert!(retained > 0);
                    assert!(
                        rows[..retained as usize]
                            .iter()
                            .all(|row| !row.contains("C-c quit"))
                    );
                }
            }
        }
    }

    #[test]
    fn short_results_keep_statistics_and_controls_without_retaining_footer_rows() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        application.screen = Screen::Results;
        for width in [26, 36, 72, 120] {
            for height in [3, 4, 6, 8, 12] {
                let view = View {
                    application: &application,
                    now: Duration::ZERO,
                };
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal.draw(|frame| view.render(frame)).unwrap();
                let rows = collect_rows(&terminal);
                assert!(rows[0].contains("wpm"), "{width}x{height}: {rows:?}");
                assert!(rows.last().unwrap().contains("C-c quit"));
                let retained = view.measure_visible_result_height(ChartPreview {
                    area: Rect::new(0, 0, width, height),
                    cell_pixels: None,
                });
                assert!(retained > 0 && retained <= view.measure_result_height());
                assert!(
                    rows[..retained as usize]
                        .iter()
                        .all(|row| !row.contains("Enter new") && !row.contains("C-c quit"))
                );
            }
        }
    }

    #[test]
    fn source_abbreviation_keeps_whole_words_and_marks_omitted_details() {
        assert_eq!(
            abbreviate_source("Chapter V; Constance Garnett", 19),
            "Chapter V…"
        );
        assert_eq!(
            abbreviate_source("Chapter V; Constance Garnett", 21),
            "Chapter V; Constance…"
        );
        assert_eq!(abbreviate_source("Jane Eyre", 9), "Jane Eyre");
        assert_eq!(abbreviate_source("Jane Eyre", 5), "Jane…");
        assert_eq!(abbreviate_source("界界 author", 5), "界界…");
        assert_eq!(abbreviate_source("Longword", 4), "…");
        assert_eq!(abbreviate_source("source", 0), "");
    }

    #[test]
    fn results_use_available_width_for_attribution_instead_of_cutting_at_seventy_two_columns() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        application.screen = Screen::Results;
        application.passage.text_type = TextType::Books;
        application.passage.title = "Crime and Punishment".into();
        application.passage.author = "Fyodor Dostoevsky".into();
        application.passage.source = "PART III; CHAPTER V; Constance Garnett translation".into();
        let view = View {
            application: &application,
            now: Duration::ZERO,
        };
        for width in [72, 120] {
            let mut terminal =
                Terminal::new(TestBackend::new(width, view.measure_height(width))).unwrap();
            terminal.draw(|frame| view.render(frame)).unwrap();
            let source_row = collect_rows(&terminal)[1].trim().to_owned();
            if width == 72 {
                assert!(source_row.ends_with("CHAPTER V…"), "{source_row}");
                assert!(!source_row.contains("Constanc"));
            } else {
                assert!(
                    source_row.ends_with("Constance Garnett translation"),
                    "{source_row}"
                );
                assert!(!source_row.contains('…'));
            }
        }
    }

    #[test]
    fn every_help_page_fits_the_minimum_terminal_width_without_clipping() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        application.screen = Screen::Help;
        for (index, page) in HELP_PAGES.iter().enumerate() {
            application.help_offset = index;
            let view = View {
                application: &application,
                now: Duration::ZERO,
            };
            let mut terminal =
                Terminal::new(TestBackend::new(26, view.measure_height(26))).unwrap();
            terminal.draw(|frame| view.render(frame)).unwrap();
            let rows = collect_rows(&terminal);
            assert!(rows[0].contains(&format!("Keys · {}/{}", index + 1, HELP_PAGES.len())));
            for line in page {
                assert!(line.width() <= 24, "{line}");
                assert!(
                    rows.iter().any(|row| row.trim() == *line),
                    "{line}: {rows:?}"
                );
            }
            assert!(rows.join("\n").contains("F2 settings"));
        }
    }

    #[test]
    fn normal_mode_keeps_the_settings_shortcut_visible_at_every_width() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        application.session.enter_normal_mode();
        for width in [26, 36, 48, 72, 120] {
            let view = View {
                application: &application,
                now: Duration::ZERO,
            };
            let mut terminal =
                Terminal::new(TestBackend::new(width, view.measure_height(width))).unwrap();
            terminal.draw(|frame| view.render(frame)).unwrap();
            let rows = collect_rows(&terminal);
            for action in ["i insert", "F1 keys", "F2 settings", "C-c quit"] {
                assert!(
                    rows.iter().any(|row| row.contains(action)),
                    "{width}: {action}"
                );
            }
            assert!(rows.last().unwrap().contains("C-c quit"));
        }
    }

    #[test]
    fn all_views_remain_small_in_large_terminals_and_keep_actions_visible() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        application.selection = Some(ActiveSelection {
            return_screen: Screen::Settings,
            action: SettingAction::Theme,
            prompt: SelectionPrompt {
                title: "Theme".into(),
                mode: SelectionMode::Multiple,
                focused: 39,
                choices: (0..48)
                    .map(|index| Choice {
                        label: format!("Choice {index}"),
                        is_selected: index == 39,
                    })
                    .collect(),
            },
        });
        for width in [36, 48, 72, 120] {
            for screen in [
                Screen::Typing,
                Screen::Settings,
                Screen::Select,
                Screen::Results,
                Screen::Help,
            ] {
                application.screen = screen;
                let view = View {
                    application: &application,
                    now: Duration::ZERO,
                };
                let height = view.measure_height(width);
                assert!(height <= if screen == Screen::Results { 13 } else { 10 });
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal.draw(|frame| view.render(frame)).unwrap();
                let rows = collect_rows(&terminal);
                assert!(
                    rows.last().unwrap().contains(match screen {
                        Screen::Typing | Screen::Results => "C-c quit",
                        _ => "Esc",
                    }),
                    "{screen:?} at {width}: {rows:?}"
                );
                if screen == Screen::Select {
                    assert!(rows.iter().any(|row| row.contains("▸ ● Choice 39")));
                }
                if width > 74 && screen != Screen::Results {
                    assert!(
                        rows.iter()
                            .all(|row| row.chars().skip(74).all(|character| character == ' '))
                    );
                }
            }
        }
    }

    #[test]
    fn rendering_uses_the_inline_origin_and_never_paints_the_shell() {
        let directory = tempfile::tempdir().unwrap();
        let application = create_application(directory.path());
        let mut terminal = Terminal::with_options(
            TestBackend::new(72, 20),
            TerminalOptions {
                viewport: Viewport::Fixed(Rect::new(0, 8, 72, 4)),
            },
        )
        .unwrap();
        terminal
            .draw(|frame| {
                View {
                    application: &application,
                    now: Duration::ZERO,
                }
                .render(frame)
            })
            .unwrap();
        let rows = collect_rows(&terminal);
        assert!(rows[..8].iter().all(|row| row.trim().is_empty()));
        assert!(rows[8].contains("30s"));
        assert!(rows[11].contains("C-c quit"));
        assert!(rows[12..].iter().all(|row| row.trim().is_empty()));
    }

    #[test]
    fn compact_language_picker_keeps_the_focused_option_visible_even_with_a_notice() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        application.screen = Screen::Select;
        application.selection = Some(ActiveSelection {
            action: SettingAction::Languages,
            prompt: SelectionPrompt {
                title: "Languages".into(),
                mode: SelectionMode::Multiple,
                choices: vec![
                    Choice {
                        label: "Go".into(),
                        is_selected: false,
                    },
                    Choice {
                        label: "Rust".into(),
                        is_selected: true,
                    },
                ],
                focused: 1,
            },
            return_screen: Screen::Settings,
        });
        for notice in [None, Some("Select at least one option.")] {
            application.notice = notice.map(str::to_owned);
            for width in [26, 36, 72] {
                let mut terminal = Terminal::new(TestBackend::new(width, 4)).unwrap();
                terminal
                    .draw(|frame| {
                        View {
                            application: &application,
                            now: Duration::ZERO,
                        }
                        .render(frame)
                    })
                    .unwrap();
                let text = collect_rows(&terminal).join("\n");
                assert!(text.contains("▸ ● Rust"), "{width}x4: {text}");
                assert!(
                    text.contains("Enter apply") && text.contains("Esc cancel"),
                    "{width}x4: {text}"
                );
                if notice.is_none() {
                    assert!(
                        text.contains("Languages") && text.contains("Space toggle"),
                        "{width}x4: {text}"
                    );
                } else {
                    assert!(text.contains("Select at least one"), "{width}x4: {text}");
                }
            }
        }
    }

    #[test]
    fn direct_language_and_book_controls_stay_visible_on_compact_terminals() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        for (mode, label, count) in [
            (crate::settings::PracticeMode::Code, "Languages", 5),
            (crate::settings::PracticeMode::Prose, "Books", 7),
        ] {
            application.configuration.settings.mode = mode;
            application.screen = Screen::Settings;
            let settings = application.build_setting_rows();
            assert_eq!(settings.len(), count);
            application.selected_setting = settings.len() - 1;
            for width in [26, 36, 72] {
                for height in [4, 8, 20] {
                    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                    terminal
                        .draw(|frame| {
                            View {
                                application: &application,
                                now: Duration::ZERO,
                            }
                            .render(frame)
                        })
                        .unwrap();
                    let rows = collect_rows(&terminal);
                    let text = rows.join("\n");
                    assert!(
                        text.contains(&format!("▸ {label}")),
                        "{width}x{height}: {text}"
                    );
                    assert!(text.contains("Esc back"), "{width}x{height}: {text}");
                    assert!(!text.contains("Sources") && !text.contains("Samples"));
                }
            }
        }
    }

    #[test]
    fn settings_show_dependencies_last_without_listing_filter_choices() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        application.configuration.settings.mode = crate::settings::PracticeMode::Prose;
        application.screen = Screen::Settings;
        application.selected_setting = application.build_setting_rows().len() - 1;
        let view = View {
            application: &application,
            now: Duration::ZERO,
        };
        let mut terminal = Terminal::new(TestBackend::new(48, view.measure_height(48))).unwrap();
        terminal.draw(|frame| view.render(frame)).unwrap();
        let text = collect_rows(&terminal).join("\n");
        assert_eq!(view.measure_height(48), 8);
        let rows = collect_rows(&terminal);
        for (row, name) in
            rows[1..7]
                .iter()
                .zip(["Theme", "Sound", "Mode", "Capitals", "Punctuation", "Books"])
        {
            assert!(row.contains(name), "{row}");
        }
        assert!(text.contains("Esc back"));
        assert!(!text.contains("Esc quit"));
        assert!(!text.contains("Start"));
        assert!(!text.to_lowercase().contains("preset"));
        assert!(!text.contains("Jane Eyre"));
    }
}
