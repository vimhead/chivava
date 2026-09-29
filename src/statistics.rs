use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct Sample {
    pub seconds: f64,
    pub wpm: f64,
    pub raw_wpm: f64,
    pub burst_wpm: f64,
    pub errors: usize,
    pub counts: CharacterCounts,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChartMetric {
    Wpm,
    Raw,
    Burst,
    Errors,
}

impl ChartMetric {
    pub const ALL: [Self; 4] = [Self::Wpm, Self::Raw, Self::Burst, Self::Errors];
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct TypingError {
    pub seconds: f64,
    pub is_corrected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct CharacterCounts {
    pub correct_word: usize,
    pub raw: usize,
    pub insertions: usize,
    pub errors: usize,
}

impl CharacterCounts {
    const ZERO: Self = Self {
        correct_word: 0,
        raw: 0,
        insertions: 0,
        errors: 0,
    };

    pub fn count_correct_words(target: &[char], typed: &[char]) -> usize {
        target
            .split_inclusive(|character| matches!(character, ' ' | '\n'))
            .zip(typed.split_inclusive(|character| matches!(character, ' ' | '\n')))
            .filter(|(word, input)| word.starts_with(input))
            .map(|(_, input)| {
                input
                    .iter()
                    .map(|character| character.len_utf16())
                    .sum::<usize>()
            })
            .sum()
    }

    pub fn calculate_final_rate(characters: usize, elapsed: Duration) -> f64 {
        let seconds = (elapsed.as_secs_f64() * 100.0).round() / 100.0;
        if seconds == 0.0 {
            0.0
        } else {
            ((characters as f64 / 5.0) / (seconds / 60.0) * 100.0).round() / 100.0
        }
    }

    pub fn calculate_rate(characters: usize, elapsed: Duration) -> f64 {
        if elapsed.is_zero() {
            0.0
        } else {
            (characters as f64 / 5.0) / (elapsed.as_secs_f64() / 60.0)
        }
    }
}

pub struct SamplingInput {
    pub elapsed: Duration,
    pub counts: CharacterCounts,
    pub is_finished: bool,
    pub is_timed: bool,
}

pub struct SpeedSampling {
    observed: CharacterCounts,
    last_boundary: CharacterCounts,
    previous_boundary: CharacterCounts,
    next_second: u64,
}

impl SpeedSampling {
    pub fn create() -> Self {
        Self {
            observed: CharacterCounts::ZERO,
            last_boundary: CharacterCounts::ZERO,
            previous_boundary: CharacterCounts::ZERO,
            next_second: 1,
        }
    }

    pub fn record(&mut self, input: SamplingInput, samples: &mut Vec<Sample>) {
        while Duration::from_secs(self.next_second) < input.elapsed {
            self.append_second(self.observed, samples);
        }
        self.observed = input.counts;
        if Duration::from_secs(self.next_second) == input.elapsed {
            self.append_second(input.counts, samples);
        } else if let Some(last) = samples.last_mut()
            && last.seconds == input.elapsed.as_secs_f64()
        {
            *last = Self::build_sample(SampleInput {
                elapsed: input.elapsed,
                interval: Duration::from_secs(1),
                counts: input.counts,
                previous: self.previous_boundary,
            });
            self.last_boundary = input.counts;
        }
        let rounded_seconds = (input.elapsed.as_secs_f64() * 100.0).round() / 100.0;
        if input.is_finished && !input.is_timed && rounded_seconds.fract() >= 0.5 {
            samples.push(Self::build_sample(SampleInput {
                elapsed: input.elapsed,
                interval: input.elapsed - Duration::from_secs(self.next_second - 1),
                counts: input.counts,
                previous: self.last_boundary,
            }));
        }
    }

    fn append_second(&mut self, counts: CharacterCounts, samples: &mut Vec<Sample>) {
        samples.push(Self::build_sample(SampleInput {
            elapsed: Duration::from_secs(self.next_second),
            interval: Duration::from_secs(1),
            counts,
            previous: self.last_boundary,
        }));
        self.previous_boundary = self.last_boundary;
        self.last_boundary = counts;
        self.next_second += 1;
    }

    fn build_sample(input: SampleInput) -> Sample {
        Sample {
            seconds: input.elapsed.as_secs_f64(),
            wpm: CharacterCounts::calculate_rate(input.counts.correct_word, input.elapsed).round(),
            raw_wpm: CharacterCounts::calculate_rate(input.counts.raw, input.elapsed).round(),
            burst_wpm: CharacterCounts::calculate_rate(
                input
                    .counts
                    .insertions
                    .saturating_sub(input.previous.insertions),
                input.interval,
            )
            .round(),
            errors: input.counts.errors.saturating_sub(input.previous.errors),
            counts: input.counts,
        }
    }
}

struct SampleInput {
    elapsed: Duration,
    interval: Duration,
    counts: CharacterCounts,
    previous: CharacterCounts,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_credit_includes_separators_and_correct_unfinished_prefixes() {
        for (target, typed, credit) in [
            ("hello world", "hello wor", 9),
            ("hello world", "hxllo wor", 3),
            ("hello world", "hello wxr", 6),
            ("hello world", "helloXwor", 0),
            ("hello world rest", "helo world ", 6),
            ("hello world rest", "helloo world ", 6),
            ("hello world", "h", 1),
            ("hello world", "hx", 0),
            ("hello world", "", 0),
            ("call();\nnext()", "call();\nne", 10),
            ("call();\nnext()", "caXl();\nne", 2),
            ("a\tb c", "a\tx c", 1),
            ("été 界 😀", "été 界 😀", 8),
        ] {
            assert_eq!(
                CharacterCounts::count_correct_words(
                    &target.chars().collect::<Vec<_>>(),
                    &typed.chars().collect::<Vec<_>>()
                ),
                credit,
                "{target:?} / {typed:?}"
            );
        }
    }

    fn observe(sampling: &mut SpeedSampling, samples: &mut Vec<Sample>, millis: u64, count: usize) {
        sampling.record(
            SamplingInput {
                elapsed: Duration::from_millis(millis),
                counts: CharacterCounts {
                    correct_word: count,
                    raw: count,
                    insertions: count,
                    errors: 0,
                },
                is_finished: false,
                is_timed: false,
            },
            samples,
        );
    }

    #[test]
    fn delayed_observations_fill_exact_seconds_without_using_future_input() {
        let mut sampling = SpeedSampling::create();
        let mut samples = vec![];
        observe(&mut sampling, &mut samples, 0, 1);
        observe(&mut sampling, &mut samples, 900, 5);
        observe(&mut sampling, &mut samples, 3400, 10);
        assert_eq!(
            samples
                .iter()
                .map(|s| (s.seconds, s.wpm, s.burst_wpm))
                .collect::<Vec<_>>(),
            [(1.0, 60.0, 60.0), (2.0, 30.0, 0.0), (3.0, 20.0, 0.0)]
        );
        observe(&mut sampling, &mut samples, 4000, 10);
        assert_eq!((samples[3].wpm, samples[3].burst_wpm), (30.0, 60.0));
    }

    #[test]
    fn events_exactly_on_a_boundary_update_that_bucket_without_duplicates() {
        let mut sampling = SpeedSampling::create();
        let mut samples = vec![];
        observe(&mut sampling, &mut samples, 0, 1);
        observe(&mut sampling, &mut samples, 1000, 4);
        observe(&mut sampling, &mut samples, 1000, 5);
        observe(&mut sampling, &mut samples, 1000, 6);
        assert_eq!(samples.len(), 1);
        assert_eq!((samples[0].wpm, samples[0].burst_wpm), (72.0, 72.0));
        observe(&mut sampling, &mut samples, 2000, 10);
        assert_eq!((samples[1].wpm, samples[1].burst_wpm), (60.0, 48.0));
    }

    #[test]
    fn fractional_tail_matches_the_half_second_rule_for_non_timed_tests() {
        for is_timed in [false, true] {
            for (millis, untimed_length) in
                [(0, 0), (400, 0), (600, 1), (1000, 1), (1400, 1), (1600, 2)]
            {
                let mut sampling = SpeedSampling::create();
                let mut samples = vec![];
                observe(&mut sampling, &mut samples, 0, 1);
                sampling.record(
                    SamplingInput {
                        elapsed: Duration::from_millis(millis),
                        counts: CharacterCounts {
                            correct_word: 2,
                            raw: 3,
                            insertions: 4,
                            errors: 1,
                        },
                        is_finished: true,
                        is_timed,
                    },
                    &mut samples,
                );
                assert_eq!(
                    samples.len(),
                    if is_timed {
                        millis as usize / 1000
                    } else {
                        untimed_length
                    }
                );
                if millis == 600 && !is_timed {
                    assert_eq!(
                        (
                            samples[0].wpm,
                            samples[0].raw_wpm,
                            samples[0].burst_wpm,
                            samples[0].errors
                        ),
                        (40.0, 60.0, 80.0, 1)
                    );
                }
            }
        }
    }
}
