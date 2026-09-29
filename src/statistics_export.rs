use std::{path::PathBuf, time::Duration};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::{
    engine::TypingSession,
    statistics::{CharacterCounts, Sample, TypingError},
};

pub struct StatisticsExport {
    path: Option<PathBuf>,
    last_exported_end: Option<Duration>,
}

#[derive(Serialize)]
struct StatisticsReport<'a> {
    version: u8,
    timer_seconds: Option<f64>,
    elapsed_seconds: f64,
    wpm: f64,
    raw_wpm: f64,
    accuracy: f64,
    attempts: usize,
    correct_attempts: usize,
    typed_character_count: usize,
    correct_word_character_count: usize,
    samples: &'a [Sample],
    errors: &'a [TypingError],
}

impl StatisticsExport {
    pub fn create(path: Option<PathBuf>) -> Self {
        Self {
            path,
            last_exported_end: None,
        }
    }

    pub fn write_finished_session(&mut self, session: &TypingSession) -> Result<()> {
        let (Some(path), Some(end)) = (&self.path, session.ended_at) else {
            return Ok(());
        };
        if self.last_exported_end == Some(end) {
            return Ok(());
        }
        self.last_exported_end = Some(end);
        let report = StatisticsReport {
            version: 1,
            timer_seconds: session.duration.map(|duration| duration.as_secs_f64()),
            elapsed_seconds: session.elapsed(end).as_secs_f64(),
            wpm: session.calculate_wpm(end),
            raw_wpm: session.calculate_raw_wpm(end),
            accuracy: session.calculate_accuracy(),
            attempts: session.attempts,
            correct_attempts: session.correct_attempts,
            typed_character_count: session
                .typed
                .iter()
                .map(|character| character.len_utf16())
                .sum(),
            correct_word_character_count: CharacterCounts::count_correct_words(
                &session.target,
                &session.typed,
            ),
            samples: &session.samples,
            errors: &session.errors,
        };
        std::fs::write(path, serde_json::to_vec_pretty(&report)?).with_context(|| {
            format!(
                "Write statistics to {}",
                path.to_string_lossy().escape_debug()
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_exact_counts_without_passage_or_typed_text_and_only_when_finished() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("stats.json");
        let mut export = StatisticsExport::create(Some(path.clone()));
        let mut session = TypingSession::create("private passage", Some(Duration::from_secs(15)));
        session.insert_character('p', Duration::ZERO);
        session.insert_character('x', Duration::from_millis(125));
        session.delete_previous_character();
        session.insert_character('r', Duration::from_millis(250));
        export.write_finished_session(&session).unwrap();
        assert!(!path.exists());
        session.tick(Duration::from_secs(15));
        export.write_finished_session(&session).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let data: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(!text.contains("private"));
        assert!(data.get("target").is_none() && data.get("typed").is_none());
        assert_eq!(data["elapsed_seconds"], 15.0);
        assert_eq!(data["wpm"], 1.6);
        assert_eq!(data["samples"][0]["seconds"], 1.0);
        assert_eq!(data["samples"][0]["counts"]["correct_word"], 2);
        assert_eq!(data["samples"][0]["counts"]["insertions"], 3);
        assert_eq!(
            data["errors"],
            serde_json::json!([{ "seconds": 0.125, "is_corrected": true }])
        );
        assert_eq!(data["samples"][0]["wpm"], 24.0);
        std::fs::write(&path, "not rewritten").unwrap();
        export.write_finished_session(&session).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not rewritten");
    }

    #[test]
    fn export_is_opt_in_and_write_failures_do_not_repeat_every_frame() {
        let mut session = TypingSession::create("ab", None);
        session.insert_character('a', Duration::ZERO);
        session.insert_character('b', Duration::from_secs(1));
        StatisticsExport::create(None)
            .write_finished_session(&session)
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let mut export =
            StatisticsExport::create(Some(directory.path().join("missing/stats.json")));
        assert!(export.write_finished_session(&session).is_err());
        assert!(export.write_finished_session(&session).is_ok());
    }
}
