use ratatui::{layout::Rect, style::Color};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Point, Stroke, StrokeDash, Transform};

use crate::{
    chart_layout::{CHART_WIDTH, ChartLayout, LayoutInput, LegendKind},
    chart_text::{ChartFont, ChartLabel, TextDirection, TextRun},
    statistics::ChartMetric,
    theme::Theme,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlotPoint {
    pub seconds: f64,
    pub value: f64,
    pub color: Color,
    pub is_uncorrected_error: bool,
}

pub struct PlotTrace {
    pub metric: ChartMetric,
    pub points: Vec<PlotPoint>,
}

pub struct PlotInput<'a> {
    pub traces: &'a [PlotTrace],
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub maximum: f64,
    pub maximum_errors: usize,
    pub theme: &'a Theme,
    pub font: &'a ChartFont,
    pub is_legend_visible: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellPixels {
    pub width: u16,
    pub height: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChartImage {
    pub area: Rect,
    pub width: u32,
    pub height: u32,
    pub visible_height: u32,
    pub rgba: Vec<u8>,
}

impl PlotInput<'_> {
    pub fn render_image(&self, area: Rect, cell: CellPixels) -> Option<ChartImage> {
        let layout = ChartLayout::create(LayoutInput {
            area,
            cell,
            font: self.font,
            start_seconds: self.start_seconds,
            end_seconds: self.end_seconds,
            maximum_speed: self.maximum,
            maximum_errors: self.maximum_errors,
            is_legend_visible: self.is_legend_visible,
        })?;
        let mut canvas = ChartCanvas {
            pixmap: Pixmap::new(layout.width, layout.height).expect("bounded chart dimensions"),
            projection: PlotProjection {
                left: layout.plot.left(),
                right: layout.plot.right(),
                top: layout.plot.top(),
                bottom: layout.plot.bottom(),
                start_seconds: self.start_seconds,
                end_seconds: self.end_seconds,
                maximum: self.maximum,
            },
            style_scale: layout.scale,
        };
        let decoration = CanvasDecoration {
            layout: &layout,
            font: self.font,
            theme: self.theme,
        };
        canvas.draw_grid(decoration);
        if let Some(burst) = self
            .traces
            .iter()
            .find(|trace| trace.metric == ChartMetric::Burst)
        {
            canvas.draw_burst_fill(burst);
        }
        for metric in [
            ChartMetric::Burst,
            ChartMetric::Raw,
            ChartMetric::Wpm,
            ChartMetric::Errors,
        ] {
            if let Some(trace) = self.traces.iter().find(|trace| trace.metric == metric) {
                canvas.draw_trace(trace);
            }
        }
        canvas.draw_labels(decoration);
        canvas.draw_legend(decoration);
        Some(ChartImage {
            area: Rect {
                width: area.width.min(CHART_WIDTH),
                height: layout.visible_rows,
                ..area
            },
            width: layout.width,
            height: layout.height,
            visible_height: layout.visible_height,
            rgba: canvas
                .pixmap
                .pixels()
                .iter()
                .flat_map(|pixel| {
                    let color = pixel.demultiply();
                    [color.red(), color.green(), color.blue(), color.alpha()]
                })
                .collect(),
        })
    }
}

#[derive(Clone, Copy)]
struct CanvasDecoration<'a> {
    layout: &'a ChartLayout,
    font: &'a ChartFont,
    theme: &'a Theme,
}

struct ChartCanvas {
    pixmap: Pixmap,
    projection: PlotProjection,
    style_scale: f32,
}

impl ChartCanvas {
    fn draw_grid(&mut self, input: CanvasDecoration<'_>) {
        let mut grid = PathBuilder::new();
        for &y in &input.layout.speed_ticks {
            grid.move_to(self.projection.left, y);
            grid.line_to(self.projection.right, y);
        }
        for &x in &input.layout.time_ticks {
            grid.move_to(x, self.projection.top);
            grid.line_to(x, self.projection.bottom);
        }
        if let Some(path) = grid.finish() {
            self.pixmap.stroke_path(
                &path,
                &create_paint(PaintInput {
                    color: input.theme.dim,
                    alpha: 65,
                }),
                &Stroke {
                    width: self.style_scale,
                    ..Stroke::default()
                },
                Transform::identity(),
                None,
            );
        }
    }

    fn draw_labels(&mut self, input: CanvasDecoration<'_>) {
        for label in &input.layout.labels {
            input.font.paint(
                &mut self.pixmap,
                ChartLabel {
                    run: TextRun {
                        text: &label.text,
                        pixels: input.layout.font_size,
                    },
                    center: label.center,
                    direction: label.direction,
                    rgba: resolve_rgba(input.theme.muted),
                },
            );
        }
    }

    fn draw_legend(&mut self, input: CanvasDecoration<'_>) {
        for entry in &input.layout.legend {
            let color = match entry.kind {
                LegendKind::Wpm | LegendKind::Raw => input.theme.accent,
                LegendKind::Burst => input.theme.dim,
                LegendKind::Uncorrected => input.theme.error,
                LegendKind::Corrected => input.theme.success,
            };
            let alpha = if entry.kind == LegendKind::Raw {
                153
            } else {
                255
            };
            let scale = self.style_scale;
            let mut swatch = PathBuilder::new();
            if matches!(entry.kind, LegendKind::Uncorrected | LegendKind::Corrected) {
                let x = entry.left + 9.0 * scale;
                let y = entry.center_y;
                let radius = 3.0 * scale;
                swatch.move_to(x - radius, y - radius);
                swatch.line_to(x + radius, y + radius);
                swatch.move_to(x - radius, y + radius);
                swatch.line_to(x + radius, y - radius);
            } else {
                swatch.move_to(entry.left, entry.center_y);
                swatch.line_to(entry.left + 18.0 * scale, entry.center_y);
            }
            if let Some(path) = swatch.finish() {
                self.pixmap.stroke_path(
                    &path,
                    &create_paint(PaintInput { color, alpha }),
                    &Stroke {
                        width: if matches!(entry.kind, LegendKind::Wpm | LegendKind::Burst) {
                            3.0 * scale
                        } else {
                            2.0 * scale
                        },
                        dash: if entry.kind == LegendKind::Raw {
                            StrokeDash::new(vec![5.0 * scale; 2], 0.0)
                        } else {
                            None
                        },
                        ..Stroke::default()
                    },
                    Transform::identity(),
                    None,
                );
            }
            let mut rgba = resolve_rgba(color);
            rgba[3] = alpha;
            input.font.paint(
                &mut self.pixmap,
                ChartLabel {
                    run: TextRun {
                        text: entry.text,
                        pixels: 11.0 * scale,
                    },
                    center: Point::from_xy(
                        entry.left + 24.0 * scale + entry.text_width / 2.0,
                        entry.center_y,
                    ),
                    direction: TextDirection::Horizontal,
                    rgba,
                },
            );
        }
    }

    fn draw_burst_fill(&mut self, trace: &PlotTrace) {
        let points = self.projection.project_trace(trace);
        if points.len() < 2 {
            return;
        }
        let mut fill = self.projection.build_curve(&points);
        fill.line_to(points.last().unwrap().x, self.projection.bottom);
        fill.line_to(points[0].x, self.projection.bottom);
        fill.close();
        if let Some(path) = fill.finish() {
            self.pixmap.fill_path(
                &path,
                &create_paint(PaintInput {
                    color: trace.points[0].color,
                    alpha: 28,
                }),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }

    fn draw_trace(&mut self, trace: &PlotTrace) {
        if trace.metric == ChartMetric::Errors {
            self.draw_errors(trace);
            return;
        }
        let points = self.projection.project_trace(trace);
        if points.is_empty() {
            return;
        }
        let is_raw = trace.metric == ChartMetric::Raw;
        let paint = create_paint(PaintInput {
            color: trace.points[0].color,
            alpha: if is_raw { 153 } else { 255 },
        });
        let stroke = Stroke {
            width: (if is_raw { 2.0 } else { 3.0 }) * self.style_scale,
            dash: if is_raw {
                StrokeDash::new(vec![8.0 * self.style_scale; 2], 0.0)
            } else {
                None
            },
            ..Stroke::default()
        };
        if let Some(path) = self.projection.build_curve(&points).finish() {
            self.pixmap
                .stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
        if !is_raw || points.len() == 1 {
            let mut markers = PathBuilder::new();
            for point in &points {
                markers.push_circle(point.x, point.y, self.style_scale.max(stroke.width / 2.0));
            }
            if let Some(path) = markers.finish() {
                self.pixmap.fill_path(
                    &path,
                    &paint,
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
        }
    }

    fn draw_errors(&mut self, trace: &PlotTrace) {
        for is_uncorrected in [false, true] {
            for point in trace
                .points
                .iter()
                .filter(|point| point.is_uncorrected_error == is_uncorrected)
            {
                let center = self.projection.project_point(point);
                let radius = 3.0 * self.style_scale;
                let mut marker = PathBuilder::new();
                marker.move_to(center.x - radius, center.y - radius);
                marker.line_to(center.x + radius, center.y + radius);
                marker.move_to(center.x - radius, center.y + radius);
                marker.line_to(center.x + radius, center.y - radius);
                if let Some(path) = marker.finish() {
                    self.pixmap.stroke_path(
                        &path,
                        &create_paint(PaintInput {
                            color: point.color,
                            alpha: 255,
                        }),
                        &Stroke {
                            width: 2.0 * self.style_scale,
                            ..Stroke::default()
                        },
                        Transform::identity(),
                        None,
                    );
                }
            }
        }
    }
}

struct PlotProjection {
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
    start_seconds: f64,
    end_seconds: f64,
    maximum: f64,
}

impl PlotProjection {
    fn project_point(&self, point: &PlotPoint) -> Point {
        let duration = self.end_seconds - self.start_seconds;
        let fraction = if duration > 0.0 {
            ((point.seconds - self.start_seconds) / duration).clamp(0.0, 1.0)
        } else {
            0.0
        };
        Point::from_xy(
            self.left + fraction as f32 * (self.right - self.left),
            self.bottom
                - (point.value / self.maximum).clamp(0.0, 1.0) as f32 * (self.bottom - self.top),
        )
    }

    fn project_trace(&self, trace: &PlotTrace) -> Vec<Point> {
        trace
            .points
            .iter()
            .map(|point| self.project_point(point))
            .collect()
    }

    fn build_curve(&self, points: &[Point]) -> PathBuilder {
        let mut path = PathBuilder::new();
        let Some(first) = points.first() else {
            return path;
        };
        path.move_to(first.x, first.y);
        let knots: Vec<_> = points
            .iter()
            .enumerate()
            .map(|(index, &position)| {
                let previous = points[index.saturating_sub(1)];
                let next = points[(index + 1).min(points.len() - 1)];
                let before = (position.x - previous.x).hypot(position.y - previous.y);
                let after = (next.x - position.x).hypot(next.y - position.y);
                let total = before + after;
                let incoming_weight = if total > 0.0 {
                    0.5 * before / total
                } else {
                    0.0
                };
                let outgoing_weight = if total > 0.0 {
                    0.5 * after / total
                } else {
                    0.0
                };
                CurveKnot {
                    position,
                    incoming: Point::from_xy(
                        (position.x - (next.x - previous.x) * incoming_weight)
                            .clamp(previous.x, position.x),
                        (position.y - (next.y - previous.y) * incoming_weight)
                            .clamp(self.top, self.bottom),
                    ),
                    outgoing: Point::from_xy(
                        (position.x + (next.x - previous.x) * outgoing_weight)
                            .clamp(position.x, next.x),
                        (position.y + (next.y - previous.y) * outgoing_weight)
                            .clamp(self.top, self.bottom),
                    ),
                }
            })
            .collect();
        for pair in knots.windows(2) {
            let (left, right) = (&pair[0], &pair[1]);
            path.cubic_to(
                left.outgoing.x,
                left.outgoing.y,
                right.incoming.x,
                right.incoming.y,
                right.position.x,
                right.position.y,
            );
        }
        path
    }
}

struct CurveKnot {
    position: Point,
    incoming: Point,
    outgoing: Point,
}

struct PaintInput {
    color: Color,
    alpha: u8,
}

fn create_paint(input: PaintInput) -> Paint<'static> {
    let [red, green, blue, _] = resolve_rgba(input.color);
    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    paint.set_color_rgba8(red, green, blue, input.alpha);
    paint
}

fn resolve_rgba(color: Color) -> [u8; 4] {
    let index = match color {
        Color::Rgb(red, green, blue) => return [red, green, blue, 255],
        Color::Indexed(index) => index,
        Color::Black => 0,
        Color::Red => 1,
        Color::Green => 2,
        Color::Yellow => 3,
        Color::Blue => 4,
        Color::Magenta => 5,
        Color::Cyan => 6,
        Color::Gray => 7,
        Color::DarkGray => 8,
        Color::LightRed => 9,
        Color::LightGreen => 10,
        Color::LightYellow => 11,
        Color::LightBlue => 12,
        Color::LightMagenta => 13,
        Color::LightCyan => 14,
        Color::White | Color::Reset => 15,
    };
    if index < 16 {
        let colors = [
            [0, 0, 0],
            [128, 0, 0],
            [0, 128, 0],
            [128, 128, 0],
            [0, 0, 128],
            [128, 0, 128],
            [0, 128, 128],
            [192, 192, 192],
            [128, 128, 128],
            [255, 0, 0],
            [0, 255, 0],
            [255, 255, 0],
            [0, 0, 255],
            [255, 0, 255],
            [0, 255, 255],
            [255, 255, 255],
        ];
        let [red, green, blue] = colors[usize::from(index)];
        [red, green, blue, 255]
    } else if index < 232 {
        let index = index - 16;
        let level = |value| if value == 0 { 0 } else { 55 + value * 40 };
        [
            level(index / 36),
            level(index / 6 % 6),
            level(index % 6),
            255,
        ]
    } else {
        let gray = 8 + (index - 232) * 10;
        [gray, gray, gray, 255]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trace(metric: ChartMetric, value: f64) -> PlotTrace {
        PlotTrace {
            metric,
            points: [1.0, 10.0]
                .into_iter()
                .map(|seconds| PlotPoint {
                    seconds,
                    value,
                    color: Color::Rgb(255, 0, 0),
                    is_uncorrected_error: false,
                })
                .collect(),
        }
    }

    fn render(traces: &[PlotTrace]) -> ChartImage {
        PlotInput {
            traces,
            start_seconds: 1.0,
            end_seconds: 10.0,
            maximum: 100.0,
            maximum_errors: 1,
            theme: &Theme::load_bundled().unwrap().remove(0),
            font: &ChartFont::create(),
            is_legend_visible: false,
        }
        .render_image(
            Rect::new(2, 3, 50, 5),
            CellPixels {
                width: 10,
                height: 20,
            },
        )
        .unwrap()
    }

    fn count_red_pixels_in_column(image: &ChartImage, x: u32) -> usize {
        (0..image.height)
            .filter(|y| {
                let offset = ((y * image.width + x) * 4) as usize;
                image.rgba[offset] > 200
                    && image.rgba[offset + 1] < 80
                    && image.rgba[offset + 2] < 80
                    && image.rgba[offset + 3] > 60
            })
            .count()
    }

    #[test]
    fn speed_lines_are_thick_antialiased_and_raw_is_dashed() {
        for metric in [ChartMetric::Wpm, ChartMetric::Burst, ChartMetric::Raw] {
            let image = render(&[trace(metric, 50.0)]);
            let columns: Vec<_> = (100..400)
                .map(|x| count_red_pixels_in_column(&image, x))
                .collect();
            if metric == ChartMetric::Raw {
                assert!(columns.contains(&0));
                assert!(columns.iter().any(|count| *count >= 2));
            } else {
                assert!(columns.iter().all(|count| *count >= 3));
            }
            assert!(
                image
                    .rgba
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|pixel| pixel[0] > 200 && pixel[3] > 0 && pixel[3] < 255)
            );
        }
    }

    #[test]
    fn all_traces_keep_their_own_colors_and_raw_is_translucent() {
        let mut burst = trace(ChartMetric::Burst, 80.0);
        for point in &mut burst.points {
            point.color = Color::Rgb(80, 80, 80);
        }
        let mut errors = trace(ChartMetric::Errors, 100.0);
        errors.points[0].color = Color::Rgb(0, 255, 0);
        errors.points[1].is_uncorrected_error = true;
        let traces = [
            trace(ChartMetric::Wpm, 40.0),
            trace(ChartMetric::Raw, 47.0),
            burst,
            errors,
        ];
        let image = render(&traces);
        for expected in [[255, 0, 0, 255], [80, 80, 80, 255], [0, 255, 0, 255]] {
            assert!(image.rgba.as_chunks::<4>().0.contains(&expected));
        }
        let raw = render(&[trace(ChartMetric::Raw, 47.0)]);
        assert!(
            raw.rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[0] > 200 && pixel[1] < 20 && pixel[3] == 153)
        );
        assert!(
            !raw.rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[0] > 200 && pixel[1] < 20 && pixel[3] == 255)
        );
        assert_eq!(image, render(&traces));
    }

    #[test]
    fn error_crosses_are_visible_and_uncorrected_overlaps_win() {
        let mut errors = trace(ChartMetric::Errors, 100.0);
        errors.points.truncate(1);
        errors.points[0].color = Color::Rgb(0, 255, 0);
        errors.points.push(PlotPoint {
            color: Color::Rgb(255, 0, 0),
            is_uncorrected_error: true,
            ..errors.points[0]
        });
        let image = render(&[errors]);
        let red = image.rgba[..(image.width * image.visible_height * 4) as usize]
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| pixel[0] > pixel[1] && pixel[0] > pixel[2] && pixel[3] > 100)
            .count();
        assert!(red >= 20, "marker has only {red} colored pixels");
        assert!(!image.rgba.as_chunks::<4>().0.contains(&[0, 255, 0, 255]));
    }

    #[test]
    fn bezier_paths_keep_actual_endpoints_and_fractional_tails_monotone_in_time() {
        let projection = PlotProjection {
            start_seconds: 0.0,
            left: 0.0,
            right: 100.0,
            top: 0.0,
            bottom: 100.0,
            end_seconds: 10.0,
            maximum: 100.0,
        };
        let points = [
            Point::from_xy(10.0, 80.0),
            Point::from_xy(90.0, 10.0),
            Point::from_xy(91.0, 90.0),
        ];
        let path = projection.build_curve(&points).finish().unwrap();
        let mut start = points[0];
        let mut curves = 0;
        for segment in path.segments() {
            if let tiny_skia::PathSegment::CubicTo(first, second, end) = segment {
                assert!(first.x >= start.x && first.x <= end.x);
                assert!(second.x >= start.x && second.x <= end.x);
                assert!((0.0..=100.0).contains(&first.y) && (0.0..=100.0).contains(&second.y));
                start = end;
                curves += 1;
            }
        }
        assert_eq!(curves, 2);
        assert_eq!(start, points[2]);
    }

    #[test]
    fn half_tension_produces_expected_cubic_controls() {
        let projection = PlotProjection {
            start_seconds: 0.0,
            left: 0.0,
            right: 100.0,
            top: 0.0,
            bottom: 100.0,
            end_seconds: 10.0,
            maximum: 100.0,
        };
        let points = [
            Point::from_xy(0.0, 50.0),
            Point::from_xy(50.0, 0.0),
            Point::from_xy(100.0, 50.0),
        ];
        let path = projection.build_curve(&points).finish().unwrap();
        let controls: Vec<_> = path
            .segments()
            .filter_map(|segment| match segment {
                tiny_skia::PathSegment::CubicTo(first, second, end) => Some([first, second, end]),
                _ => None,
            })
            .collect();
        assert_eq!(
            controls,
            [
                [
                    Point::from_xy(25.0, 25.0),
                    Point::from_xy(25.0, 0.0),
                    points[1]
                ],
                [
                    Point::from_xy(75.0, 0.0),
                    Point::from_xy(75.0, 25.0),
                    points[2]
                ],
            ]
        );
    }

    #[test]
    fn recorded_time_range_fills_the_plot_without_relabeling_samples() {
        for (start, end) in [(1.0, 15.0), (2.0, 15.0), (0.4, 0.4)] {
            let projection = PlotProjection {
                left: 4.0,
                right: 525.0,
                top: 10.0,
                bottom: 89.0,
                start_seconds: start,
                end_seconds: end,
                maximum: 100.0,
            };
            let point = |seconds| PlotPoint {
                seconds,
                value: 50.0,
                color: Color::Red,
                is_uncorrected_error: false,
            };
            assert_eq!(projection.project_point(&point(start)).x, 4.0);
            if end > start {
                assert_eq!(projection.project_point(&point(end)).x, 525.0);
                assert_eq!(
                    projection.project_point(&point((start + end) / 2.0)).x,
                    264.5
                );
            }
        }
        let image = render(&[trace(ChartMetric::Wpm, 50.0)]);
        let layout = ChartLayout::create(LayoutInput {
            area: image.area,
            cell: CellPixels {
                width: 10,
                height: 20,
            },
            font: &ChartFont::create(),
            start_seconds: 1.0,
            end_seconds: 10.0,
            maximum_speed: 100.0,
            maximum_errors: 1,
            is_legend_visible: false,
        })
        .unwrap();
        assert!(count_red_pixels_in_column(&image, layout.plot.left() as u32) >= 3);
        assert!(count_red_pixels_in_column(&image, layout.plot.right() as u32) >= 3);
    }

    #[test]
    fn legend_cropping_reuses_identical_axes_traces_and_text_pixels_at_all_scales() {
        use crate::chart_layout::ChartMeasure;
        let theme = Theme::load_bundled().unwrap().remove(0);
        let font = ChartFont::create();
        let traces = [trace(ChartMetric::Wpm, 50.0)];
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
                let rows = ChartLayout::measure_rows(ChartMeasure {
                    width,
                    cell,
                    font: &font,
                    is_legend_visible: true,
                });
                let mut input = PlotInput {
                    traces: &traces,
                    start_seconds: 1.0,
                    end_seconds: 10.0,
                    maximum: 100.0,
                    maximum_errors: 1,
                    theme: &theme,
                    font: &font,
                    is_legend_visible: false,
                };
                let hidden = input.render_image(Rect::new(2, 3, width, 8), cell).unwrap();
                input.is_legend_visible = true;
                let visible = input
                    .render_image(Rect::new(2, 3, width, rows), cell)
                    .unwrap();
                assert_eq!(
                    (hidden.width, hidden.height),
                    (visible.width, visible.height)
                );
                assert_eq!(hidden.rgba, visible.rgba);
                assert!(hidden.visible_height < visible.visible_height);
                assert_eq!(visible.visible_height, visible.height);
                assert!(visible.width <= 1200 && visible.height <= 800);
                for image in [&hidden, &visible] {
                    let displayed_height_numerator = u64::from(image.visible_height)
                        * u64::from(image.area.width)
                        * u64::from(cell.width);
                    let allocated_height_numerator = u64::from(image.area.height)
                        * u64::from(cell.height)
                        * u64::from(image.width);
                    assert!(
                        displayed_height_numerator <= allocated_height_numerator,
                        "image overlaps the native footer"
                    );
                }
            }
        }
    }

    #[test]
    fn single_samples_large_cells_and_indexed_colors_remain_safe() {
        let mut speed = trace(ChartMetric::Wpm, 50.0);
        speed.points.truncate(1);
        let traces = [speed];
        let input = PlotInput {
            traces: &traces,
            start_seconds: 1.0,
            end_seconds: 1.0,
            maximum: 100.0,
            maximum_errors: 1,
            theme: &Theme::load_bundled().unwrap().remove(0),
            font: &ChartFont::create(),
            is_legend_visible: false,
        };
        let image = input
            .render_image(
                Rect::new(0, 0, 60, 8),
                CellPixels {
                    width: u16::MAX,
                    height: u16::MAX,
                },
            )
            .unwrap();
        assert!(image.width <= 1200 && image.height <= 800);
        assert_eq!(image.rgba.len(), (image.width * image.height * 4) as usize);
        assert!(
            image
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[0] > 200 && pixel[1] < 20 && pixel[3] > 0)
        );
        assert_eq!(resolve_rgba(Color::Indexed(196)), [255, 0, 0, 255]);
        assert_eq!(resolve_rgba(Color::Indexed(244)), [128, 128, 128, 255]);
    }
}
