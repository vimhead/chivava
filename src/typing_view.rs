use std::ops::Range;

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthChar;

use crate::{
    engine::{EditMode, TypingSession},
    syntax::CodeHighlights,
    theme::Theme,
};

pub struct TypingPreview<'a> {
    pub session: &'a TypingSession,
    pub theme: &'a Theme,
    pub width: u16,
}

pub struct PreviewWindowInput<'a> {
    pub highlights: &'a CodeHighlights,
    pub available_rows: u16,
}

pub struct VisibleTyping {
    pub lines: Vec<Line<'static>>,
    pub cursor_column: u16,
    pub cursor_row: u16,
}

impl TypingPreview<'_> {
    pub fn line_limit(session: &TypingSession) -> usize {
        if session.line_indentation.is_some() {
            9
        } else {
            4
        }
    }

    fn indentation_width(&self, position: usize) -> usize {
        self.session
            .line_indentation
            .as_ref()
            .and_then(|lines| lines.get(&position))
            .copied()
            .unwrap_or(0)
            .min(self.width.saturating_sub(2) as usize)
    }

    fn character_width(&self, index: usize, column: usize) -> usize {
        if self.session.target[index] == '\t' {
            (4 - column % 4).min(self.width.max(1) as usize)
        } else {
            self.display_character(index).width().unwrap_or(1)
        }
    }

    #[cfg(test)]
    fn build_visible_lines(&self) -> VisibleTyping {
        self.build_highlighted_lines(&CodeHighlights::create_unhighlighted())
    }

    #[cfg(test)]
    pub fn build_highlighted_lines(&self, highlights: &CodeHighlights) -> VisibleTyping {
        self.build_window(PreviewWindowInput {
            highlights,
            available_rows: Self::line_limit(self.session) as u16,
        })
    }

    pub fn build_window(&self, input: PreviewWindowInput<'_>) -> VisibleTyping {
        let budget = usize::from(input.available_rows).min(Self::line_limit(self.session));
        if budget == 0 || self.width == 0 {
            return VisibleTyping {
                lines: vec![],
                cursor_column: 0,
                cursor_row: 0,
            };
        }
        let ranges = self.wrap_target();
        if ranges.is_empty() {
            return VisibleTyping {
                lines: vec![],
                cursor_column: 0,
                cursor_row: 0,
            };
        }
        let current = ranges
            .iter()
            .position(|range| range.contains(&self.session.cursor))
            .unwrap_or(ranges.len().saturating_sub(1));
        let is_code = self.session.line_indentation.is_some();
        let reserved_lookahead = usize::from(current + 1 < ranges.len());
        let history =
            (if is_code { 3 } else { 1 }).min(budget.saturating_sub(1 + reserved_lookahead));
        let start = current
            .saturating_sub(history)
            .min(ranges.len().saturating_sub(budget));
        let cursor_row = current - start;
        let end = (start + budget).min(ranges.len());
        let mut cursor_column = 0;
        let lines = ranges
            .into_iter()
            .skip(start)
            .take(end - start)
            .map(|range| {
                let indentation = self.indentation_width(range.start);
                let mut column = indentation as u16;
                let mut spans = vec![];
                if indentation > 0 {
                    spans.push(Span::styled(
                        " ".repeat(indentation),
                        Style::default().fg(self.theme.dim),
                    ));
                }
                spans.extend(range.map(|index| {
                    let expected = self.session.target[index];
                    let typed = self.session.typed.get(index).copied();
                    let is_wrong = typed.is_some_and(|character| character != expected);
                    let displayed = self.display_character(index);
                    let mut style = Style::default().fg(if is_wrong {
                        self.theme.error
                    } else if typed.is_some() {
                        self.session
                            .source_byte_offsets
                            .get(index)
                            .and_then(|offset| input.highlights.resolve_role_at_byte(*offset))
                            .map(|role| self.theme.resolve_syntax_color(role))
                            .unwrap_or(self.theme.success)
                    } else {
                        self.theme.dim
                    });
                    if is_wrong {
                        style = style.add_modifier(Modifier::UNDERLINED);
                    }
                    if index == self.session.cursor {
                        cursor_column = column;
                        if self.session.mode == EditMode::Normal {
                            style = style.bg(self.theme.selected_background);
                        }
                    }
                    let width = self.character_width(index, column as usize);
                    column += width as u16;
                    let content = if expected == '\t' {
                        format!("{displayed}{}", " ".repeat(width.saturating_sub(1)))
                    } else {
                        displayed.to_string()
                    };
                    Span::styled(content, style)
                }));
                Line::from(spans)
            })
            .collect();
        VisibleTyping {
            lines,
            cursor_column,
            cursor_row: cursor_row as u16,
        }
    }

    fn display_character(&self, index: usize) -> char {
        let expected = self.session.target[index];
        if expected == '\n' {
            return '↵';
        }
        if expected == '\t' {
            return '⇥';
        }
        match self.session.typed.get(index).copied() {
            Some(character) if character != expected => {
                if character.is_whitespace() {
                    '·'
                } else if character.width() == expected.width() {
                    character
                } else {
                    expected
                }
            }
            _ => expected,
        }
    }

    fn wrap_target(&self) -> Vec<Range<usize>> {
        if self.width == 0 {
            return vec![];
        }
        let mut ranges = vec![];
        let mut start = 0;
        let mut used = 0;
        for (index, &character) in self.session.target.iter().enumerate() {
            if index == start {
                used = self.indentation_width(index);
            }
            if !character.is_whitespace()
                && (index == 0 || self.session.target[index - 1].is_whitespace())
            {
                let word_width = self.measure_word_with_separator(index, used);
                if index > start
                    && word_width <= self.width as usize
                    && used + word_width > self.width as usize
                {
                    ranges.push(start..index);
                    start = index;
                    used = 0;
                }
            }
            let mut character_width = self.character_width(index, used);
            let separator_width = if character.is_whitespace() {
                0
            } else {
                self.session
                    .target
                    .get(index + 1)
                    .filter(|character| character.is_whitespace() && **character != '\n')
                    .map(|_| self.character_width(index + 1, used + character_width))
                    .unwrap_or(0)
            };
            if used + character_width + separator_width > self.width as usize && index > start {
                ranges.push(start..index);
                start = index;
                used = 0;
                character_width = self.character_width(index, used);
            }
            used += character_width;
            if character == '\n' {
                ranges.push(start..index + 1);
                start = index + 1;
                used = 0;
            }
            let limit = Self::line_limit(self.session);
            if ranges.len() >= limit && ranges[ranges.len() - limit].contains(&self.session.cursor)
            {
                return ranges;
            }
        }
        if start < self.session.target.len() {
            ranges.push(start..self.session.target.len());
        }
        ranges
    }

    fn measure_word_with_separator(&self, start: usize, column: usize) -> usize {
        let mut width = 0;
        for (offset, &character) in self.session.target[start..].iter().enumerate() {
            if character == '\n' {
                break;
            }
            width += self.character_width(start + offset, column + width);
            if character.is_whitespace() {
                break;
            }
        }
        width
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn collect_text(preview: &VisibleTyping) -> Vec<String> {
        preview
            .lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect()
    }

    fn get_current_line(preview: &VisibleTyping) -> &Line<'static> {
        &preview.lines[usize::from(preview.cursor_row)]
    }

    #[test]
    fn syntax_colors_follow_correct_input_but_never_override_errors_or_the_cursor() {
        use crate::syntax::{SyntaxHighlighter, SyntaxRole};
        let source = "fn demo() {\n    let value = \"café\";\n\treturn;\n}";
        let highlights = SyntaxHighlighter::create()
            .highlight_code("Rust", source)
            .unwrap();
        let themes = Theme::load_bundled().unwrap();
        let mut session = TypingSession::create_code(source, Some(Duration::from_secs(30)));
        let pending = TypingPreview {
            session: &session,
            theme: &themes[0],
            width: 72,
        }
        .build_highlighted_lines(&highlights);
        assert!(
            pending
                .lines
                .iter()
                .flat_map(|line| &line.spans)
                .all(|span| span.style.fg == Some(themes[0].dim))
        );
        for character in "fn demo() {\nlet value = \"caf".chars() {
            session.insert_character(character, Duration::ZERO);
        }
        for theme in &themes {
            let preview = TypingPreview {
                session: &session,
                theme,
                width: 72,
            }
            .build_highlighted_lines(&highlights);
            assert_eq!(get_current_line(&preview).spans[0].content, "    ");
            assert_eq!(
                get_current_line(&preview).spans[0].style.fg,
                Some(theme.dim)
            );
            assert_eq!(
                get_current_line(&preview).spans[1].style.fg,
                Some(theme.resolve_syntax_color(SyntaxRole::Keyword))
            );
            let string_span = get_current_line(&preview)
                .spans
                .iter()
                .find(|span| span.content == "f")
                .unwrap();
            assert_eq!(
                string_span.style.fg,
                Some(theme.resolve_syntax_color(SyntaxRole::String))
            );
            let pending_span = get_current_line(&preview)
                .spans
                .iter()
                .find(|span| span.content == "é")
                .unwrap();
            assert_eq!(pending_span.style.fg, Some(theme.dim));
        }
        session.insert_character('界', Duration::ZERO);
        session.enter_normal_mode();
        for width in [8, 26, 72] {
            let preview = TypingPreview {
                session: &session,
                theme: &themes[0],
                width,
            }
            .build_highlighted_lines(&highlights);
            let error = get_current_line(&preview)
                .spans
                .iter()
                .find(|span| span.style.fg == Some(themes[0].error))
                .unwrap();
            assert_eq!(error.content, "é");
            assert!(error.style.add_modifier.contains(Modifier::UNDERLINED));
            assert_eq!(error.style.bg, Some(themes[0].selected_background));
            assert!(preview.cursor_column < width);
            assert!(
                preview
                    .lines
                    .iter()
                    .all(|line| line.width() <= width as usize)
            );
        }
        session.handle_normal_character('a');
        session.delete_previous_character();
        session.insert_character('é', Duration::ZERO);
        let corrected = TypingPreview {
            session: &session,
            theme: &themes[0],
            width: 72,
        }
        .build_highlighted_lines(&highlights);
        let corrected_span = get_current_line(&corrected)
            .spans
            .iter()
            .find(|span| span.content == "é")
            .unwrap();
        assert_eq!(
            corrected_span.style.fg,
            Some(themes[0].resolve_syntax_color(SyntaxRole::String))
        );
        assert!(
            !corrected_span
                .style
                .add_modifier
                .contains(Modifier::UNDERLINED)
        );
        session.undo_edit();
        let undone = TypingPreview {
            session: &session,
            theme: &themes[0],
            width: 72,
        }
        .build_highlighted_lines(&highlights);
        assert_eq!(
            get_current_line(&undone)
                .spans
                .iter()
                .find(|span| span.content == "é")
                .unwrap()
                .style
                .fg,
            Some(themes[0].dim)
        );
        session.redo_edit();
        let fallback = TypingPreview {
            session: &session,
            theme: &themes[0],
            width: 72,
        }
        .build_highlighted_lines(&CodeHighlights::create_unhighlighted());
        assert_eq!(
            get_current_line(&fallback).spans[1].style.fg,
            Some(themes[0].success)
        );
    }

    #[test]
    fn short_code_starts_at_the_top_without_blank_padding() {
        let themes = Theme::load_bundled().unwrap();
        let mut session = TypingSession::create_code(
            "  first();\n\tsecond();\n\n    fourth();\n fifth();\n sixth();\n seventh();",
            Some(Duration::from_secs(30)),
        );
        let preview = TypingPreview {
            session: &session,
            theme: &themes[0],
            width: 72,
        }
        .build_visible_lines();
        assert_eq!(
            collect_text(&preview),
            [
                "  first();↵",
                "    second();↵",
                "↵",
                "    fourth();↵",
                " fifth();↵",
                " sixth();↵",
                " seventh();"
            ]
        );
        assert_eq!(preview.cursor_column, 2);
        assert_eq!(preview.cursor_row, 0);
        for character in "first();\n".chars() {
            session.insert_character(character, Duration::ZERO);
        }
        let preview = TypingPreview {
            session: &session,
            theme: &themes[0],
            width: 72,
        }
        .build_visible_lines();
        assert_eq!(collect_text(&preview)[0], "  first();↵");
        assert_eq!(collect_text(&preview)[1], "    second();↵");
        assert_eq!(preview.cursor_row, 1);
        assert_eq!(preview.lines.len(), 7);
        assert_eq!(preview.cursor_column, 4);
        session.delete_previous_character();
        let preview = TypingPreview {
            session: &session,
            theme: &themes[0],
            width: 72,
        }
        .build_visible_lines();
        assert_eq!(collect_text(&preview)[0], "  first();↵");
        assert_eq!(preview.cursor_row, 0);
        assert_eq!(preview.cursor_column, 10);
    }

    #[test]
    fn code_soft_wraps_do_not_create_or_require_extra_indentation() {
        let themes = Theme::load_bundled().unwrap();
        let mut session = TypingSession::create_code(
            "\t    let wide = 界界;\n  \n    call(\tvalue);\nend",
            Some(Duration::from_secs(30)),
        );
        for width in [4, 8, 26, 48, 72] {
            for cursor in 0..session.target.len() {
                session.cursor = cursor;
                let preview = TypingPreview {
                    session: &session,
                    theme: &themes[0],
                    width,
                }
                .build_visible_lines();
                assert!(!preview.lines.is_empty() && preview.lines.len() <= 9);
                assert!(usize::from(preview.cursor_row) < preview.lines.len());
                assert!(
                    preview
                        .lines
                        .iter()
                        .all(|line| line.width() <= width as usize),
                    "{width}: {:?}",
                    collect_text(&preview)
                );
                assert!(preview.cursor_column < width);
            }
        }
        let session = TypingSession::create_code("  abcdefghij", Some(Duration::from_secs(30)));
        let preview = TypingPreview {
            session: &session,
            theme: &themes[0],
            width: 6,
        }
        .build_visible_lines();
        assert_eq!(collect_text(&preview), ["  abcd", "efghij"]);
    }

    #[test]
    fn inline_code_tabs_still_require_input_and_keep_cursor_columns() {
        let themes = Theme::load_bundled().unwrap();
        let mut session = TypingSession::create_code("  x\t y", Some(Duration::from_secs(30)));
        session.insert_character('x', Duration::ZERO);
        let preview = TypingPreview {
            session: &session,
            theme: &themes[0],
            width: 72,
        }
        .build_visible_lines();
        assert_eq!(collect_text(&preview), ["  x⇥ y"]);
        assert_eq!(preview.cursor_column, 3);
        session.insert_character('\t', Duration::ZERO);
        let preview = TypingPreview {
            session: &session,
            theme: &themes[0],
            width: 72,
        }
        .build_visible_lines();
        assert_eq!(preview.cursor_column, 4);
        assert_eq!(session.attempts, 2);
    }

    #[test]
    fn only_normal_mode_paints_a_block_at_the_cursor() {
        let themes = Theme::load_bundled().unwrap();
        let mut session = TypingSession::create("hello world", Some(Duration::from_secs(30)));
        let preview = TypingPreview {
            session: &session,
            theme: &themes[0],
            width: 72,
        }
        .build_visible_lines();
        let style = get_current_line(&preview).spans[0].style;
        assert!(style.bg.is_none());
        assert!(!style.add_modifier.contains(Modifier::UNDERLINED));
        session.enter_normal_mode();
        let preview = TypingPreview {
            session: &session,
            theme: &themes[0],
            width: 72,
        }
        .build_visible_lines();
        assert_eq!(
            get_current_line(&preview).spans[0].style.bg,
            Some(themes[0].selected_background)
        );
        session.handle_normal_character('i');
        let preview = TypingPreview {
            session: &session,
            theme: &themes[0],
            width: 72,
        }
        .build_visible_lines();
        assert!(get_current_line(&preview).spans[0].style.bg.is_none());
    }

    #[test]
    fn prose_slides_both_ways_with_one_previous_row_and_two_upcoming_rows() {
        let themes = Theme::load_bundled().unwrap();
        let rows = [
            "one", "two", "three", "four", "five", "six", "seven", "eight",
        ];
        let mut session = TypingSession::create(&rows.join("\n"), None);
        for current in (0_usize..rows.len()).chain((0..rows.len()).rev()) {
            session.cursor = rows[..current].iter().map(|row| row.len() + 1).sum();
            let preview = TypingPreview {
                session: &session,
                theme: &themes[0],
                width: 72,
            }
            .build_visible_lines();
            let start = current.saturating_sub(1).min(rows.len() - 4);
            let expected: Vec<_> = (start..start + 4)
                .map(|index| {
                    format!(
                        "{}{}",
                        rows[index],
                        if index + 1 < rows.len() { "↵" } else { "" }
                    )
                })
                .collect();
            assert_eq!(collect_text(&preview), expected);
            assert_eq!(usize::from(preview.cursor_row), current - start);
            assert_eq!(preview.cursor_column, 0);
        }
    }

    #[test]
    fn code_shows_one_through_nine_then_two_through_ten_on_the_fifth_row() {
        let themes = Theme::load_bundled().unwrap();
        let rows: Vec<_> = (1..=12)
            .map(|index| format!("    line_{index:02}();"))
            .collect();
        let mut session = TypingSession::create_code(&rows.join("\n"), None);
        for current in (0_usize..rows.len()).chain((0..rows.len()).rev()) {
            session.cursor = rows[..current]
                .iter()
                .map(|row| row.trim_start().len() + 1)
                .sum();
            let preview = TypingPreview {
                session: &session,
                theme: &themes[0],
                width: 72,
            }
            .build_visible_lines();
            let start = match current {
                0..=3 => 0,
                4 => 1,
                5 => 2,
                _ => 3,
            };
            let expected: Vec<_> = (start..start + 9)
                .map(|index| {
                    format!(
                        "{}{}",
                        rows[index],
                        if index + 1 < rows.len() { "↵" } else { "" }
                    )
                })
                .collect();
            assert_eq!(collect_text(&preview), expected);
            assert_eq!(preview.lines.len(), 9);
            assert_eq!(usize::from(preview.cursor_row), current - start);
            assert_eq!(preview.cursor_column, 4);
        }
    }

    #[test]
    fn windows_count_visual_rows_and_keep_the_cursor_visible_at_every_height() {
        let themes = Theme::load_bundled().unwrap();
        let highlights = CodeHighlights::create_unhighlighted();
        let source =
            "    café 界界 wide words\tcall();\n\n    more words and long_lines();\n".repeat(3);
        for mut session in [
            TypingSession::create(&source, None),
            TypingSession::create_code(&source, None),
        ] {
            session.typed = session.target.clone();
            session.mode = EditMode::Normal;
            for width in [4, 8, 26, 72] {
                for available_rows in 1..=12 {
                    for cursor in 0..session.target.len() {
                        session.cursor = cursor;
                        let renderer = TypingPreview {
                            session: &session,
                            theme: &themes[0],
                            width,
                        };
                        let ranges = renderer.wrap_target();
                        let current = ranges
                            .iter()
                            .position(|range| range.contains(&cursor))
                            .unwrap();
                        let preview = renderer.build_window(PreviewWindowInput {
                            highlights: &highlights,
                            available_rows,
                        });
                        let budget =
                            usize::from(available_rows).min(TypingPreview::line_limit(&session));
                        assert!(!preview.lines.is_empty() && preview.lines.len() <= budget);
                        assert!(usize::from(preview.cursor_row) < preview.lines.len());
                        assert!(preview.cursor_column < width);
                        assert!(
                            preview
                                .lines
                                .iter()
                                .all(|line| line.width() <= usize::from(width))
                        );
                        let line = get_current_line(&preview);
                        let mut column = 0;
                        let mut painted_cursors = 0;
                        for span in &line.spans {
                            if span.style.bg == Some(themes[0].selected_background) {
                                assert_eq!(column, usize::from(preview.cursor_column));
                                painted_cursors += 1;
                            }
                            column += span.width();
                        }
                        assert_eq!(painted_cursors, 1);
                        if session.line_indentation.is_some() {
                            assert_eq!(preview.lines.len(), budget.min(ranges.len()));
                            assert!(preview.lines.iter().all(|line| line.width() > 0));
                            if available_rows >= 9 && current + 5 < ranges.len() {
                                assert_eq!(usize::from(preview.cursor_row), current.min(3));
                            }
                        } else if available_rows >= 4 && current + 3 < ranges.len() {
                            assert_eq!(usize::from(preview.cursor_row), current.min(1));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn zero_sized_windows_and_empty_targets_are_safe() {
        let themes = Theme::load_bundled().unwrap();
        let highlights = CodeHighlights::create_unhighlighted();
        for target in ["", "hello"] {
            let session = TypingSession::create(target, None);
            for (width, available_rows) in [(0, 4), (72, 0)] {
                let preview = TypingPreview {
                    session: &session,
                    theme: &themes[0],
                    width,
                }
                .build_window(PreviewWindowInput {
                    highlights: &highlights,
                    available_rows,
                });
                assert!(preview.lines.is_empty());
                assert_eq!((preview.cursor_row, preview.cursor_column), (0, 0));
            }
        }
    }

    #[test]
    fn empty_targets_do_not_create_phantom_history_rows() {
        let themes = Theme::load_bundled().unwrap();
        for session in [
            TypingSession::create("", None),
            TypingSession::create_code("", None),
        ] {
            let preview = TypingPreview {
                session: &session,
                theme: &themes[0],
                width: 72,
            }
            .build_visible_lines();
            assert!(preview.lines.is_empty());
            assert_eq!((preview.cursor_row, preview.cursor_column), (0, 0));
        }
    }

    #[test]
    fn wrapping_keeps_word_separators_on_the_preceding_line() {
        let themes = Theme::load_bundled().unwrap();
        let session = TypingSession::create("one four five", Some(Duration::from_secs(30)));
        let preview = TypingPreview {
            session: &session,
            theme: &themes[0],
            width: 8,
        }
        .build_visible_lines();
        assert_eq!(collect_text(&preview), ["one ", "four ", "five"]);
    }

    #[test]
    fn resizing_preserves_separator_input_and_cursor_positions() {
        let themes = Theme::load_bundled().unwrap();
        let target = "one four five";
        let mut session = TypingSession::create(target, Some(Duration::from_secs(30)));
        for character in "one four".chars() {
            session.insert_character(character, Duration::ZERO);
        }
        for width in [13, 8, 9, 8, 13] {
            let preview = TypingPreview {
                session: &session,
                theme: &themes[0],
                width,
            }
            .build_visible_lines();
            let expected_lines = if width == 8 {
                vec!["one ", "four ", "five"]
            } else if width == 9 {
                vec!["one four ", "five"]
            } else {
                vec![target]
            };
            assert_eq!(collect_text(&preview), expected_lines);
            assert_eq!(preview.cursor_column, if width == 8 { 4 } else { 8 });
            assert_eq!(preview.cursor_row, u16::from(width == 8));
            assert_eq!(session.target[session.cursor], ' ');
            assert_eq!(session.typed.iter().collect::<String>(), "one four");
        }
        session.insert_character(' ', Duration::ZERO);
        let preview = TypingPreview {
            session: &session,
            theme: &themes[0],
            width: 8,
        }
        .build_visible_lines();
        assert_eq!(collect_text(&preview), ["one ", "four ", "five"]);
        assert_eq!(preview.cursor_row, 2);
        assert_eq!(preview.cursor_column, 0);
        assert_eq!(session.calculate_accuracy(), 100.0);
    }

    #[test]
    fn wrapped_separators_remain_visible_when_typed_incorrectly() {
        let themes = Theme::load_bundled().unwrap();
        let mut session = TypingSession::create("one four five", Some(Duration::from_secs(30)));
        for character in "one fourx".chars() {
            session.insert_character(character, Duration::ZERO);
        }
        session.enter_normal_mode();
        let preview = TypingPreview {
            session: &session,
            theme: &themes[0],
            width: 8,
        }
        .build_visible_lines();
        assert_eq!(collect_text(&preview), ["one ", "fourx", "five"]);
        assert_eq!(preview.cursor_row, 1);
        assert_eq!(preview.cursor_column, 4);
        let separator_style = get_current_line(&preview).spans[4].style;
        assert_eq!(separator_style.fg, Some(themes[0].error));
        assert_eq!(separator_style.bg, Some(themes[0].selected_background));
    }

    #[test]
    fn wrapping_preserves_code_indentation_and_explicit_newlines() {
        let themes = Theme::load_bundled().unwrap();
        let session = TypingSession::create("one four\n    five", Some(Duration::from_secs(30)));
        let preview = TypingPreview {
            session: &session,
            theme: &themes[0],
            width: 12,
        }
        .build_visible_lines();
        assert_eq!(collect_text(&preview), ["one four↵", "    five"]);
    }

    #[test]
    fn resizing_keeps_all_characters_without_leading_separator_spaces() {
        let themes = Theme::load_bundled().unwrap();
        for target in [
            "one four five six",
            "abcdefgh ijkl mnop",
            "界界 界界界 four five",
        ] {
            let mut session = TypingSession::create(target, Some(Duration::from_secs(30)));
            session.cursor = session.target.len() - 1;
            for width in 4..=24 {
                let preview = TypingPreview {
                    session: &session,
                    theme: &themes[0],
                    width,
                };
                let ranges = preview.wrap_target();
                let mut reconstructed = String::new();
                for range in ranges {
                    let line: String = session.target[range].iter().collect();
                    assert!(!line.starts_with(' '), "{target:?} at {width}: {line:?}");
                    assert!(
                        line.chars()
                            .map(|character| character.width().unwrap_or(1))
                            .sum::<usize>()
                            <= width as usize
                    );
                    reconstructed.push_str(&line);
                }
                assert_eq!(reconstructed, target);
            }
        }
    }

    #[test]
    fn long_passages_and_unicode_stay_bounded_after_resize() {
        let themes = Theme::load_bundled().unwrap();
        let target = "one 界界 two three four five six seven ".repeat(100);
        let mut session = TypingSession::create(&target, Some(Duration::from_secs(30)));
        for character in target.chars().take(130) {
            session.insert_character(character, Duration::ZERO);
        }
        for width in [8, 24, 36, 72] {
            let preview = TypingPreview {
                session: &session,
                theme: &themes[0],
                width,
            }
            .build_visible_lines();
            assert_eq!(preview.lines.len(), 4);
            assert_eq!(preview.cursor_row, 1);
            assert!(
                preview
                    .lines
                    .iter()
                    .all(|line| line.width() <= width as usize)
            );
            assert!(preview.cursor_column < width);
        }
    }

    #[test]
    fn errors_and_wide_wrong_characters_preserve_width_and_feedback() {
        let themes = Theme::load_bundled().unwrap();
        let mut session = TypingSession::create("hello world", Some(Duration::from_secs(30)));
        session.insert_character('h', Duration::ZERO);
        session.insert_character('界', Duration::ZERO);
        let preview = TypingPreview {
            session: &session,
            theme: &themes[0],
            width: 8,
        }
        .build_visible_lines();
        assert_eq!(
            get_current_line(&preview).spans[0].style.fg,
            Some(themes[0].success)
        );
        assert_eq!(
            get_current_line(&preview).spans[1].style.fg,
            Some(themes[0].error)
        );
        assert_eq!(
            get_current_line(&preview).spans[3].style.fg,
            Some(themes[0].dim)
        );
        assert!(preview.lines.iter().all(|line| line.width() <= 8));
    }

    #[test]
    fn last_line_does_not_repeat_and_blank_code_lines_are_preserved() {
        let themes = Theme::load_bundled().unwrap();
        let mut session = TypingSession::create("one\n\ntwo", Some(Duration::from_secs(30)));
        for character in "one\n".chars() {
            session.insert_character(character, Duration::ZERO);
        }
        let preview = TypingPreview {
            session: &session,
            theme: &themes[0],
            width: 72,
        }
        .build_visible_lines();
        assert_eq!(collect_text(&preview), ["one↵", "↵", "two"]);
        assert_eq!(preview.cursor_row, 1);
        session.insert_character('\n', Duration::ZERO);
        let preview = TypingPreview {
            session: &session,
            theme: &themes[0],
            width: 72,
        }
        .build_visible_lines();
        assert_eq!(collect_text(&preview), ["one↵", "↵", "two"]);
        assert_eq!(preview.cursor_row, 2);
    }
}
