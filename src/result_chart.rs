use std::time::Duration;

use ratatui::layout::Rect;

use crate::{
    chart_layout::{CHART_ROWS, CHART_WIDTH},
    chart_plot::{CellPixels, ChartImage, PlotInput, PlotPoint, PlotTrace},
    chart_text::ChartFont,
    engine::Sample,
    statistics::{ChartMetric, TypingError},
    theme::Theme,
};

pub struct ResultChart<'a> {
    pub samples: &'a [Sample],
    pub errors: &'a [TypingError],
    pub elapsed: Duration,
    pub final_wpm: f64,
    pub final_raw_wpm: f64,
    pub theme: &'a Theme,
    pub font: &'a ChartFont,
    pub is_legend_visible: bool,
}

#[derive(Clone, Copy)]
pub struct ChartPreview {
    pub area: Rect,
    pub cell_pixels: Option<CellPixels>,
}

impl ResultChart<'_> {
    pub const HEIGHT: u16 = CHART_ROWS;

    pub fn render_image(&self, input: ChartPreview) -> Option<ChartImage> {
        let cell = input.cell_pixels?;
        let series = ChartSeries::collect(ChartDataInput {
            samples: self.samples,
            errors: self.errors,
            elapsed: self.elapsed,
            final_wpm: self.final_wpm,
            final_raw_wpm: self.final_raw_wpm,
        });
        if series.wpm.is_empty() {
            return None;
        }
        let traces = series.build_traces(self.theme);
        PlotInput {
            traces: &traces,
            start_seconds: series.start_seconds,
            end_seconds: series.end_seconds,
            maximum: series.maximum_wpm,
            maximum_errors: series.maximum_errors,
            font: self.font,
            theme: self.theme,
            is_legend_visible: self.is_legend_visible,
        }
        .render_image(
            Rect {
                width: input.area.width.min(CHART_WIDTH),
                ..input.area
            },
            cell,
        )
    }
}

struct ChartDataInput<'a> {
    samples: &'a [Sample],
    errors: &'a [TypingError],
    elapsed: Duration,
    final_wpm: f64,
    final_raw_wpm: f64,
}

struct ChartSeries {
    wpm: Vec<(f64, f64)>,
    raw: Vec<(f64, f64)>,
    burst: Vec<(f64, f64)>,
    error_points: Vec<(f64, f64)>,
    uncorrected_errors: Vec<(f64, f64)>,
    start_seconds: f64,
    end_seconds: f64,
    maximum_wpm: f64,
    maximum_errors: usize,
}

impl ChartSeries {
    fn collect(input: ChartDataInput<'_>) -> Self {
        let mut valid: Vec<&Sample> = vec![];
        for sample in input.samples.iter().filter(|sample| {
            sample.seconds.is_finite()
                && sample.seconds > 0.0
                && [sample.wpm, sample.raw_wpm, sample.burst_wpm]
                    .iter()
                    .all(|speed| speed.is_finite() && *speed >= 0.0)
        }) {
            if let Some(last) = valid.last_mut()
                && last.seconds == sample.seconds
            {
                *last = sample;
            } else {
                valid.push(sample);
            }
        }
        let mut wpm: Vec<_> = valid
            .iter()
            .map(|sample| (sample.seconds, sample.wpm))
            .collect();
        let mut raw: Vec<_> = valid
            .iter()
            .map(|sample| (sample.seconds, sample.raw_wpm))
            .collect();
        let burst = Self::smooth_bursts(&valid);
        let mut error_counts: Vec<_> = valid.iter().map(|sample| sample.errors).collect();
        let elapsed = input.elapsed.as_secs_f64();
        let last_boundary = valid.last().map(|sample| sample.seconds);
        if elapsed > 0.0
            && [input.final_wpm, input.final_raw_wpm]
                .iter()
                .all(|speed| speed.is_finite() && *speed >= 0.0)
            && last_boundary.is_none_or(|seconds| seconds < elapsed)
        {
            wpm.push((elapsed, input.final_wpm));
            raw.push((elapsed, input.final_raw_wpm));
            error_counts.push(
                input
                    .errors
                    .iter()
                    .filter(|error| {
                        error.seconds.is_finite()
                            && error.seconds >= 0.0
                            && error.seconds <= elapsed
                            && last_boundary.is_none_or(|seconds| error.seconds > seconds)
                    })
                    .count(),
            );
        }
        let peak = wpm
            .iter()
            .chain(&raw)
            .chain(&burst)
            .map(|point| point.1)
            .fold(0.0_f64, f64::max);
        let step = if peak <= 100.0 {
            20.0
        } else {
            10.0_f64.powf(peak.log10().floor()) / 2.0
        };
        let maximum_wpm = ((peak / step).ceil().max(1.0) * step).min(f64::MAX);
        let maximum_errors = error_counts.iter().copied().max().unwrap_or(0).max(1);
        let mut corrected_counts = vec![0; wpm.len()];
        for error in input.errors.iter().filter(|error| {
            error.is_corrected
                && error.seconds.is_finite()
                && error.seconds >= 0.0
                && error.seconds <= elapsed
        }) {
            if wpm.is_empty() {
                break;
            }
            let index = wpm
                .partition_point(|point| point.0 < error.seconds)
                .min(wpm.len() - 1);
            corrected_counts[index] += 1;
        }
        let mut error_points = vec![];
        let mut uncorrected_errors = vec![];
        for ((point, count), corrected_count) in wpm.iter().zip(error_counts).zip(corrected_counts)
        {
            if count == 0 {
                continue;
            }
            let error_point = (point.0, count as f64 / maximum_errors as f64 * maximum_wpm);
            error_points.push(error_point);
            if count != corrected_count {
                uncorrected_errors.push(error_point);
            }
        }
        let end_seconds = wpm.iter().map(|point| point.0).fold(elapsed, f64::max);
        Self {
            start_seconds: wpm.first().map_or(0.0, |point| point.0),
            wpm,
            raw,
            burst,
            error_points,
            uncorrected_errors,
            end_seconds: if end_seconds > 0.0 { end_seconds } else { 1.0 },
            maximum_wpm,
            maximum_errors,
        }
    }

    fn build_traces(&self, theme: &Theme) -> Vec<PlotTrace> {
        ChartMetric::ALL
            .into_iter()
            .map(|metric| {
                let points = match metric {
                    ChartMetric::Wpm => &self.wpm,
                    ChartMetric::Raw => &self.raw,
                    ChartMetric::Burst => &self.burst,
                    ChartMetric::Errors => &self.error_points,
                };
                PlotTrace {
                    metric,
                    points: points
                        .iter()
                        .map(|point| {
                            let is_uncorrected_error = metric == ChartMetric::Errors
                                && self.uncorrected_errors.contains(point);
                            PlotPoint {
                                seconds: point.0,
                                value: point.1,
                                is_uncorrected_error,
                                color: match metric {
                                    ChartMetric::Wpm | ChartMetric::Raw => theme.accent,
                                    ChartMetric::Burst => theme.dim,
                                    ChartMetric::Errors if is_uncorrected_error => theme.error,
                                    ChartMetric::Errors => theme.success,
                                },
                            }
                        })
                        .collect(),
                }
            })
            .collect()
    }

    fn smooth_bursts(samples: &[&Sample]) -> Vec<(f64, f64)> {
        let tolerance = samples
            .iter()
            .map(|sample| sample.burst_wpm)
            .fold(0.0_f64, f64::max)
            * 0.25;
        samples
            .iter()
            .enumerate()
            .map(|(index, sample)| {
                let neighbors = &samples[index.saturating_sub(1)..(index + 2).min(samples.len())];
                let (sum, count) = neighbors
                    .iter()
                    .filter(|neighbor| (neighbor.burst_wpm - sample.burst_wpm).abs() <= tolerance)
                    .fold((0.0, 0), |(sum, count), neighbor| {
                        (sum + neighbor.burst_wpm, count + 1)
                    });
                (
                    sample.seconds,
                    (sum / f64::from(count) * 100.0).round() / 100.0,
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::statistics::CharacterCounts;
    use ratatui::{Terminal, backend::TestBackend};

    fn sample(seconds: f64, wpm: f64) -> Sample {
        Sample {
            seconds,
            wpm,
            raw_wpm: wpm,
            burst_wpm: wpm,
            errors: 0,
            counts: CharacterCounts {
                correct_word: 0,
                raw: 0,
                insertions: 0,
                errors: 0,
            },
        }
    }

    #[test]
    fn burst_smoothing_averages_nearby_rates_without_flattening_spikes() {
        for (rates, expected) in [
            (vec![], vec![]),
            (vec![60.0], vec![60.0]),
            (vec![0.0, 0.0], vec![0.0, 0.0]),
            (vec![40.0, 50.0, 60.0], vec![45.0, 50.0, 55.0]),
            (vec![40.0, 41.0, 43.0], vec![40.5, 41.33, 42.0]),
            (vec![60.0, 120.0, 60.0], vec![60.0, 120.0, 60.0]),
            (vec![30.0, 40.0], vec![35.0, 35.0]),
            (vec![29.0, 40.0], vec![29.0, 40.0]),
        ] {
            let samples: Vec<_> = rates
                .iter()
                .enumerate()
                .map(|(index, rate)| sample((index + 1) as f64, *rate))
                .collect();
            let references: Vec<_> = samples.iter().collect();
            let expected: Vec<_> = expected
                .into_iter()
                .enumerate()
                .map(|(index, rate)| ((index + 1) as f64, rate))
                .collect();
            assert_eq!(
                ChartSeries::smooth_bursts(&references),
                expected,
                "{rates:?}"
            );
        }
    }

    #[test]
    fn shared_scales_and_error_corrections_are_independent_of_renderer() {
        let samples = [
            Sample {
                raw_wpm: 60.0,
                burst_wpm: 140.0,
                errors: 1,
                ..sample(1.0, 40.0)
            },
            Sample {
                errors: 2,
                ..sample(2.0, 50.0)
            },
        ];
        let series = ChartSeries::collect(ChartDataInput {
            samples: &samples,
            errors: &[
                TypingError {
                    seconds: 0.2,
                    is_corrected: true,
                },
                TypingError {
                    seconds: 1.2,
                    is_corrected: true,
                },
                TypingError {
                    seconds: 1.8,
                    is_corrected: false,
                },
            ],
            elapsed: Duration::from_secs(2),
            final_wpm: 50.0,
            final_raw_wpm: 50.0,
        });
        assert_eq!(series.wpm, [(1.0, 40.0), (2.0, 50.0)]);
        assert_eq!(series.raw, [(1.0, 60.0), (2.0, 50.0)]);
        assert_eq!((series.maximum_wpm, series.maximum_errors), (150.0, 2));
        assert_eq!(series.error_points, [(1.0, 75.0), (2.0, 150.0)]);
        assert_eq!(series.uncorrected_errors, [(2.0, 150.0)]);
    }

    #[test]
    fn fractional_completion_keeps_rates_and_errors_without_inventing_burst_samples() {
        for (millis, samples) in [(400, vec![]), (1400, vec![sample(1.0, 24.0)])] {
            let elapsed = Duration::from_millis(millis);
            let series = ChartSeries::collect(ChartDataInput {
                samples: &samples,
                errors: &[TypingError {
                    seconds: elapsed.as_secs_f64() - 0.1,
                    is_corrected: false,
                }],
                elapsed,
                final_wpm: 12.0,
                final_raw_wpm: 24.0,
            });
            assert_eq!(series.wpm.last(), Some(&(elapsed.as_secs_f64(), 12.0)));
            assert_eq!(series.raw.last(), Some(&(elapsed.as_secs_f64(), 24.0)));
            assert_eq!(
                series.error_points,
                [(elapsed.as_secs_f64(), series.maximum_wpm)]
            );
            assert_eq!(series.burst.len(), samples.len());
        }
    }

    #[test]
    fn time_labels_begin_at_the_first_real_sample_and_keep_the_actual_finish_time() {
        use crate::chart_layout::{ChartLayout, LayoutInput};
        let font = ChartFont::create();
        for (seconds, endpoints) in [
            (vec![1.0, 15.0], ["1", "15"]),
            (vec![2.0, 15.0], ["2", "15"]),
            (vec![0.4], ["0.4", "0.4"]),
        ] {
            let samples: Vec<_> = seconds
                .iter()
                .map(|seconds| sample(*seconds, 60.0))
                .collect();
            let elapsed = Duration::from_secs_f64(*seconds.last().unwrap());
            let series = ChartSeries::collect(ChartDataInput {
                samples: &samples,
                errors: &[],
                elapsed,
                final_wpm: 60.0,
                final_raw_wpm: 60.0,
            });
            assert_eq!(series.start_seconds, seconds[0]);
            assert_eq!(series.end_seconds, elapsed.as_secs_f64());
            assert_eq!(
                series.wpm.iter().map(|point| point.0).collect::<Vec<_>>(),
                seconds
            );
            let layout = ChartLayout::create(LayoutInput {
                area: Rect::new(0, 0, 60, 8),
                cell: CellPixels {
                    width: 10,
                    height: 20,
                },
                font: &font,
                start_seconds: series.start_seconds,
                end_seconds: series.end_seconds,
                maximum_speed: series.maximum_wpm,
                maximum_errors: series.maximum_errors,
                is_legend_visible: false,
            })
            .unwrap();
            let labels: Vec<_> = layout
                .labels
                .iter()
                .filter(|label| label.center.y > layout.plot.bottom())
                .collect();
            assert_eq!(labels.first().unwrap().text, endpoints[0]);
            assert_eq!(labels.last().unwrap().text, endpoints[1]);
            assert_eq!(labels.first().unwrap().center.x, layout.plot.left());
            if seconds.len() > 1 {
                assert_eq!(labels.last().unwrap().center.x, layout.plot.right());
            }
        }
    }

    #[test]
    fn axes_labels_and_legend_are_entirely_pixels() {
        let theme = Theme::load_bundled().unwrap().remove(0);
        let font = ChartFont::create();
        let samples = [sample(1.0, 60.0), sample(15.0, 60.0)];
        let mut terminal = Terminal::new(TestBackend::new(60, 12)).unwrap();
        let mut image = None;
        terminal
            .draw(|frame| {
                image = ResultChart {
                    samples: &samples,
                    errors: &[],
                    elapsed: Duration::from_secs(15),
                    final_wpm: 60.0,
                    final_raw_wpm: 60.0,
                    theme: &theme,
                    font: &font,
                    is_legend_visible: true,
                }
                .render_image(ChartPreview {
                    area: frame.area(),
                    cell_pixels: Some(CellPixels {
                        width: 10,
                        height: 20,
                    }),
                });
            })
            .unwrap();
        let image = image.unwrap();
        assert_eq!(image.area.x, 0);
        assert_eq!(image.area.y, 0);
        assert_eq!(image.width, 600);
        assert_eq!(image.visible_height, image.height);
        assert!(image.height > 160);
        assert!(
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .all(|cell| cell.symbol() == " ")
        );
        for (left, top, width, height) in [
            (4, 12, 45, 124),
            (556, 12, 40, 124),
            (48, 138, 510, 17),
            (0, 162, 600, image.height - 162),
        ] {
            let colored = (top..top + height)
                .flat_map(|y| (left..left + width).map(move |x| (y * image.width + x) as usize * 4))
                .filter(|offset| image.rgba[*offset + 3] > 0)
                .count();
            assert!(
                colored > 20,
                "missing rasterized axis/legend text in {left},{top}"
            );
        }
    }

    #[test]
    fn unsupported_outputs_draw_nothing_and_supported_outputs_keep_axes_fixed() {
        let theme = Theme::load_bundled().unwrap().remove(0);
        let font = ChartFont::create();
        let samples = [sample(1.0, 40.0), sample(2.0, 60.0)];
        for width in [24, 36, 60, 120] {
            let mut buffers = vec![];
            for cell_pixels in [
                None,
                Some(CellPixels {
                    width: 10,
                    height: 20,
                }),
                Some(CellPixels {
                    width: 20,
                    height: 40,
                }),
            ] {
                let mut terminal = Terminal::new(TestBackend::new(width, 8)).unwrap();
                let mut image = None;
                terminal
                    .draw(|frame| {
                        image = ResultChart {
                            samples: &samples,
                            errors: &[],
                            elapsed: Duration::from_secs(2),
                            final_wpm: 60.0,
                            final_raw_wpm: 60.0,
                            theme: &theme,
                            font: &font,
                            is_legend_visible: false,
                        }
                        .render_image(ChartPreview {
                            area: frame.area(),
                            cell_pixels,
                        });
                    })
                    .unwrap();
                assert_eq!(image.is_some(), cell_pixels.is_some());
                let buffer = terminal.backend().buffer();
                assert!(buffer.content.iter().all(|cell| cell.symbol() == " "));
                if let Some(image) = image {
                    assert_eq!(image.area, Rect::new(0, 0, width.min(60), 8));
                    assert!(image.visible_height < image.height);
                    buffers.push(buffer.clone());
                }
            }
            for buffer in &buffers[1..] {
                for y in [6, 7] {
                    for x in 0..width {
                        assert_eq!(buffers[0][(x, y)], buffer[(x, y)]);
                    }
                }
            }
        }
    }

    #[test]
    fn repeated_rendering_keeps_axes_fixed_without_any_text_dots() {
        let theme = Theme::load_bundled().unwrap().remove(0);
        let font = ChartFont::create();
        let samples = [
            Sample {
                raw_wpm: 60.0,
                burst_wpm: 140.0,
                errors: 1,
                ..sample(1.0, 20.0)
            },
            Sample {
                raw_wpm: 70.0,
                burst_wpm: 60.0,
                errors: 2,
                ..sample(2.0, 40.0)
            },
        ];
        for width in [24, 36, 60] {
            let mut buffers = vec![];
            for _ in 0..2 {
                let mut terminal = Terminal::new(TestBackend::new(width, 8)).unwrap();
                terminal
                    .draw(|frame| {
                        ResultChart {
                            samples: &samples,
                            errors: &[],
                            elapsed: Duration::from_secs(2),
                            final_wpm: 40.0,
                            final_raw_wpm: 70.0,
                            theme: &theme,
                            font: &font,
                            is_legend_visible: false,
                        }
                        .render_image(ChartPreview {
                            area: frame.area(),
                            cell_pixels: Some(CellPixels {
                                width: 10,
                                height: 20,
                            }),
                        });
                    })
                    .unwrap();
                let buffer = terminal.backend().buffer();
                assert!(buffer.content.iter().all(|cell| cell.symbol() != "·"));
                buffers.push(buffer.clone());
            }
            for buffer in &buffers[1..] {
                for y in 1..8 {
                    for x in 0..width {
                        assert_eq!(buffers[0][(x, y)].symbol(), buffer[(x, y)].symbol());
                    }
                }
            }
        }
    }

    #[test]
    fn tiny_areas_and_large_counts_are_safe_with_or_without_graphics() {
        let theme = Theme::load_bundled().unwrap().remove(0);
        let font = ChartFont::create();
        for width in [0, 1, 13, 14, 24, 72, 120] {
            for height in 0..=10 {
                for cell_pixels in [
                    None,
                    Some(CellPixels {
                        width: 10,
                        height: 20,
                    }),
                ] {
                    for samples in [
                        vec![],
                        vec![Sample {
                            errors: usize::MAX,
                            ..sample(1.0, 1_000_000.0)
                        }],
                    ] {
                        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                        terminal
                            .draw(|frame| {
                                ResultChart {
                                    samples: &samples,
                                    errors: &[],
                                    elapsed: Duration::from_secs(u64::from(!samples.is_empty())),
                                    final_wpm: 0.0,
                                    final_raw_wpm: 0.0,
                                    theme: &theme,
                                    font: &font,
                                    is_legend_visible: false,
                                }
                                .render_image(ChartPreview {
                                    area: frame.area(),
                                    cell_pixels,
                                });
                            })
                            .unwrap();
                    }
                }
            }
        }
    }
}
