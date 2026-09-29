use std::{fs, path::Path};

use anyhow::{Context, Result, ensure};
use rand::seq::IndexedRandom;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use unicode_width::UnicodeWidthChar;

use crate::settings::{PracticeMode, ProseSettings, Settings, TextType};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Passage {
    pub text_type: TextType,
    pub group: String,
    pub title: String,
    pub author: String,
    pub source: String,
    pub license: String,
    pub text: String,
    #[serde(skip)]
    pub is_adapted: bool,
}

impl Passage {
    pub fn format_result_source(&self) -> Option<String> {
        let credit = match self.text_type {
            TextType::Books => format!("{} · {} · {}", self.title, self.author, self.source),
            TextType::Code => format!("{} · {} · {}", self.group, self.title, self.source),
        };
        Some(if self.is_adapted {
            format!("Adapted from {credit}")
        } else {
            credit
        })
    }

    fn apply_prose_options(&mut self, settings: &ProseSettings) {
        self.is_adapted = !settings.capitals || !settings.punctuation;
        if !settings.capitals {
            self.text = self.text.to_lowercase();
        }
        if !settings.punctuation {
            let words: Vec<String> = self
                .text
                .split_whitespace()
                .map(|word| {
                    word.chars()
                        .filter(|character| character.is_alphanumeric())
                        .collect::<String>()
                })
                .filter(|word| !word.is_empty())
                .collect();
            self.text = words.join(" ");
        }
    }

    fn matches_settings(&self, settings: &Settings) -> bool {
        match settings.mode {
            PracticeMode::Prose => {
                self.text_type == TextType::Books
                    && matches_filter(&settings.prose.books, &self.group)
                    && (settings.prose.punctuation || self.text.chars().any(char::is_alphanumeric))
            }
            PracticeMode::Code => {
                self.text_type == TextType::Code
                    && matches_filter(&settings.code.languages, &self.group)
            }
        }
    }
}

pub struct Corpus {
    passages: Vec<Passage>,
}

impl Corpus {
    pub fn load(directory: &Path) -> Result<Self> {
        let mut passages = Self::parse_passages(include_str!("../assets/corpus.json"))?;
        passages.extend(Self::parse_passages(include_str!("../assets/code.json"))?);
        let path = directory.join("corpus.json");
        if path.exists() {
            passages.extend(
                Self::parse_passages(&fs::read_to_string(&path)?)
                    .with_context(|| format!("Read {}", path.display()))?,
            );
        }
        Ok(Self { passages })
    }

    pub fn import(directory: &Path, source: &Path) -> Result<usize> {
        let imported = Self::parse_passages(&fs::read_to_string(source)?)?;
        let path = directory.join("corpus.json");
        let mut stored: Vec<Value> = if path.exists() {
            serde_json::from_str(&fs::read_to_string(&path)?)?
        } else {
            vec![]
        };
        Self::validate_passages(serde_json::from_value(Value::Array(stored.clone()))?)
            .with_context(|| format!("Read {}", path.display()))?;
        let count = imported.len();
        for passage in imported {
            stored.push(serde_json::to_value(passage)?);
        }
        fs::create_dir_all(directory)?;
        let temporary_path = directory.join("corpus.json.tmp");
        fs::write(&temporary_path, serde_json::to_string_pretty(&stored)?)?;
        fs::rename(temporary_path, path)?;
        Ok(count)
    }

    fn parse_passages(json: &str) -> Result<Vec<Passage>> {
        let passages =
            serde_json::from_str(json).context("Expected a JSON array of book or code passages")?;
        Self::validate_passages(passages)
    }

    fn validate_passages(mut passages: Vec<Passage>) -> Result<Vec<Passage>> {
        for passage in &mut passages {
            for label in [
                &passage.title,
                &passage.author,
                &passage.source,
                &passage.license,
                &passage.group,
            ] {
                ensure!(
                    !label.trim().is_empty() && !label.chars().any(char::is_control),
                    "Passage metadata must be nonempty and contain no control characters"
                );
            }
            passage.text = passage.text.replace("\r\n", "\n");
            ensure!(
                passage.text.chars().count() <= 30_000,
                "Passages must be at most 30,000 characters"
            );
            ensure!(
                !passage.text.trim().is_empty(),
                "Passage text cannot be empty"
            );
            ensure!(
                passage.text.chars().all(is_typable_character),
                "Passage contains terminal controls or unsupported zero-width characters"
            );
            if passage.text_type == TextType::Books {
                passage.text = passage
                    .text
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
            }
        }
        Ok(passages)
    }

    pub fn list_groups(&self, text_type: TextType) -> Vec<String> {
        let mut groups: Vec<_> = self
            .passages
            .iter()
            .filter(|passage| passage.text_type == text_type)
            .map(|passage| passage.group.clone())
            .collect();
        groups.sort();
        groups.dedup();
        groups
    }

    pub fn choose_passage(&self, settings: &Settings) -> Result<Passage> {
        settings.validate()?;
        let candidates: Vec<_> = self
            .passages
            .iter()
            .filter(|passage| passage.matches_settings(settings))
            .collect();
        let mut passage = (*candidates.choose(&mut rand::rng()).with_context(|| {
            format!(
                "No {} passages match. Choose more {} in Settings.",
                settings.mode.label(),
                match settings.mode {
                    PracticeMode::Prose => "Books",
                    PracticeMode::Code => "Languages",
                }
            )
        })?)
        .clone();
        if settings.mode == PracticeMode::Prose {
            passage.apply_prose_options(&settings.prose);
            ensure!(
                passage.text.chars().all(is_typable_character),
                "Lowercasing introduced unsupported combining characters. Turn Capitals on for this source."
            );
        }
        Ok(passage)
    }
}

fn is_typable_character(character: char) -> bool {
    matches!(character, '\n' | '\t')
        || (!character.is_control() && character.width().is_some_and(|width| width > 0))
}

fn matches_filter(filters: &[String], value: &str) -> bool {
    filters.is_empty() || filters.iter().any(|filter| filter == value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::TypingSession;
    use std::collections::{BTreeMap, HashSet};

    fn passage(text_type: TextType, text: &str) -> Passage {
        Passage {
            text_type,
            group: "Group".into(),
            title: "Title".into(),
            author: "Author".into(),
            source: "Source".into(),
            license: "CC0".into(),
            text: text.into(),
            is_adapted: false,
        }
    }

    #[test]
    fn attribution_preserves_sources_and_marks_transformed_prose() {
        for (kind, expected) in [
            (TextType::Books, "Title · Author · Source"),
            (TextType::Code, "Group · Title · Source"),
        ] {
            let mut sample = passage(kind, "Hello.");
            assert_eq!(sample.format_result_source().as_deref(), Some(expected));
            sample.is_adapted = true;
            assert_eq!(
                sample.format_result_source().unwrap(),
                format!("Adapted from {expected}")
            );
        }
    }

    #[test]
    fn bundled_passages_are_consistently_sized_complete_and_distinct() {
        let directory = tempfile::tempdir().unwrap();
        let corpus = Corpus::load(directory.path()).unwrap();
        let mut books = BTreeMap::new();
        let mut languages = BTreeMap::new();
        let mut seen = HashSet::new();
        for passage in &corpus.passages {
            let scored = match passage.text_type {
                TextType::Books => {
                    *books.entry(&passage.group).or_insert(0) += 1;
                    assert!(
                        passage
                            .text
                            .trim_end_matches(['\"', '\''])
                            .ends_with(['.', '!', '?'])
                    );
                    assert_eq!(passage.text.matches('"').count() % 2, 0);
                    let scored = passage.text.chars().count();
                    assert!((600..=900).contains(&scored), "{}: {scored}", passage.title);
                    scored
                }
                TextType::Code => {
                    *languages.entry(&passage.group).or_insert(0) += 1;
                    assert!(passage.text.is_ascii());
                    let scored = TypingSession::create_code(&passage.text, None).target.len();
                    assert!((1..=1600).contains(&scored), "{}: {scored}", passage.title);
                    scored
                }
            };
            assert!(scored > 0);
            assert!(seen.insert(passage.text.clone()));
            for boilerplate in [
                "project gutenberg",
                "transcriber's note",
                "all rights reserved",
                "[illustration",
            ] {
                assert!(!passage.text.to_ascii_lowercase().contains(boilerplate));
            }
        }
        assert_eq!(books.len(), 20);
        assert!(books.values().all(|count| *count == 10));
        assert_eq!(
            corpus.list_groups(TextType::Code),
            ["GDScript", "Go", "Python", "Rust", "TypeScript"]
        );
        assert!(languages.values().all(|count| *count == 20));
        assert_eq!(corpus.passages.len(), 300);
    }

    #[test]
    fn every_passage_is_eligible_for_every_timer_without_slicing_or_size_preferences() {
        let directory = tempfile::tempdir().unwrap();
        let corpus = Corpus::load(directory.path()).unwrap();
        for original in corpus.passages.iter().cloned().chain([
            passage(TextType::Books, "One complete sentence."),
            passage(TextType::Books, &"A complete sentence. ".repeat(100)),
            passage(TextType::Code, "x()"),
            passage(TextType::Code, &"call();\n".repeat(200)),
        ]) {
            let isolated = Corpus {
                passages: vec![original.clone()],
            };
            let mut settings = Settings::create_initial();
            settings.mode = match original.text_type {
                TextType::Books => PracticeMode::Prose,
                TextType::Code => PracticeMode::Code,
            };
            for seconds in Settings::TIMER_OPTIONS {
                settings.seconds = seconds;
                assert!(original.matches_settings(&settings));
                assert_eq!(
                    isolated.choose_passage(&settings).unwrap().text,
                    original.text
                );
            }
        }
        for mode in PracticeMode::ALL {
            let mut settings = Settings::create_initial();
            settings.mode = mode;
            let initial: Vec<_> = corpus
                .passages
                .iter()
                .filter(|passage| passage.matches_settings(&settings))
                .map(|passage| &passage.text)
                .collect();
            for seconds in Settings::TIMER_OPTIONS {
                settings.seconds = seconds;
                let eligible: Vec<_> = corpus
                    .passages
                    .iter()
                    .filter(|passage| passage.matches_settings(&settings))
                    .map(|passage| &passage.text)
                    .collect();
                assert_eq!(initial, eligible);
            }
        }
    }

    #[test]
    fn book_and_language_filters_include_every_matching_passage() {
        let directory = tempfile::tempdir().unwrap();
        let corpus = Corpus::load(directory.path()).unwrap();
        let mut settings = Settings::create_initial();
        for group in corpus.list_groups(TextType::Books) {
            settings.prose.books = vec![group.clone()];
            assert_eq!(corpus.choose_passage(&settings).unwrap().group, group);
        }
        settings.prose.books = vec!["missing".into()];
        assert!(
            corpus
                .choose_passage(&settings)
                .unwrap_err()
                .to_string()
                .contains("Choose more Books in Settings")
        );
        settings.mode = PracticeMode::Code;
        settings.prose.capitals = false;
        settings.prose.punctuation = false;
        for original in corpus
            .passages
            .iter()
            .filter(|passage| passage.text_type == TextType::Code)
        {
            settings.code.languages = vec![original.group.clone()];
            assert!(original.matches_settings(&settings));
            let eligible: Vec<_> = corpus
                .passages
                .iter()
                .filter(|passage| passage.matches_settings(&settings))
                .collect();
            assert_eq!(eligible.len(), 20);
            assert!(
                eligible
                    .iter()
                    .all(|passage| passage.group == original.group)
            );
            let selected = corpus.choose_passage(&settings).unwrap();
            assert_eq!(selected.group, original.group);
            assert!(!selected.is_adapted);
        }
        settings.code.languages = vec![];
        assert_eq!(
            corpus
                .passages
                .iter()
                .filter(|passage| passage.matches_settings(&settings))
                .count(),
            100
        );
        settings.code.languages = vec!["Rust".into(), "Go".into()];
        assert_eq!(
            corpus
                .passages
                .iter()
                .filter(|passage| passage.matches_settings(&settings))
                .count(),
            40
        );
        settings.code.languages = vec!["missing".into()];
        assert!(
            corpus
                .choose_passage(&settings)
                .unwrap_err()
                .to_string()
                .contains("Choose more Languages in Settings")
        );
    }

    #[test]
    fn capitals_and_punctuation_are_independent_and_never_change_word_order() {
        for (original, stripped) in [
            (
                "Don't re-enter, Alice! Numbers: 123.",
                "Dont reenter Alice Numbers 123",
            ),
            ("“Été,” she said—again. — ‘Oui!’", "Été she saidagain Oui"),
            (
                "(First) [second]; THIRD… first!",
                "First second THIRD first",
            ),
        ] {
            for capitals in [false, true] {
                for punctuation in [false, true] {
                    let settings = ProseSettings {
                        capitals,
                        punctuation,
                        books: vec![],
                    };
                    let expected = if punctuation { original } else { stripped };
                    let expected = if capitals {
                        expected.to_owned()
                    } else {
                        expected.to_lowercase()
                    };
                    for _ in 0..10 {
                        let mut sample = passage(TextType::Books, original);
                        sample.apply_prose_options(&settings);
                        assert_eq!(sample.text, expected);
                        assert_eq!(sample.is_adapted, !capitals || !punctuation);
                        assert_eq!(
                            sample
                                .format_result_source()
                                .unwrap()
                                .starts_with("Adapted from"),
                            sample.is_adapted
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn imports_preserve_code_indentation_normalize_prose_and_reject_unsafe_data() {
        let code = passage(TextType::Code, "\tcall(\tvalue);\r\n");
        let prose = passage(TextType::Books, "One\n  whole   sentence.");
        let parsed =
            Corpus::parse_passages(&serde_json::to_string(&vec![code, prose]).unwrap()).unwrap();
        assert_eq!(parsed[0].text, "\tcall(\tvalue);\n");
        assert_eq!(parsed[1].text, "One whole sentence.");
        for text in [" ", "bad\u{1b}[31m", "bad\u{200b}"] {
            assert!(
                Corpus::parse_passages(
                    &serde_json::to_string(&vec![passage(TextType::Code, text)]).unwrap()
                )
                .is_err()
            );
        }
        assert!(
            Corpus::parse_passages(
                &serde_json::to_string(&vec![passage(TextType::Books, &"a".repeat(30_001))])
                    .unwrap()
            )
            .is_err()
        );
        let mut invalid = passage(TextType::Books, "Valid.");
        invalid.title = "bad\u{1b}".into();
        assert!(Corpus::parse_passages(&serde_json::to_string(&vec![invalid]).unwrap()).is_err());
    }

    #[test]
    fn transformed_imports_never_create_empty_or_untypable_targets() {
        let corpus = Corpus {
            passages: vec![passage(TextType::Books, "İstanbul.")],
        };
        let mut settings = Settings::create_initial();
        settings.prose.capitals = false;
        assert!(
            corpus
                .choose_passage(&settings)
                .unwrap_err()
                .to_string()
                .contains("Capitals on")
        );
        let corpus = Corpus {
            passages: vec![passage(TextType::Books, "!!!")],
        };
        settings.prose.punctuation = false;
        assert!(corpus.choose_passage(&settings).is_err());
    }
}
