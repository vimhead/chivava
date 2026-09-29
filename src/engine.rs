use std::{collections::BTreeMap, ops::Range, time::Duration};

pub use crate::statistics::Sample;
use crate::statistics::{CharacterCounts, SamplingInput, SpeedSampling, TypingError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditMode {
    Insert,
    Normal,
}

#[derive(Clone)]
struct Revision {
    text: Vec<char>,
    cursor: usize,
    error_positions: BTreeMap<usize, usize>,
}

pub struct TypingSession {
    pub target: Vec<char>,
    pub source_byte_offsets: Vec<usize>,
    pub line_indentation: Option<BTreeMap<usize, usize>>,
    pub typed: Vec<char>,
    pub cursor: usize,
    pub mode: EditMode,
    pub pending: String,
    pub attempts: usize,
    pub correct_attempts: usize,
    pub samples: Vec<Sample>,
    pub errors: Vec<TypingError>,
    error_positions: BTreeMap<usize, usize>,
    pub duration: Option<Duration>,
    pub started_at: Option<Duration>,
    pub ended_at: Option<Duration>,
    sampling: SpeedSampling,
    undo: Vec<Revision>,
    redo: Vec<Revision>,
}

impl TypingSession {
    pub fn create(target: &str, duration: Option<Duration>) -> Self {
        Self {
            target: target.chars().collect(),
            source_byte_offsets: target.char_indices().map(|(offset, _)| offset).collect(),
            line_indentation: None,
            typed: vec![],
            cursor: 0,
            mode: EditMode::Insert,
            pending: String::new(),
            attempts: 0,
            correct_attempts: 0,
            samples: vec![],
            errors: vec![],
            error_positions: BTreeMap::new(),
            duration,
            started_at: None,
            ended_at: None,
            sampling: SpeedSampling::create(),
            undo: vec![],
            redo: vec![],
        }
    }

    pub fn create_code(code: &str, duration: Option<Duration>) -> Self {
        let mut target = String::new();
        let mut source_byte_offsets = vec![];
        let mut line_start = 0;
        let mut line_indentation = BTreeMap::new();
        let mut position = 0;
        for line in code.split_inclusive('\n') {
            let content = line.trim_start_matches([' ', '\t']);
            let prefix = &line[..line.len() - content.len()];
            let columns = prefix.chars().fold(0, |columns, character| {
                columns
                    + if character == '\t' {
                        4 - columns % 4
                    } else {
                        1
                    }
            });
            line_indentation.insert(position, columns);
            target.push_str(content);
            source_byte_offsets.extend(
                content
                    .char_indices()
                    .map(|(offset, _)| line_start + prefix.len() + offset),
            );
            line_start += line.len();
            position += content.chars().count();
        }
        let mut session = Self::create(&target, duration);
        session.line_indentation = Some(line_indentation);
        session.source_byte_offsets = source_byte_offsets;
        session
    }

    pub fn elapsed(&self, now: Duration) -> Duration {
        self.started_at
            .map(|start| {
                let elapsed = self.ended_at.unwrap_or(now).saturating_sub(start);
                self.duration.map_or(elapsed, |limit| elapsed.min(limit))
            })
            .unwrap_or(Duration::ZERO)
    }

    pub fn calculate_wpm(&self, now: Duration) -> f64 {
        CharacterCounts::calculate_final_rate(
            CharacterCounts::count_correct_words(&self.target, &self.typed),
            self.elapsed(now),
        )
    }

    pub fn calculate_raw_wpm(&self, now: Duration) -> f64 {
        CharacterCounts::calculate_final_rate(
            self.typed
                .iter()
                .map(|character| character.len_utf16())
                .sum(),
            self.elapsed(now),
        )
    }

    pub fn calculate_accuracy(&self) -> f64 {
        if self.attempts == 0 {
            return 100.0;
        }
        100.0 * self.correct_attempts as f64 / self.attempts as f64
    }

    #[cfg(test)]
    pub fn count_correct_characters(&self) -> usize {
        self.typed
            .iter()
            .zip(&self.target)
            .filter(|(typed, target)| typed == target)
            .count()
    }

    pub fn tick(&mut self, now: Duration) {
        if self.started_at.is_none() || self.ended_at.is_some() {
            return;
        }
        let elapsed = self.elapsed(now);
        if let Some(limit) = self.duration.filter(|limit| elapsed >= *limit) {
            self.ended_at = Some(self.started_at.unwrap() + limit);
        } else if self.typed.len() >= self.target.len() {
            self.ended_at = Some(now);
        }
        self.sampling.record(
            SamplingInput {
                elapsed,
                counts: CharacterCounts {
                    correct_word: CharacterCounts::count_correct_words(&self.target, &self.typed),
                    raw: self
                        .typed
                        .iter()
                        .map(|character| character.len_utf16())
                        .sum(),
                    insertions: self.attempts,
                    errors: self.attempts - self.correct_attempts,
                },
                is_finished: self.ended_at.is_some(),
                is_timed: self.duration.is_some(),
            },
            &mut self.samples,
        );
    }

    pub fn insert_character(&mut self, character: char, now: Duration) {
        self.tick(now);
        if self.ended_at.is_some() || self.typed.len() >= self.target.len() {
            return;
        }
        self.started_at.get_or_insert(now);
        self.remember_revision();
        self.attempts += 1;
        let following_errors = self.error_positions.split_off(&self.cursor);
        self.error_positions.extend(
            following_errors
                .into_iter()
                .map(|(position, error)| (position + 1, error)),
        );
        if self.target.get(self.cursor) == Some(&character) {
            self.correct_attempts += 1;
        } else {
            self.error_positions.insert(self.cursor, self.errors.len());
            self.errors.push(TypingError {
                seconds: self.elapsed(now).as_secs_f64(),
                is_corrected: false,
            });
        }
        self.typed.insert(self.cursor, character);
        self.cursor += 1;
        self.update_error_corrections();
        self.tick(now);
    }

    pub fn enter_normal_mode(&mut self) {
        if self.mode == EditMode::Insert {
            self.cursor = self.cursor.saturating_sub(1);
        }
        self.mode = EditMode::Normal;
        self.pending.clear();
        self.clamp_cursor();
    }

    pub fn move_cursor(&mut self, direction: isize) {
        self.cursor = self.cursor.saturating_add_signed(direction);
        self.clamp_cursor();
    }

    pub fn delete_previous_character(&mut self) {
        if self.cursor > 0 {
            self.delete_range(self.cursor - 1..self.cursor);
        }
    }

    pub fn delete_previous_word(&mut self) {
        self.delete_range(self.find_previous_word()..self.cursor);
    }

    pub fn handle_normal_character(&mut self, character: char) {
        if self.ended_at.is_some() {
            return;
        }
        if !self.pending.is_empty() {
            self.pending.push(character);
            if matches!(self.pending.as_str(), "ci" | "di" | "ca" | "da") {
                return;
            }
            let command = std::mem::take(&mut self.pending);
            self.execute_operator(&command);
            return;
        }
        match character {
            'i' => self.mode = EditMode::Insert,
            'a' => {
                self.cursor = (self.cursor + 1).min(self.typed.len());
                self.mode = EditMode::Insert;
            }
            'I' => {
                self.cursor = self.find_line_start();
                self.mode = EditMode::Insert;
            }
            'A' => {
                self.cursor = self.find_line_end();
                self.mode = EditMode::Insert;
            }
            'h' => self.move_cursor(-1),
            'l' => self.move_cursor(1),
            'w' => {
                self.cursor = self.find_next_word();
                self.clamp_cursor();
            }
            'b' => self.cursor = self.find_previous_word(),
            'e' => {
                if self.find_word_end().saturating_sub(1) == self.cursor {
                    self.cursor = (self.cursor + 1).min(self.typed.len());
                }
                self.cursor = self.find_word_end().saturating_sub(1);
                self.clamp_cursor();
            }
            '0' => self.cursor = self.find_line_start(),
            '$' => {
                self.cursor = self
                    .find_line_end()
                    .saturating_sub(1)
                    .max(self.find_line_start());
                self.clamp_cursor();
            }
            'x' => self.delete_range(self.cursor..(self.cursor + 1).min(self.typed.len())),
            'X' => self.delete_previous_character(),
            'D' | 'C' => {
                let insertion_cursor = self.cursor;
                self.delete_range(self.cursor..self.find_line_end());
                if character == 'C' {
                    self.mode = EditMode::Insert;
                    self.cursor = insertion_cursor.min(self.typed.len());
                }
            }
            'c' | 'd' => self.pending.push(character),
            'u' => self.undo_edit(),
            _ => {}
        }
    }

    fn execute_operator(&mut self, command: &str) {
        let range = match command {
            "ciw" | "diw" | "caw" | "daw" => {
                let mut range = self.find_inner_word();
                if command.ends_with("aw") {
                    let end = range.end;
                    while range.end < self.typed.len() && self.typed[range.end].is_whitespace() {
                        range.end += 1;
                    }
                    if end == range.end {
                        while range.start > 0 && self.typed[range.start - 1].is_whitespace() {
                            range.start -= 1;
                        }
                    }
                }
                range
            }
            "dw" => self.cursor..self.find_next_word(),
            "cw" => {
                self.cursor
                    ..if self
                        .typed
                        .get(self.cursor)
                        .is_some_and(|character| character.is_whitespace())
                    {
                        self.find_next_word()
                    } else {
                        self.find_word_end()
                    }
            }
            "db" | "cb" => self.find_previous_word()..self.cursor,
            "dd" => self.find_line_start()..(self.find_line_end() + 1).min(self.typed.len()),
            "cc" => self.find_line_start()..self.find_line_end(),
            _ => return,
        };
        let insertion_cursor = range.start;
        self.delete_range(range);
        if command.starts_with('c') {
            self.mode = EditMode::Insert;
            self.cursor = insertion_cursor.min(self.typed.len());
        }
    }

    fn classify_character(character: char) -> u8 {
        if character.is_whitespace() {
            0
        } else if character.is_alphanumeric() || character == '_' {
            1
        } else {
            2
        }
    }

    fn find_inner_word(&self) -> Range<usize> {
        let Some(&character) = self.typed.get(self.cursor) else {
            return self.cursor..self.cursor;
        };
        let class = Self::classify_character(character);
        let mut start = self.cursor;
        let mut end = self.cursor + 1;
        while start > 0 && Self::classify_character(self.typed[start - 1]) == class {
            start -= 1;
        }
        while end < self.typed.len() && Self::classify_character(self.typed[end]) == class {
            end += 1;
        }
        start..end
    }

    fn find_next_word(&self) -> usize {
        let mut end = self.find_inner_word().end;
        while end < self.typed.len() && self.typed[end].is_whitespace() {
            end += 1;
        }
        end
    }

    fn find_word_end(&self) -> usize {
        let mut start = self.cursor;
        while start < self.typed.len() && self.typed[start].is_whitespace() {
            start += 1;
        }
        let Some(&character) = self.typed.get(start) else {
            return start;
        };
        let class = Self::classify_character(character);
        let mut end = start + 1;
        while end < self.typed.len() && Self::classify_character(self.typed[end]) == class {
            end += 1;
        }
        end
    }

    fn find_previous_word(&self) -> usize {
        let mut start = self.cursor.saturating_sub(1);
        while start > 0 && self.typed[start].is_whitespace() {
            start -= 1;
        }
        let Some(&character) = self.typed.get(start) else {
            return 0;
        };
        let class = Self::classify_character(character);
        while start > 0 && Self::classify_character(self.typed[start - 1]) == class {
            start -= 1;
        }
        start
    }

    fn find_line_start(&self) -> usize {
        self.typed[..self.cursor]
            .iter()
            .rposition(|character| *character == '\n')
            .map(|index| index + 1)
            .unwrap_or(0)
    }

    fn find_line_end(&self) -> usize {
        self.typed[self.cursor..]
            .iter()
            .position(|character| *character == '\n')
            .map(|index| self.cursor + index)
            .unwrap_or(self.typed.len())
    }

    fn remember_revision(&mut self) {
        if self.undo.len() >= 2048 {
            self.undo.remove(0);
        }
        self.undo.push(Revision {
            text: self.typed.clone(),
            cursor: self.cursor,
            error_positions: self.error_positions.clone(),
        });
        self.redo.clear();
    }

    fn delete_range(&mut self, range: Range<usize>) {
        if range.is_empty() || self.ended_at.is_some() {
            return;
        }
        self.remember_revision();
        self.cursor = range.start;
        let following_errors = self.error_positions.split_off(&range.end);
        self.error_positions
            .retain(|position, _| *position < range.start);
        self.error_positions.extend(
            following_errors
                .into_iter()
                .map(|(position, error)| (position - range.len(), error)),
        );
        self.typed.drain(range);
        self.update_error_corrections();
        self.clamp_cursor();
    }

    fn update_error_corrections(&mut self) {
        for error in &mut self.errors {
            error.is_corrected = true;
        }
        for (&position, &error) in &self.error_positions {
            self.errors[error].is_corrected = self.typed.get(position) == self.target.get(position);
        }
    }

    fn clamp_cursor(&mut self) {
        let maximum = if self.mode == EditMode::Insert {
            self.typed.len()
        } else {
            self.typed.len().saturating_sub(1)
        };
        self.cursor = self.cursor.min(maximum);
    }

    pub fn undo_edit(&mut self) {
        if self.ended_at.is_some() {
            return;
        }
        if let Some(revision) = self.undo.pop() {
            self.redo.push(Revision {
                text: self.typed.clone(),
                cursor: self.cursor,
                error_positions: self.error_positions.clone(),
            });
            self.typed = revision.text;
            self.cursor = revision.cursor;
            self.error_positions = revision.error_positions;
            self.update_error_corrections();
            self.clamp_cursor();
        }
    }

    pub fn redo_edit(&mut self) {
        if self.ended_at.is_some() {
            return;
        }
        if let Some(revision) = self.redo.pop() {
            self.undo.push(Revision {
                text: self.typed.clone(),
                cursor: self.cursor,
                error_positions: self.error_positions.clone(),
            });
            self.typed = revision.text;
            self.cursor = revision.cursor;
            self.error_positions = revision.error_positions;
            self.update_error_corrections();
            self.clamp_cursor();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correction_marks_the_original_mistake_without_erasing_accuracy_history() {
        let mut session = TypingSession::create("hello world", None);
        session.insert_character('h', Duration::ZERO);
        session.insert_character('x', Duration::from_millis(200));
        assert_eq!(
            session.errors,
            [TypingError {
                seconds: 0.2,
                is_corrected: false
            }]
        );
        session.delete_previous_character();
        assert!(session.errors[0].is_corrected);
        session.insert_character('e', Duration::from_millis(1400));
        assert_eq!(session.errors[0].seconds, 0.2);
        assert!(session.errors[0].is_corrected);
        assert_eq!((session.attempts, session.correct_attempts), (3, 2));
        session.undo_edit();
        assert!(session.errors[0].is_corrected);
        session.undo_edit();
        assert!(!session.errors[0].is_corrected);
        session.redo_edit();
        assert!(session.errors[0].is_corrected);
        session.redo_edit();
        assert!(session.errors[0].is_corrected);
        assert_eq!((session.attempts, session.correct_attempts), (3, 2));
    }

    #[test]
    fn error_identities_follow_position_changes_and_restore_through_undo() {
        let mut session = TypingSession::create("abcdef", None);
        session.insert_character('x', Duration::ZERO);
        session.insert_character('b', Duration::from_millis(100));
        session.insert_character('y', Duration::from_millis(200));
        session.delete_range(0..1);
        assert!(session.errors[0].is_corrected);
        assert!(!session.errors[1].is_corrected);
        assert_eq!(session.error_positions, BTreeMap::from([(1, 1)]));
        session.undo_edit();
        assert!(session.errors.iter().all(|error| !error.is_corrected));
        assert_eq!(session.error_positions, BTreeMap::from([(0, 0), (2, 1)]));
        session.redo_edit();
        session.insert_character('a', Duration::from_secs(1));
        assert_eq!(session.error_positions, BTreeMap::from([(2, 1)]));
        assert!(!session.errors[1].is_corrected);
        session.delete_range(2..3);
        assert!(session.errors.iter().all(|error| error.is_corrected));
    }

    #[test]
    fn inserting_missing_text_can_correct_a_displaced_character() {
        let mut session = TypingSession::create("abc", None);
        session.insert_character('b', Duration::ZERO);
        session.move_cursor(-1);
        session.insert_character('a', Duration::from_secs(1));
        assert!(session.errors[0].is_corrected);
        session.undo_edit();
        assert!(!session.errors[0].is_corrected);
        session.redo_edit();
        assert!(session.errors[0].is_corrected);
    }

    #[test]
    fn vim_word_corrections_restore_mistake_identity_through_undo_and_redo() {
        let mut session = TypingSession::create("hello world later", None);
        for (index, character) in "hxllo world".chars().enumerate() {
            session.insert_character(character, Duration::from_millis(index as u64 * 100));
        }
        session.enter_normal_mode();
        for character in "0ciw".chars() {
            session.handle_normal_character(character);
        }
        assert!(session.errors[0].is_corrected);
        for character in "hello".chars() {
            session.insert_character(character, Duration::from_secs(2));
        }
        assert_eq!(session.typed.iter().collect::<String>(), "hello world");
        assert_eq!(session.errors.len(), 1);
        for _ in 0..6 {
            session.undo_edit();
        }
        assert_eq!(session.typed.iter().collect::<String>(), "hxllo world");
        assert!(!session.errors[0].is_corrected);
        for _ in 0..6 {
            session.redo_edit();
        }
        assert!(session.errors[0].is_corrected);
        assert_eq!(session.errors[0].seconds, 0.1);
    }

    #[test]
    fn new_edits_after_undo_do_not_reuse_old_mistake_identities() {
        let mut session = TypingSession::create("abc", None);
        session.insert_character('x', Duration::ZERO);
        session.undo_edit();
        session.insert_character('y', Duration::from_secs(1));
        assert_eq!(
            session.errors,
            [
                TypingError {
                    seconds: 0.0,
                    is_corrected: true
                },
                TypingError {
                    seconds: 1.0,
                    is_corrected: false
                },
            ]
        );
        session.redo_edit();
        assert!(!session.errors[1].is_corrected);
        assert_eq!(
            session.errors.len(),
            session.attempts - session.correct_attempts
        );
    }

    #[test]
    fn sampling_is_independent_of_tick_frequency_during_corrections_and_idle_time() {
        let finished_at = Duration::from_secs(5);
        let mut sampled_sessions = vec![];
        for tick_interval in [None, Some(33), Some(250)] {
            let mut session = TypingSession::create("hello world", Some(finished_at));
            let mut next_tick = tick_interval.unwrap_or(0);
            for (milliseconds, character) in [
                (0, Some('h')),
                (200, Some('e')),
                (400, Some('x')),
                (600, None),
                (800, Some('l')),
                (1000, Some('l')),
                (1200, Some('o')),
                (1400, Some(' ')),
                (3600, Some('w')),
            ] {
                if let Some(interval) = tick_interval {
                    while next_tick < milliseconds {
                        session.tick(Duration::from_millis(next_tick));
                        next_tick += interval;
                    }
                }
                let now = Duration::from_millis(milliseconds);
                session.tick(now);
                if let Some(character) = character {
                    session.insert_character(character, now);
                } else {
                    session.delete_previous_character();
                }
                session.tick(now);
            }
            session.tick(finished_at);
            assert_eq!(session.calculate_wpm(finished_at), 16.8);
            assert_eq!(session.calculate_raw_wpm(finished_at), 16.8);
            assert_eq!(session.calculate_accuracy(), 87.5);
            assert_eq!(
                session.errors,
                [TypingError {
                    seconds: 0.4,
                    is_corrected: true
                }]
            );
            assert_eq!(session.samples.len(), 5);
            sampled_sessions.push(session.samples);
        }
        for pair in sampled_sessions.windows(2) {
            assert_eq!(pair[0], pair[1]);
        }
    }

    #[test]
    fn steady_sixty_wpm_matches_timestamped_one_second_buckets() {
        let target = "word ".repeat(100);
        for mut session in [
            TypingSession::create(&target, Some(Duration::from_secs(60))),
            TypingSession::create_code(&format!("    {target}"), Some(Duration::from_secs(60))),
        ] {
            for (index, character) in target.chars().take(300).enumerate() {
                session.insert_character(
                    character,
                    Duration::from_millis(10_000 + index as u64 * 200),
                );
            }
            session.tick(Duration::from_secs(70));
            assert_eq!(session.samples.len(), 60);
            for (index, sample) in session.samples.iter().enumerate() {
                let second = index + 1;
                let characters = (second * 5 + 1).min(300);
                assert_eq!(sample.seconds, second as f64);
                assert_eq!(
                    sample.wpm,
                    ((characters as f64 / 5.0) / (second as f64 / 60.0)).round()
                );
                assert_eq!(sample.raw_wpm, sample.wpm);
                assert!((60.0..=72.0).contains(&sample.wpm));
                assert_eq!(sample.errors, 0);
            }
            assert_eq!(session.calculate_wpm(Duration::from_secs(90)), 60.0);
            assert_eq!(session.calculate_raw_wpm(Duration::from_secs(90)), 60.0);
        }
    }

    #[test]
    fn corrections_change_word_credit_but_do_not_rewrite_past_samples_or_errors() {
        let mut session = TypingSession::create("hello world again", None);
        for (index, character) in "helx".chars().enumerate() {
            session.insert_character(character, Duration::from_millis(index as u64 * 200));
        }
        session.tick(Duration::from_secs(1));
        let first = session.samples[0];
        assert_eq!(
            (first.wpm, first.raw_wpm, first.burst_wpm, first.errors),
            (0.0, 48.0, 48.0, 1)
        );
        session.tick(Duration::from_millis(1200));
        session.delete_previous_character();
        session.tick(Duration::from_millis(1200));
        for (index, character) in "lo ".chars().enumerate() {
            session.insert_character(character, Duration::from_millis(1400 + index as u64 * 200));
        }
        session.insert_character('w', Duration::from_millis(2200));
        session.tick(Duration::from_secs(3));
        assert_eq!(session.samples[0], first);
        assert_eq!(
            (
                session.samples[1].wpm,
                session.samples[1].raw_wpm,
                session.samples[1].burst_wpm
            ),
            (36.0, 36.0, 36.0)
        );
        assert_eq!(
            (
                session.samples[2].wpm,
                session.samples[2].raw_wpm,
                session.samples[2].burst_wpm
            ),
            (28.0, 28.0, 12.0)
        );
        assert_eq!(session.calculate_accuracy(), 87.5);
        session.undo_edit();
        session.tick(Duration::from_millis(3200));
        session.redo_edit();
        session.tick(Duration::from_millis(3400));
        session.tick(Duration::from_secs(4));
        assert_eq!(session.samples[3].burst_wpm, 0.0);
        assert_eq!(session.samples[3].errors, 0);
        assert_eq!(session.calculate_accuracy(), 87.5);
    }

    #[test]
    fn idle_time_reduces_cumulative_speed_but_burst_drops_to_zero() {
        let mut session = TypingSession::create("hello world again", None);
        for (index, character) in "hello ".chars().enumerate() {
            session.insert_character(character, Duration::from_millis(index as u64 * 100));
        }
        session.tick(Duration::from_secs(3));
        assert_eq!(
            session
                .samples
                .iter()
                .map(|s| (s.wpm, s.burst_wpm))
                .collect::<Vec<_>>(),
            [(72.0, 72.0), (36.0, 0.0), (24.0, 0.0)]
        );
    }

    #[test]
    fn source_offsets_preserve_unicode_and_skip_only_automatic_indentation() {
        let source = "  fn café() {\n\tlet 界 = \"é\";\n  \n}\n  ";
        for session in [
            TypingSession::create(source, Some(Duration::from_secs(30))),
            TypingSession::create_code(source, Some(Duration::from_secs(30))),
        ] {
            assert_eq!(session.target.len(), session.source_byte_offsets.len());
            for (&character, &offset) in session.target.iter().zip(&session.source_byte_offsets) {
                assert!(source.is_char_boundary(offset));
                assert_eq!(source[offset..].chars().next(), Some(character));
            }
            assert!(
                session
                    .source_byte_offsets
                    .windows(2)
                    .all(|pair| pair[0] < pair[1])
            );
        }
        let session = TypingSession::create_code(source, Some(Duration::from_secs(30)));
        assert_eq!(session.source_byte_offsets[0], 2);
        let position = session
            .target
            .iter()
            .position(|character| *character == '界')
            .unwrap();
        assert_eq!(
            session.source_byte_offsets[position],
            source.find('界').unwrap()
        );
    }

    #[test]
    fn code_indentation_is_display_only_and_does_not_start_the_timer() {
        let session = TypingSession::create_code(
            " \t界( a);\n\t  next();\n  \n    end",
            Some(Duration::from_secs(30)),
        );
        assert_eq!(
            session.target.iter().collect::<String>(),
            "界( a);\nnext();\n\nend"
        );
        assert_eq!(
            session.line_indentation.unwrap(),
            BTreeMap::from([(0, 4), (7, 6), (15, 2), (16, 4)])
        );
        assert_eq!(session.attempts, 0);
        assert_eq!(session.correct_attempts, 0);
        assert!(session.started_at.is_none());
    }

    #[test]
    fn indentation_does_not_affect_completion_accuracy_or_wpm() {
        let mut code = TypingSession::create_code("  a\n\t b\n    c", None);
        let mut plain = TypingSession::create("a\nb\nc", None);
        for session in [&mut code, &mut plain] {
            session.insert_character('x', Duration::ZERO);
            session.delete_previous_character();
            for (index, character) in "a\nb\nc".chars().enumerate() {
                session.insert_character(character, Duration::from_secs(index as u64 + 1));
            }
            session.tick(Duration::from_secs(5));
            assert_eq!(session.attempts, 6);
            assert_eq!(session.correct_attempts, 5);
            assert_eq!(session.count_correct_characters(), 5);
            assert_eq!(session.ended_at, Some(Duration::from_secs(5)));
        }
        assert_eq!(code.calculate_accuracy(), plain.calculate_accuracy());
        assert_eq!(
            code.calculate_wpm(Duration::from_secs(100)),
            plain.calculate_wpm(Duration::from_secs(100))
        );
        assert_eq!(code.calculate_wpm(Duration::from_secs(100)), 12.0);
    }

    #[test]
    fn backspace_undo_and_line_changes_cross_auto_indent_without_typing_it() {
        let mut session = TypingSession::create_code(
            "    one();\n\t two();\n  three();",
            Some(Duration::from_secs(30)),
        );
        for character in "one();\n".chars() {
            session.insert_character(character, Duration::ZERO);
        }
        let attempts = session.attempts;
        session.delete_previous_character();
        assert_eq!(session.typed.iter().collect::<String>(), "one();");
        assert_eq!(session.cursor, 6);
        session.undo_edit();
        assert_eq!(session.typed.iter().collect::<String>(), "one();\n");
        session.redo_edit();
        assert_eq!(session.typed.iter().collect::<String>(), "one();");
        assert_eq!(session.attempts, attempts);
        for character in "\ntwo();".chars() {
            session.insert_character(character, Duration::ZERO);
        }
        session.enter_normal_mode();
        session.handle_normal_character('0');
        assert_eq!(session.cursor, 7);
        session.handle_normal_character('c');
        session.handle_normal_character('c');
        assert_eq!(session.mode, EditMode::Insert);
        assert_eq!(session.cursor, 7);
        assert_eq!(session.typed.iter().collect::<String>(), "one();\n");
        session.insert_character('t', Duration::ZERO);
        assert_eq!(session.typed.last(), Some(&'t'));
        assert_eq!(session.line_indentation.as_ref().unwrap()[&7], 5);
    }

    fn create_session(typed: &str) -> TypingSession {
        let mut session =
            TypingSession::create("hello world again and again", Some(Duration::from_secs(30)));
        for character in typed.chars() {
            session.insert_character(character, Duration::ZERO);
        }
        session
    }

    fn execute(session: &mut TypingSession, command: &str) {
        for character in command.chars() {
            session.handle_normal_character(character);
        }
    }

    #[test]
    fn changes_inner_word_and_preserves_surrounding_spaces() {
        let mut session = create_session("hello wrold");
        session.enter_normal_mode();
        execute(&mut session, "ciw");
        assert_eq!(session.typed.iter().collect::<String>(), "hello ");
        assert_eq!(session.cursor, 6);
        assert_eq!(session.mode, EditMode::Insert);
        for character in "world".chars() {
            session.insert_character(character, Duration::from_secs(2));
        }
        assert_eq!(session.count_correct_characters(), 11);
        assert!(session.calculate_accuracy() < 100.0);
    }

    #[test]
    fn deletes_inner_word_then_undoes_and_redoes() {
        let mut session = create_session("hello world again");
        session.enter_normal_mode();
        execute(&mut session, "bdiw");
        assert_eq!(session.typed.iter().collect::<String>(), "hello world ");
        execute(&mut session, "u");
        assert_eq!(
            session.typed.iter().collect::<String>(),
            "hello world again"
        );
        session.redo_edit();
        assert_eq!(session.typed.iter().collect::<String>(), "hello world ");
    }

    #[test]
    fn code_word_objects_distinguish_punctuation() {
        let mut session = create_session("call(value)");
        session.enter_normal_mode();
        execute(&mut session, "bciw");
        assert_eq!(session.typed.iter().collect::<String>(), "call()");
        assert_eq!(session.cursor, 5);
    }

    #[test]
    fn expires_before_accepting_late_input() {
        let mut session = create_session("hello");
        session.insert_character(' ', Duration::from_secs(31));
        assert_eq!(session.typed.len(), 5);
        assert_eq!(
            session.elapsed(Duration::from_secs(80)),
            Duration::from_secs(30)
        );
        assert_eq!(session.calculate_wpm(Duration::from_secs(80)), 2.0);
        assert!(session.ended_at.is_some());
    }

    #[test]
    fn waits_for_first_character_and_keeps_error_history() {
        let mut session = create_session("");
        session.tick(Duration::from_secs(100));
        assert_eq!(session.elapsed(Duration::from_secs(100)), Duration::ZERO);
        session.insert_character('x', Duration::from_secs(100));
        session.delete_previous_character();
        session.insert_character('h', Duration::from_secs(101));
        assert_eq!(session.calculate_accuracy(), 50.0);
        assert_eq!(session.calculate_wpm(Duration::from_secs(112)), 1.0);
    }

    #[test]
    fn unicode_indices_and_empty_commands_are_safe() {
        let mut session = TypingSession::create("été snow 界", Some(Duration::from_secs(15)));
        for character in "été".chars() {
            session.insert_character(character, Duration::ZERO);
        }
        session.enter_normal_mode();
        execute(&mut session, "ciw");
        assert!(session.typed.is_empty());
        session.enter_normal_mode();
        execute(&mut session, "bwe$0dddiwciw");
        assert_eq!(session.cursor, 0);
    }

    #[test]
    fn change_to_line_end_keeps_insertion_after_the_prefix() {
        let mut session = create_session("hello world");
        session.enter_normal_mode();
        execute(&mut session, "bC");
        assert_eq!(session.cursor, 6);
        session.insert_character('w', Duration::from_secs(1));
        assert_eq!(session.typed.iter().collect::<String>(), "hello w");
    }

    #[test]
    fn repeated_end_motion_advances_to_the_next_word() {
        let mut session = create_session("hello world again");
        session.enter_normal_mode();
        execute(&mut session, "0ee");
        assert_eq!(session.cursor, 10);
    }

    #[test]
    fn completion_freezes_statistics_with_or_without_a_timer() {
        for duration in [None, Some(Duration::from_secs(15))] {
            let mut session = TypingSession::create("hi", duration);
            session.insert_character('h', Duration::ZERO);
            session.insert_character('i', Duration::from_secs(1));
            assert_eq!(session.ended_at, Some(Duration::from_secs(1)));
            assert_eq!(session.calculate_wpm(Duration::from_secs(100)), 24.0);
            session.insert_character('x', Duration::from_secs(2));
            assert_eq!(session.attempts, 2);
        }
    }

    #[test]
    fn untimed_tests_wait_for_input_never_expire_and_keep_actual_elapsed_time() {
        let mut session = TypingSession::create("hi", None);
        session.tick(Duration::from_secs(1000));
        assert_eq!(session.elapsed(Duration::from_secs(1000)), Duration::ZERO);
        session.insert_character('x', Duration::from_secs(1000));
        session.delete_previous_character();
        session.insert_character('h', Duration::from_secs(1001));
        session.tick(Duration::from_secs(4600));
        assert!(session.ended_at.is_none());
        assert_eq!(
            session.elapsed(Duration::from_secs(4600)),
            Duration::from_secs(3600)
        );
        session.insert_character('i', Duration::from_secs(5000));
        assert_eq!(session.ended_at, Some(Duration::from_secs(5000)));
        assert_eq!(
            session.elapsed(Duration::from_secs(9000)),
            Duration::from_secs(4000)
        );
        assert_eq!(session.calculate_wpm(Duration::from_secs(9000)), 0.01);
        assert_eq!(session.attempts, 3);
        assert_eq!(session.correct_attempts, 2);
    }
}
