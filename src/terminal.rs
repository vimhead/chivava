use std::io::{self, Stdout};

use crossterm::{
    cursor::SetCursorStyle,
    execute,
    style::{Attribute, ResetColor, SetAttribute},
    terminal::{BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate},
};
use ratatui::{
    Terminal, TerminalOptions, Viewport,
    backend::{Backend, CrosstermBackend},
    buffer::{Buffer, Cell},
    layout::{Position, Size},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    app::Screen,
    engine::EditMode,
    kitty_graphics::{GraphicsEnvironment, KittyGraphics},
    result_chart::ChartPreview,
    ui::{View, ViewOutput},
};

pub struct InlineTerminal {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    requested_height: u16,
    terminal_size: Size,
    is_reflow_detected: bool,
    last_screen: Screen,
    is_legend_visible: bool,
    painted_row_widths: Vec<u16>,
    overflowed_row_widths: Vec<u16>,
    last_drawn_buffer: Option<Buffer>,
    result_rows_to_keep: u16,
    cursor_mode: Option<EditMode>,
    graphics: KittyGraphics,
    can_display_pixels: bool,
}

impl InlineTerminal {
    pub fn create(view: &View<'_>) -> io::Result<Self> {
        let width = crossterm::terminal::size()?.0;
        let requested_height = view.measure_height(width);
        let terminal = Terminal::with_options(
            CrosstermBackend::new(io::stdout()),
            TerminalOptions {
                viewport: Viewport::Inline(requested_height),
            },
        )?;
        let terminal_size = terminal.size()?;
        let term = std::env::var("TERM").ok();
        let program = std::env::var("TERM_PROGRAM").ok();
        let can_display_pixels = KittyGraphics::supports_environment(GraphicsEnvironment {
            term: term.as_deref(),
            program: program.as_deref(),
            is_multiplexed: std::env::var_os("TMUX").is_some() || std::env::var_os("STY").is_some(),
        });
        Ok(Self {
            terminal,
            requested_height,
            terminal_size,
            is_reflow_detected: false,
            last_screen: view.application.screen,
            is_legend_visible: view.application.is_chart_legend_visible,
            painted_row_widths: vec![],
            overflowed_row_widths: vec![],
            last_drawn_buffer: None,
            result_rows_to_keep: 0,
            cursor_mode: None,
            graphics: KittyGraphics::create(rand::random_range(1..=u32::MAX)),
            can_display_pixels,
        })
    }

    pub fn draw(&mut self, view: &View<'_>) -> io::Result<()> {
        let should_synchronize = self.can_display_pixels
            && self.last_screen == Screen::Results
            && view.application.screen == Screen::Results
            && self.is_legend_visible != view.application.is_chart_legend_visible;
        if should_synchronize {
            execute!(io::stdout(), BeginSynchronizedUpdate)?;
        }
        let result = self.draw_frame(view);
        let finish = if should_synchronize {
            execute!(io::stdout(), EndSynchronizedUpdate)
        } else {
            Ok(())
        };
        if result.is_ok() {
            self.is_legend_visible = view.application.is_chart_legend_visible;
        }
        result.and(finish)
    }

    fn draw_frame(&mut self, view: &View<'_>) -> io::Result<()> {
        let terminal_size = self.terminal.size()?;
        if terminal_size != self.terminal_size {
            self.graphics.clear(&mut io::stdout())?;
            self.correct_reflowed_viewport_origin(terminal_size)?;
        }
        self.terminal.autoresize()?;
        if terminal_size != self.terminal_size {
            self.painted_row_widths.clear();
            self.last_drawn_buffer = None;
            self.terminal_size = terminal_size;
        }
        let cell_pixels = if self.can_display_pixels && view.application.screen == Screen::Results {
            crossterm::terminal::window_size()
                .ok()
                .and_then(KittyGraphics::resolve_cell_pixels)
        } else {
            None
        };
        let requested_height = view.measure_output_height(ViewOutput {
            width: terminal_size.width,
            cell_pixels,
        });
        if requested_height != self.requested_height {
            self.graphics.hide(&mut io::stdout())?;
            let origin = self.terminal.get_frame().area().as_position();
            self.terminal.clear()?;
            self.terminal.set_cursor_position(origin)?;
            self.terminal = Terminal::with_options(
                CrosstermBackend::new(io::stdout()),
                TerminalOptions {
                    viewport: Viewport::Inline(requested_height),
                },
            )?;
            self.requested_height = requested_height;
            self.painted_row_widths.clear();
            self.last_drawn_buffer = None;
        } else if view.application.screen != self.last_screen {
            self.graphics.clear(&mut io::stdout())?;
            self.terminal.clear()?;
            self.painted_row_widths.clear();
            self.last_drawn_buffer = None;
        }
        if view.application.screen == Screen::Typing
            && self.cursor_mode != Some(view.application.session.mode)
        {
            let mode = view.application.session.mode;
            execute!(self.terminal.backend_mut(), resolve_cursor_style(mode))?;
            self.cursor_mode = Some(mode);
        }
        let mut image = None;
        let frame = self
            .terminal
            .draw(|frame| image = view.render_preview(frame, cell_pixels))?;
        let did_paint = self.last_drawn_buffer.as_ref() != Some(frame.buffer);
        if did_paint {
            self.last_drawn_buffer = Some(frame.buffer.clone());
        }
        let row_widths = measure_painted_row_widths(frame.buffer);
        self.painted_row_widths.resize(row_widths.len(), 0);
        for (previous_width, width) in self.painted_row_widths.iter_mut().zip(row_widths) {
            *previous_width = (*previous_width).max(width);
        }
        if did_paint && view.application.screen != Screen::Typing {
            let area = self.terminal.get_frame().area();
            self.terminal
                .set_cursor_position((area.x, area.bottom().saturating_sub(1)))?;
            self.terminal.backend_mut().flush()?;
        }
        self.graphics.update(&mut io::stdout(), image)?;
        self.last_screen = view.application.screen;
        self.result_rows_to_keep = if view.application.screen == Screen::Results {
            view.measure_visible_result_height(ChartPreview {
                area: self.terminal.get_frame().area(),
                cell_pixels,
            })
        } else {
            0
        };
        Ok(())
    }

    fn correct_reflowed_viewport_origin(&mut self, terminal_size: Size) -> io::Result<()> {
        if self.last_screen == Screen::Typing || self.painted_row_widths.is_empty() {
            return Ok(());
        }
        let area = self.terminal.get_frame().area();
        let cursor = self.terminal.get_cursor_position()?;
        let previous_anchor_row = area.bottom().saturating_sub(1);
        let previous_offset = area.height.saturating_sub(1);
        let mut reflowed_offset = previous_offset;
        if terminal_size.width < self.terminal_size.width {
            if !self.is_reflow_detected
                && cursor.y == previous_anchor_row
                && cursor.y < terminal_size.height.saturating_sub(1)
            {
                return Ok(());
            }
            self.is_reflow_detected = true;
            reflowed_offset = measure_reflowed_rows(
                &self.painted_row_widths[..usize::from(previous_offset)],
                terminal_size.width,
            );
        }
        self.overflowed_row_widths.extend(
            ReflowPrefix {
                row_widths: &self.painted_row_widths[..usize::from(previous_offset)],
                terminal_width: terminal_size.width,
                rows: reflowed_offset.saturating_sub(cursor.y),
            }
            .collect_widths(),
        );
        let origin_row = cursor.y.saturating_sub(reflowed_offset);
        let pending_rows = measure_reflowed_rows(&self.overflowed_row_widths, terminal_size.width);
        let restored_rows = origin_row.min(pending_rows);
        self.overflowed_row_widths = ReflowPrefix {
            row_widths: &self.overflowed_row_widths,
            terminal_width: terminal_size.width,
            rows: pending_rows - restored_rows,
        }
        .collect_widths();
        let origin_row = origin_row - restored_rows;
        // Ratatui's autoresize subtracts the old row offset, not the reflowed one.
        self.terminal.backend_mut().set_cursor_position(Position {
            x: 0,
            y: origin_row
                .saturating_add(previous_offset)
                .min(terminal_size.height.saturating_sub(1)),
        })?;
        self.terminal.backend_mut().flush()
    }

    pub fn finish(&mut self, screen: Screen) -> io::Result<()> {
        let area = self.terminal.get_frame().area();
        if screen == Screen::Results {
            self.terminal.set_cursor_position((
                0,
                (area.y + self.result_rows_to_keep).min(area.bottom().saturating_sub(1)),
            ))?;
            execute!(
                self.terminal.backend_mut(),
                SetAttribute(Attribute::Reset),
                ResetColor,
                Clear(ClearType::FromCursorDown)
            )?;
        } else {
            self.graphics.clear(&mut io::stdout())?;
            self.terminal.clear()?;
            self.terminal.set_cursor_position(area.as_position())?;
        }
        self.terminal.show_cursor()?;
        if screen == Screen::Results {
            self.graphics.retain_result_in_scrollback();
        }
        Ok(())
    }
}

impl Drop for InlineTerminal {
    fn drop(&mut self) {
        let _ = self.graphics.clear(&mut io::stdout());
    }
}

struct ReflowPrefix<'a> {
    row_widths: &'a [u16],
    terminal_width: u16,
    rows: u16,
}

impl ReflowPrefix<'_> {
    fn collect_widths(&self) -> Vec<u16> {
        let terminal_width = self.terminal_width.max(1);
        let mut remaining_rows = self.rows;
        let mut widths = vec![];
        for &width in self.row_widths {
            if remaining_rows == 0 {
                break;
            }
            let rows = width.max(1).div_ceil(terminal_width).min(remaining_rows);
            widths.push(width.min(rows.saturating_mul(terminal_width)));
            remaining_rows -= rows;
        }
        widths
    }
}

fn measure_painted_row_widths(buffer: &Buffer) -> Vec<u16> {
    let empty = Cell::default();
    buffer
        .content
        .chunks(usize::from(buffer.area.width.max(1)))
        .map(|row| {
            row.iter()
                .enumerate()
                .rev()
                .find(|(_, cell)| **cell != empty)
                .map(|(column, cell)| (column + cell.symbol().width().max(1)) as u16)
                .unwrap_or(0)
        })
        .collect()
}

fn measure_reflowed_rows(row_widths: &[u16], terminal_width: u16) -> u16 {
    row_widths.iter().fold(0, |rows, width| {
        rows.saturating_add((*width).max(1).div_ceil(terminal_width.max(1)))
    })
}

fn resolve_cursor_style(mode: EditMode) -> SetCursorStyle {
    match mode {
        EditMode::Insert => SetCursorStyle::SteadyBar,
        EditMode::Normal => SetCursorStyle::SteadyBlock,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_measurement_includes_wide_glyphs_and_painted_blank_cells() {
        use ratatui::{
            layout::Rect,
            style::{Color, Style},
        };

        let mut buffer = Buffer::empty(Rect::new(0, 0, 12, 3));
        buffer.set_string(1, 0, "wide界", Style::default());
        buffer.set_style(Rect::new(1, 1, 8, 1), Style::default().fg(Color::Red));
        assert_eq!(measure_painted_row_widths(&buffer), vec![7, 9, 0]);
    }

    #[test]
    fn resize_measurement_counts_reflowed_rows_without_dropping_blank_lines() {
        assert_eq!(measure_reflowed_rows(&[73, 73, 2], 72), 5);
        assert_eq!(measure_reflowed_rows(&[0, 1, 36, 37], 36), 5);
        assert_eq!(measure_reflowed_rows(&[], 36), 0);
        assert_eq!(measure_reflowed_rows(&[0, 2], 0), 3);
    }

    #[test]
    fn overflow_tracking_retains_only_the_owned_rows_pushed_into_scrollback() {
        let overflow = ReflowPrefix {
            row_widths: &[25, 95, 2],
            terminal_width: 26,
            rows: 3,
        }
        .collect_widths();
        assert_eq!(overflow, vec![25, 52]);
        assert_eq!(measure_reflowed_rows(&overflow, 120), 2);
        assert_eq!(
            ReflowPrefix {
                row_widths: &overflow,
                terminal_width: 120,
                rows: 1,
            }
            .collect_widths(),
            vec![25]
        );
        assert!(
            ReflowPrefix {
                row_widths: &overflow,
                terminal_width: 120,
                rows: 0,
            }
            .collect_widths()
            .is_empty()
        );
    }

    #[test]
    fn insert_mode_uses_a_thin_bar() {
        assert!(matches!(
            resolve_cursor_style(EditMode::Insert),
            SetCursorStyle::SteadyBar
        ));
    }

    #[test]
    fn normal_mode_uses_a_block() {
        assert!(matches!(
            resolve_cursor_style(EditMode::Normal),
            SetCursorStyle::SteadyBlock
        ));
    }
}
