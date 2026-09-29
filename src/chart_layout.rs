use tiny_skia::{Point, Rect};

use crate::{
    chart_plot::CellPixels,
    chart_text::{ChartFont, TextDirection, TextRun},
};

pub const CHART_WIDTH: u16 = 60;
pub const CHART_ROWS: u16 = 8;

pub struct ChartMeasure<'a> {
    pub width: u16,
    pub cell: CellPixels,
    pub font: &'a ChartFont,
    pub is_legend_visible: bool,
}

pub struct LayoutInput<'a> {
    pub area: ratatui::layout::Rect,
    pub cell: CellPixels,
    pub font: &'a ChartFont,
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub maximum_speed: f64,
    pub maximum_errors: usize,
    pub is_legend_visible: bool,
}

pub struct ChartLayout {
    pub width: u32,
    pub height: u32,
    pub visible_height: u32,
    pub visible_rows: u16,
    pub scale: f32,
    pub font_size: f32,
    pub plot: Rect,
    pub speed_ticks: Vec<f32>,
    pub time_ticks: Vec<f32>,
    pub labels: Vec<AxisLabel>,
    pub legend: Vec<LegendItem>,
}

pub struct AxisLabel {
    pub text: String,
    pub center: Point,
    pub direction: TextDirection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LegendKind {
    Wpm,
    Raw,
    Burst,
    Uncorrected,
    Corrected,
}

pub struct LegendItem {
    pub kind: LegendKind,
    pub text: &'static str,
    pub left: f32,
    pub center_y: f32,
    pub text_width: f32,
}

struct LegendInput<'a> {
    width: f32,
    top: f32,
    scale: f32,
    font: &'a ChartFont,
}

impl ChartLayout {
    pub fn measure_rows(input: ChartMeasure<'_>) -> u16 {
        if !input.is_legend_visible || input.cell.height == 0 {
            return CHART_ROWS;
        }
        CHART_ROWS.saturating_add(Self::legend_rows(&input))
    }

    fn legend_rows(input: &ChartMeasure<'_>) -> u16 {
        let scale = f32::from(input.cell.height) / 20.0;
        let (_, height) = Self::arrange_legend(LegendInput {
            width: f32::from(input.width.min(CHART_WIDTH)) * f32::from(input.cell.width),
            top: 0.0,
            scale,
            font: input.font,
        });
        (height / f32::from(input.cell.height.max(1))).ceil() as u16
    }

    pub fn create(input: LayoutInput<'_>) -> Option<Self> {
        if !input.maximum_speed.is_finite()
            || input.maximum_speed <= 0.0
            || !input.start_seconds.is_finite()
            || !input.end_seconds.is_finite()
            || input.end_seconds < input.start_seconds
            || input.area.width < 14
            || input.area.height < 3
            || input.cell.width == 0
            || input.cell.height == 0
        {
            return None;
        }
        let columns = input.area.width.min(CHART_WIDTH);
        let base_rows = input.area.height.min(CHART_ROWS);
        let legend_rows = Self::legend_rows(&ChartMeasure {
            width: columns,
            cell: input.cell,
            font: input.font,
            is_legend_visible: true,
        });
        let full_width = u32::from(columns) * u32::from(input.cell.width);
        let full_height =
            u32::from(base_rows.saturating_add(legend_rows)) * u32::from(input.cell.height);
        let reduction = full_width
            .div_ceil(1200)
            .max(full_height.div_ceil(800))
            .max(1);
        let width = (full_width / reduction).max(1);
        let raster_height = |rows: u16| {
            ((u64::from(rows) * u64::from(input.cell.height) * u64::from(width))
                / u64::from(full_width)) as u32
        };
        let height = raster_height(base_rows.saturating_add(legend_rows));
        if height == 0 || height > 800 {
            return None;
        }
        let base_height = raster_height(base_rows);
        let visible_rows = if input.is_legend_visible {
            input.area.height.min(base_rows.saturating_add(legend_rows))
        } else {
            base_rows
        };
        let visible_height = raster_height(visible_rows).clamp(1, height);
        let scale =
            (f64::from(input.cell.height) / 20.0 * f64::from(width) / f64::from(full_width)) as f32;
        if scale < 0.25 {
            return None;
        }
        let font_size = 12.0 * scale;
        let text_height = input.font.measure_height(font_size);
        let padding = 6.0 * scale;
        let speed_width = input.font.measure_width(TextRun {
            text: &Self::format_value(input.maximum_speed),
            pixels: font_size,
        });
        let errors_width = input.font.measure_width(TextRun {
            text: &Self::format_value(input.maximum_errors.max(1) as f64),
            pixels: font_size,
        });
        let left = (text_height + speed_width + padding * 4.0).ceil();
        let right = width as f32 - (text_height + errors_width + padding * 4.0).ceil();
        let top = (text_height / 2.0 + padding).ceil();
        let bottom = (base_height as f32 - text_height - padding * 2.0).floor();
        if right - left < 20.0 * scale || bottom - top < text_height {
            return None;
        }
        let plot = Rect::from_ltrb(left, top, right, bottom)?;
        let maximum_ticks =
            ((plot.height() / (text_height + 20.0 * scale)).floor() as usize + 1).clamp(2, 6);
        let speed_values = Self::axis_values(input.maximum_speed, maximum_ticks, false);
        let error_values =
            Self::axis_values(input.maximum_errors.max(1) as f64, maximum_ticks, true);
        let mut labels = vec![];
        let mut speed_ticks = vec![];
        for value in speed_values {
            let y = bottom - (value / input.maximum_speed) as f32 * plot.height();
            speed_ticks.push(y);
            let text = Self::format_value(value);
            let label_width = input.font.measure_width(TextRun {
                text: &text,
                pixels: font_size,
            });
            labels.push(AxisLabel {
                text,
                center: Point::from_xy(left - padding - label_width / 2.0, y),
                direction: TextDirection::Horizontal,
            });
        }
        for value in error_values {
            let y = bottom - (value / input.maximum_errors.max(1) as f64) as f32 * plot.height();
            let text = Self::format_value(value);
            let label_width = input.font.measure_width(TextRun {
                text: &text,
                pixels: font_size,
            });
            labels.push(AxisLabel {
                text,
                center: Point::from_xy(right + padding + label_width / 2.0, y),
                direction: TextDirection::Horizontal,
            });
        }
        let speed_title = if input.font.measure_width(TextRun {
            text: "Words per Minute",
            pixels: font_size,
        }) <= base_height as f32 - padding * 2.0
        {
            "Words per Minute"
        } else {
            "WPM"
        };
        labels.push(AxisLabel {
            text: speed_title.into(),
            center: Point::from_xy(padding + text_height / 2.0, (top + bottom) / 2.0),
            direction: TextDirection::Upward,
        });
        labels.push(AxisLabel {
            text: "Errors".into(),
            center: Point::from_xy(
                width as f32 - padding - text_height / 2.0,
                (top + bottom) / 2.0,
            ),
            direction: TextDirection::Downward,
        });
        let time_label_width = [input.start_seconds, input.end_seconds]
            .into_iter()
            .map(|value| {
                input.font.measure_width(TextRun {
                    text: &Self::format_seconds(value),
                    pixels: font_size,
                })
            })
            .fold(0.0_f32, f32::max);
        let time_capacity =
            ((plot.width() / (time_label_width + 20.0 * scale)).floor() as usize + 1).clamp(2, 30);
        let duration = input.end_seconds - input.start_seconds;
        let mut times = vec![input.start_seconds];
        if duration > 0.0 {
            let step = Self::nice_step(duration / (time_capacity - 1) as f64);
            let mut value = ((input.start_seconds / step).floor() + 1.0) * step;
            while value < input.end_seconds && times.len() < 30 {
                let previous = *times.last().unwrap();
                if (value - previous) / duration * f64::from(plot.width())
                    >= f64::from(time_label_width + padding)
                {
                    times.push(value);
                }
                value += step;
            }
            if times.len() > 1
                && (input.end_seconds - times.last().unwrap()) / duration * f64::from(plot.width())
                    < f64::from(time_label_width + padding)
            {
                times.pop();
            }
            times.push(input.end_seconds);
        }
        let mut time_ticks = vec![];
        for value in times {
            let fraction = if duration > 0.0 {
                ((value - input.start_seconds) / duration).clamp(0.0, 1.0) as f32
            } else {
                0.0
            };
            let x = left + fraction * plot.width();
            time_ticks.push(x);
            labels.push(AxisLabel {
                text: Self::format_seconds(value),
                center: Point::from_xy(x, bottom + padding + text_height / 2.0),
                direction: TextDirection::Horizontal,
            });
        }
        let (legend, _) = Self::arrange_legend(LegendInput {
            width: width as f32,
            top: base_height as f32,
            scale,
            font: input.font,
        });
        Some(Self {
            width,
            height,
            visible_height,
            visible_rows,
            scale,
            font_size,
            plot,
            speed_ticks,
            time_ticks,
            labels,
            legend,
        })
    }

    fn arrange_legend(input: LegendInput<'_>) -> (Vec<LegendItem>, f32) {
        let pixels = 11.0 * input.scale;
        let padding = 6.0 * input.scale;
        let line_height = input.font.measure_height(pixels) + 2.0 * input.scale;
        let swatch_width = 24.0 * input.scale;
        let mut left = padding;
        let mut row = 0;
        let mut entries = vec![];
        for (kind, long, short) in [
            (LegendKind::Wpm, "WPM: correct words", "Correct WPM"),
            (LegendKind::Raw, "Raw: with mistakes", "Raw input"),
            (LegendKind::Burst, "Burst: recent, shaded", "Recent burst"),
            (LegendKind::Uncorrected, "Uncorrected errors", "Uncorrected"),
            (LegendKind::Corrected, "All errors corrected", "Corrected"),
        ] {
            let text = if input.font.measure_width(TextRun { text: long, pixels }) + swatch_width
                <= input.width - padding * 2.0
            {
                long
            } else {
                short
            };
            let text_width = input.font.measure_width(TextRun { text, pixels });
            let width = swatch_width + text_width;
            if left > padding && left + width > input.width - padding {
                left = padding;
                row += 1;
            }
            entries.push(LegendItem {
                kind,
                text,
                left,
                center_y: input.top + padding + line_height * (row as f32 + 0.5),
                text_width,
            });
            left += width + padding * 2.0;
        }
        (entries, padding * 2.0 + (row + 1) as f32 * line_height)
    }

    fn nice_step(value: f64) -> f64 {
        if !value.is_finite() || value <= 0.0 {
            return 1.0;
        }
        let order = 10.0_f64.powf(value.log10().floor());
        let fraction = value / order;
        [1.0, 2.0, 5.0, 10.0]
            .into_iter()
            .find(|candidate| *candidate >= fraction)
            .unwrap_or(10.0)
            * order
    }

    fn axis_values(maximum: f64, capacity: usize, integers: bool) -> Vec<f64> {
        let step =
            Self::nice_step(maximum / (capacity - 1) as f64).max(if integers { 1.0 } else { 0.0 });
        let mut values = vec![0.0];
        for index in 1..capacity {
            let value = index as f64 * step;
            if value >= maximum {
                break;
            }
            values.push(value);
        }
        if values.len() > 1 && maximum - values.last().unwrap() < step * 0.5 {
            values.pop();
        }
        values.push(maximum);
        values
    }

    pub fn format_value(value: f64) -> String {
        if value >= 10_000.0 {
            format!("{value:.1e}")
        } else {
            format!("{value:.0}")
        }
    }

    fn format_seconds(value: f64) -> String {
        if value >= 10_000.0 {
            format!("{value:.1e}")
        } else if value >= 1.0 {
            format!("{value:.2}")
                .trim_end_matches('0')
                .trim_end_matches('.')
                .to_string()
        } else if value >= 0.01 {
            format!("{value:.2}").trim_end_matches('0').to_string()
        } else if value > 0.0 {
            format!("{value:.2e}")
        } else {
            "0".into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_fit_the_canvas_and_ticks_share_the_plot_coordinate_system() {
        let font = ChartFont::create();
        for cell in [
            CellPixels {
                width: 10,
                height: 20,
            },
            CellPixels {
                width: 11,
                height: 22,
            },
            CellPixels {
                width: 20,
                height: 40,
            },
            CellPixels {
                width: 36,
                height: 72,
            },
            CellPixels {
                width: 29,
                height: 57,
            },
            CellPixels {
                width: 31,
                height: 61,
            },
        ] {
            for width in [24, 34, 60] {
                for maximum_speed in [20.0, 100.0, 150.0, 1_000_000.0] {
                    for maximum_errors in [1, 2, usize::MAX] {
                        let rows = ChartLayout::measure_rows(ChartMeasure {
                            width,
                            cell,
                            font: &font,
                            is_legend_visible: true,
                        });
                        let layout = ChartLayout::create(LayoutInput {
                            area: ratatui::layout::Rect::new(0, 0, width, rows),
                            cell,
                            font: &font,
                            start_seconds: 1.0,
                            end_seconds: 15.0,
                            maximum_speed,
                            maximum_errors,
                            is_legend_visible: true,
                        })
                        .unwrap();
                        assert_eq!(layout.visible_height, layout.height);
                        for label in &layout.labels {
                            let text_width = font.measure_width(TextRun {
                                text: &label.text,
                                pixels: layout.font_size,
                            });
                            let text_height = font.measure_height(layout.font_size);
                            let (width, height) = match label.direction {
                                TextDirection::Horizontal => (text_width, text_height),
                                _ => (text_height, text_width),
                            };
                            assert!(
                                label.center.x - width / 2.0 >= 0.0
                                    && label.center.x + width / 2.0 <= layout.width as f32,
                                "horizontal clipping: {}",
                                label.text
                            );
                            assert!(
                                label.center.y - height / 2.0 >= 0.0
                                    && label.center.y + height / 2.0 < layout.height as f32,
                                "vertical clipping: {}",
                                label.text
                            );
                        }
                        for item in &layout.legend {
                            assert!(
                                item.left + 24.0 * layout.scale + item.text_width
                                    <= layout.width as f32
                            );
                            assert!(
                                item.center_y + font.measure_height(11.0 * layout.scale) / 2.0
                                    < layout.height as f32
                            );
                        }
                        assert_eq!(layout.speed_ticks.first(), Some(&layout.plot.bottom()));
                        assert_eq!(layout.speed_ticks.last(), Some(&layout.plot.top()));
                        assert_eq!(layout.time_ticks.first(), Some(&layout.plot.left()));
                        assert_eq!(layout.time_ticks.last(), Some(&layout.plot.right()));
                        for label in layout.labels.iter().filter(|label| {
                            matches!(label.direction, TextDirection::Horizontal)
                                && label.center.x < layout.plot.left()
                        }) {
                            assert!(layout.speed_ticks.contains(&label.center.y));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn small_frames_fractional_singletons_and_long_runs_have_safe_labels() {
        let font = ChartFont::create();
        for rows in [3, 4, 5, 7, 8] {
            for (start, end) in [
                (0.0004, 0.0004),
                (0.4, 0.4),
                (1.0, 1.234),
                (2.0, 15.0),
                (1.0, 1e12),
            ] {
                let layout = ChartLayout::create(LayoutInput {
                    area: ratatui::layout::Rect::new(0, 0, 24, rows),
                    cell: CellPixels {
                        width: 10,
                        height: 20,
                    },
                    font: &font,
                    start_seconds: start,
                    end_seconds: end,
                    maximum_speed: 100.0,
                    maximum_errors: 1,
                    is_legend_visible: false,
                })
                .unwrap();
                assert!(layout.visible_height <= u32::from(rows) * 20);
                for label in &layout.labels {
                    assert!(label.text.len() <= 16);
                    assert!(label.center.x >= 0.0 && label.center.x < layout.width as f32);
                    assert!(label.center.y >= 0.0 && label.center.y < layout.visible_height as f32);
                }
                assert_eq!(layout.time_ticks.len() == 1, start == end);
            }
        }
    }
}
