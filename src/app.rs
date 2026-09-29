use std::{path::PathBuf, time::Duration};

use anyhow::{Context, Result, ensure};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::{
    chart_text::ChartFont,
    content::{Corpus, Passage},
    engine::{EditMode, TypingSession},
    selection::{Choice, SelectionMode, SelectionOutcome, SelectionPrompt},
    settings::{Configuration, PracticeMode, Settings, SoundSettings, TextType},
    syntax::{CodeHighlights, SyntaxHighlighter},
    theme::Theme,
};

const RESULTS_COOLDOWN: Duration = Duration::from_secs(1);

pub const HELP_PAGES: [[&str; 4]; 4] = [
    [
        "Esc normal · i/a insert",
        "h/l move · w/b/e words",
        "ciw change · diw delete",
        "u undo · C-r redo",
    ],
    [
        "caw/daw include spaces",
        "cw/dw ahead · cb/db back",
        "C/D to end · cc/dd line",
        "0/$ edges · I/A insert",
    ],
    [
        "F1 keys · F2 settings",
        "F5 restart · C-c quit",
        "Enter new · Tab legend",
        "Esc back/cancel in menus",
    ],
    [
        "C-w erase previous word",
        "Code indent is automatic",
        "Help time counts.",
        "Corrected errors count.",
    ],
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Typing,
    Settings,
    Select,
    Results,
    Help,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingAction {
    Mode,
    Timer,
    Theme,
    Sound,
    SoundEnabled,
    Volume,
    Capitals,
    Punctuation,
    Books,
    Languages,
}

impl SettingAction {
    fn find_managed_key(self, configuration: &Configuration) -> Option<&'static str> {
        let keys: &[&str] = match self {
            Self::Mode => &["mode"],
            Self::Timer => &["seconds"],
            Self::Theme => &["theme"],
            Self::Sound => &["sound.is_enabled", "sound.volume"],
            Self::SoundEnabled => &["sound.is_enabled"],
            Self::Volume => &["sound.volume"],
            Self::Capitals => &["prose.capitals"],
            Self::Punctuation => &["prose.punctuation"],
            Self::Books => &["prose.books"],
            Self::Languages => &["code.languages"],
        };
        keys.iter()
            .copied()
            .find(|key| configuration.managed.is_managed(key))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputFeedback {
    Silent,
    Keystroke,
}

pub struct ActiveSelection {
    pub return_screen: Screen,
    pub action: SettingAction,
    pub prompt: SelectionPrompt,
}

pub struct SettingRow {
    pub name: String,
    pub value: String,
    pub action: SettingAction,
}

pub struct Application {
    pub configuration: Configuration,
    pub directory: PathBuf,
    pub corpus: Corpus,
    pub themes: Vec<Theme>,
    pub passage: Passage,
    pub session: TypingSession,
    pub code_highlights: CodeHighlights,
    syntax_highlighter: SyntaxHighlighter,
    pub screen: Screen,
    pub is_chart_legend_visible: bool,
    pub chart_font: ChartFont,
    pub selected_setting: usize,
    pub selection: Option<ActiveSelection>,
    pub notice: Option<String>,
    pub help_offset: usize,
    previous_screen: Screen,
    active_settings: Settings,
    settings_opened_at: Option<Duration>,
    results_opened_at: Option<Duration>,
    pub should_quit: bool,
}

pub struct ApplicationInput {
    pub configuration: Configuration,
    pub directory: PathBuf,
    pub corpus: Corpus,
    pub themes: Vec<Theme>,
}

impl Application {
    pub fn create(input: ApplicationInput) -> Result<Self> {
        let ApplicationInput {
            configuration,
            directory,
            corpus,
            themes,
        } = input;
        ensure!(
            themes
                .iter()
                .any(|theme| theme.selector == configuration.settings.theme),
            "Selected theme '{}' was not loaded",
            configuration.settings.theme
        );
        let (passage, notice) = match corpus.choose_passage(&configuration.settings) {
            Ok(passage) => (passage, None),
            Err(error) => (
                corpus.choose_passage(&Settings::create_initial())?,
                Some(format!("Error: {error}")),
            ),
        };
        let mut syntax_highlighter = SyntaxHighlighter::create();
        let code_highlights = prepare_highlights(&mut syntax_highlighter, &passage)?;
        let session = create_session(&passage, configuration.settings.seconds);
        let screen = if notice.is_some() {
            Screen::Settings
        } else {
            Screen::Typing
        };
        let application = Self {
            active_settings: if notice.is_some() {
                Settings::create_initial()
            } else {
                configuration.settings.clone()
            },
            settings_opened_at: (screen == Screen::Settings).then_some(Duration::ZERO),
            configuration,
            directory,
            corpus,
            themes,
            passage,
            session,
            code_highlights,
            syntax_highlighter,
            screen,
            selected_setting: 0,
            selection: None,
            notice,
            help_offset: 0,
            is_chart_legend_visible: false,
            chart_font: ChartFont::create(),
            previous_screen: Screen::Typing,
            results_opened_at: None,
            should_quit: false,
        };
        Ok(application)
    }

    pub fn current_theme(&self) -> &Theme {
        self.themes
            .iter()
            .find(|theme| theme.selector == self.configuration.settings.theme)
            .expect("Selected theme is validated on load and chosen from the loaded themes")
    }

    pub fn tick(&mut self, now: Duration) {
        if self.screen == Screen::Typing
            || (self.screen == Screen::Help && self.previous_screen == Screen::Typing)
        {
            self.session.tick(now);
            if self.session.ended_at.is_some() {
                self.screen = Screen::Results;
                self.results_opened_at = Some(now);
                self.notice = None;
            }
        }
    }

    pub fn is_results_cooling_down(&self, now: Duration) -> bool {
        self.screen == Screen::Results
            && self
                .results_opened_at
                .is_some_and(|opened_at| now.saturating_sub(opened_at) < RESULTS_COOLDOWN)
    }

    pub fn handle_key_with_feedback(
        &mut self,
        key: KeyEvent,
        now: Duration,
    ) -> Result<InputFeedback> {
        let was_inserting = self.screen == Screen::Typing && self.session.mode == EditMode::Insert;
        let previous_attempts = self.session.attempts;
        let previous_length = self.session.typed.len();
        self.handle_key(key, now)?;
        let was_deleting = key.code == KeyCode::Backspace
            || (key.code == KeyCode::Char('w') && key.modifiers.contains(KeyModifiers::CONTROL));
        let did_edit = self.session.attempts > previous_attempts
            || (was_deleting && self.session.typed.len() < previous_length);
        Ok(if was_inserting && did_edit {
            InputFeedback::Keystroke
        } else {
            InputFeedback::Silent
        })
    }

    pub fn handle_key(&mut self, key: KeyEvent, now: Duration) -> Result<()> {
        if key.kind == KeyEventKind::Release
            || key.modifiers.intersects(
                KeyModifiers::ALT | KeyModifiers::SUPER | KeyModifiers::HYPER | KeyModifiers::META,
            )
        {
            return Ok(());
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return self.save_and_quit();
        }
        let was_typing = self.screen == Screen::Typing;
        self.tick(now);
        if (was_typing && self.screen == Screen::Results)
            || self.is_results_cooling_down(now)
            || (self.screen == Screen::Results && key.kind == KeyEventKind::Repeat)
        {
            return Ok(());
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            if self.screen == Screen::Typing {
                match (key.code, self.session.mode) {
                    (KeyCode::Char('r'), EditMode::Normal) => self.session.redo_edit(),
                    (KeyCode::Char('w'), EditMode::Insert) => self.session.delete_previous_word(),
                    _ => {}
                }
            }
            return Ok(());
        }
        if key.code == KeyCode::F(1) {
            if self.screen == Screen::Help {
                self.screen = self.previous_screen;
            } else {
                self.previous_screen = self.screen;
                self.screen = Screen::Help;
                self.help_offset = 0;
            }
            return Ok(());
        }
        if self.can_use_practice_shortcuts() {
            match key.code {
                KeyCode::F(2) => {
                    self.open_settings(now);
                    return Ok(());
                }
                KeyCode::F(5) => {
                    self.restart_passage();
                    return Ok(());
                }
                _ => {}
            }
        }
        match self.screen {
            Screen::Typing => self.handle_typing_key(key, now),
            Screen::Settings => self.handle_settings_key(key, now)?,
            Screen::Select => self.handle_selection_key(key),
            Screen::Results => match key.code {
                KeyCode::Enter => self.start_new_passage()?,
                KeyCode::Tab => self.is_chart_legend_visible = !self.is_chart_legend_visible,
                _ => {}
            },
            Screen::Help => match key.code {
                KeyCode::Esc => self.screen = self.previous_screen,
                KeyCode::Down | KeyCode::Char('j') | KeyCode::PageDown => {
                    self.help_offset = (self.help_offset + 1).min(HELP_PAGES.len() - 1)
                }
                KeyCode::Up | KeyCode::Char('k') | KeyCode::PageUp => {
                    self.help_offset = self.help_offset.saturating_sub(1)
                }
                KeyCode::Home => self.help_offset = 0,
                KeyCode::End => self.help_offset = HELP_PAGES.len() - 1,
                _ => {}
            },
        }
        self.tick(now);
        Ok(())
    }

    pub fn can_use_practice_shortcuts(&self) -> bool {
        matches!(self.screen, Screen::Typing | Screen::Results)
            || (self.screen == Screen::Help
                && matches!(self.previous_screen, Screen::Typing | Screen::Results))
    }

    fn save_and_quit(&mut self) -> Result<()> {
        self.should_quit = true;
        self.configuration.save(&self.directory)
    }

    fn handle_typing_key(&mut self, key: KeyEvent, now: Duration) {
        match key.code {
            KeyCode::Esc => self.session.enter_normal_mode(),
            KeyCode::Left => self.session.move_cursor(-1),
            KeyCode::Right => self.session.move_cursor(1),
            KeyCode::Char(character)
                if self.session.mode == EditMode::Insert && !character.is_control() =>
            {
                self.session.insert_character(character, now)
            }
            KeyCode::Char(character) => self.session.handle_normal_character(character),
            KeyCode::Backspace if self.session.mode == EditMode::Insert => {
                self.session.delete_previous_character()
            }
            KeyCode::Enter if self.session.mode == EditMode::Insert => {
                self.session.insert_character('\n', now)
            }
            KeyCode::Tab if self.session.mode == EditMode::Insert => {
                if self.session.target.get(self.session.cursor) == Some(&'\t') {
                    self.session.insert_character('\t', now);
                    return;
                }
                let remaining_spaces = self.session.target[self.session.cursor..]
                    .iter()
                    .take_while(|character| **character == ' ')
                    .count();
                for _ in 0..remaining_spaces.clamp(1, 4) {
                    self.session.insert_character(' ', now);
                }
            }
            _ => {}
        }
    }

    fn open_settings(&mut self, now: Duration) {
        self.settings_opened_at = Some(now);
        self.screen = Screen::Settings;
        self.notice = None;
    }

    fn return_to_typing(&mut self, now: Duration) -> Result<()> {
        if !self
            .active_settings
            .matches_exercise(&self.configuration.settings)
            || self.session.ended_at.is_some()
            || !passage_matches_mode(&self.passage, self.configuration.settings.mode)
        {
            return self.start_new_passage();
        }
        self.configuration.save(&self.directory)?;
        if let Some(opened_at) = self.settings_opened_at.take()
            && let Some(started_at) = &mut self.session.started_at
        {
            *started_at += now.saturating_sub(opened_at);
        }
        self.screen = Screen::Typing;
        self.notice = None;
        Ok(())
    }

    fn handle_settings_key(&mut self, key: KeyEvent, now: Duration) -> Result<()> {
        let rows = self.build_setting_rows();
        let mut focused = self.selected_setting.min(rows.len() - 1);
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => focused = (focused + 1) % rows.len(),
            KeyCode::Up | KeyCode::Char('k') => focused = (focused + rows.len() - 1) % rows.len(),
            KeyCode::PageDown => focused = (focused + 6).min(rows.len() - 1),
            KeyCode::PageUp => focused = focused.saturating_sub(6),
            KeyCode::Home => focused = 0,
            KeyCode::End => focused = rows.len() - 1,
            KeyCode::Enter | KeyCode::Char(' ') => self.open_selection(rows[focused].action),
            KeyCode::Esc => self.return_to_typing(now)?,
            _ => {}
        }
        self.selected_setting = focused;
        Ok(())
    }

    fn open_selection(&mut self, action: SettingAction) {
        if let Some(key) = action.find_managed_key(&self.configuration) {
            self.notice = Some(format!("Managed by Nix: {key}"));
            return;
        }
        let settings = &self.configuration.settings;
        let (title, mode, choices): (&str, SelectionMode, Vec<Choice>) = match action {
            SettingAction::Mode => (
                "Mode",
                SelectionMode::Single,
                PracticeMode::ALL
                    .into_iter()
                    .map(|mode| Choice {
                        label: mode.label().into(),
                        is_selected: mode == settings.mode,
                    })
                    .collect(),
            ),
            SettingAction::Capitals | SettingAction::Punctuation | SettingAction::SoundEnabled => {
                let (title, current) = match action {
                    SettingAction::Capitals => ("Capitals", settings.prose.capitals),
                    SettingAction::Punctuation => ("Punctuation", settings.prose.punctuation),
                    _ => ("Sound", settings.sound.is_enabled),
                };
                (
                    title,
                    SelectionMode::Single,
                    [false, true]
                        .into_iter()
                        .map(|is_enabled| Choice {
                            label: if is_enabled { "On" } else { "Off" }.into(),
                            is_selected: is_enabled == current,
                        })
                        .collect(),
                )
            }
            SettingAction::Timer => (
                "Timer",
                SelectionMode::Single,
                Settings::TIMER_OPTIONS
                    .iter()
                    .map(|seconds| Choice {
                        label: seconds
                            .map_or_else(|| "Off".into(), |seconds| format!("{seconds}s")),
                        is_selected: *seconds == settings.seconds,
                    })
                    .collect(),
            ),
            SettingAction::Theme => (
                "Theme",
                SelectionMode::Single,
                self.themes
                    .iter()
                    .map(|theme| Choice {
                        label: theme.format_label(),
                        is_selected: theme.selector == settings.theme,
                    })
                    .collect(),
            ),
            SettingAction::Sound | SettingAction::Volume => (
                if action == SettingAction::Volume {
                    "Volume"
                } else {
                    "Sound"
                },
                SelectionMode::Single,
                settings
                    .sound
                    .list_volume_levels()
                    .into_iter()
                    .map(|volume| Choice {
                        label: if action == SettingAction::Volume {
                            format!("{volume}%")
                        } else {
                            SoundSettings::format_volume_level(volume)
                        },
                        is_selected: if action == SettingAction::Volume {
                            settings.sound.volume == volume
                        } else {
                            settings.sound.calculate_effective_volume() == volume
                        },
                    })
                    .collect(),
            ),
            SettingAction::Books | SettingAction::Languages => {
                let (title, filters, choices) = match action {
                    SettingAction::Books => (
                        "Books",
                        &settings.prose.books,
                        self.corpus.list_groups(TextType::Books),
                    ),
                    SettingAction::Languages => (
                        "Languages",
                        &settings.code.languages,
                        self.corpus.list_groups(TextType::Code),
                    ),
                    _ => unreachable!(),
                };
                (
                    title,
                    SelectionMode::Multiple,
                    choices
                        .into_iter()
                        .map(|label| Choice {
                            is_selected: filters.is_empty() || filters.contains(&label),
                            label,
                        })
                        .collect(),
                )
            }
        };
        if choices.is_empty() {
            self.notice = Some("No choices available. Import text first.".into());
            return;
        }
        let focused = choices
            .iter()
            .position(|choice| choice.is_selected)
            .unwrap_or(0);
        self.selection = Some(ActiveSelection {
            return_screen: self.screen,
            action,
            prompt: SelectionPrompt {
                title: title.into(),
                mode,
                choices,
                focused,
            },
        });
        self.notice = None;
        self.screen = Screen::Select;
    }

    fn handle_selection_key(&mut self, key: KeyEvent) {
        let Some(selection) = &mut self.selection else {
            return;
        };
        match selection.prompt.handle_key(key.code) {
            SelectionOutcome::Pending => self.notice = None,
            SelectionOutcome::Cancelled => {
                self.screen = selection.return_screen;
                self.selection = None;
                self.notice = None;
            }
            SelectionOutcome::Confirmed => self.apply_selection(),
        }
    }

    fn apply_selection(&mut self) {
        let Some(selection) = self.selection.take() else {
            return;
        };
        if let Some(key) = selection.action.find_managed_key(&self.configuration) {
            self.screen = selection.return_screen;
            self.notice = Some(format!("Managed by Nix: {key}"));
            return;
        }
        let prompt = &selection.prompt;
        if prompt.mode == SelectionMode::Multiple
            && !prompt.choices.iter().any(|choice| choice.is_selected)
        {
            self.notice = Some("Select at least one option.".into());
            self.selection = Some(selection);
            return;
        }
        let settings = &mut self.configuration.settings;
        match selection.action {
            SettingAction::Mode => settings.mode = PracticeMode::ALL[prompt.focused],
            SettingAction::Capitals => settings.prose.capitals = prompt.focused == 1,
            SettingAction::Punctuation => settings.prose.punctuation = prompt.focused == 1,
            SettingAction::Timer => settings.seconds = Settings::TIMER_OPTIONS[prompt.focused],
            SettingAction::Theme => settings.theme = self.themes[prompt.focused].selector.clone(),
            SettingAction::Sound => {
                let volume = settings.sound.list_volume_levels()[prompt.focused];
                settings.sound.select_volume(volume);
            }
            SettingAction::SoundEnabled => settings.sound.is_enabled = prompt.focused == 1,
            SettingAction::Volume => {
                settings.sound.volume = settings.sound.list_volume_levels()[prompt.focused]
            }
            SettingAction::Books | SettingAction::Languages => {
                let selected = if prompt.choices.iter().all(|choice| choice.is_selected) {
                    vec![]
                } else {
                    prompt
                        .choices
                        .iter()
                        .filter(|choice| choice.is_selected)
                        .map(|choice| choice.label.clone())
                        .collect()
                };
                match selection.action {
                    SettingAction::Books => settings.prose.books = selected,
                    SettingAction::Languages => settings.code.languages = selected,
                    _ => unreachable!(),
                }
            }
        }
        self.screen = selection.return_screen;
    }

    pub fn build_setting_rows(&self) -> Vec<SettingRow> {
        let settings = &self.configuration.settings;
        let mut rows = vec![
            setting_row("Timer", settings.format_timer(), SettingAction::Timer),
            setting_row(
                "Theme",
                self.current_theme().format_label(),
                SettingAction::Theme,
            ),
        ];
        if self.configuration.managed.is_managed("sound.is_enabled")
            || self.configuration.managed.is_managed("sound.volume")
        {
            rows.push(setting_row(
                "Sound",
                describe_toggle(settings.sound.is_enabled),
                SettingAction::SoundEnabled,
            ));
            rows.push(setting_row(
                "Volume",
                format!("{}%", settings.sound.volume),
                SettingAction::Volume,
            ));
        } else {
            rows.push(setting_row(
                "Sound",
                SoundSettings::format_volume_level(settings.sound.calculate_effective_volume()),
                SettingAction::Sound,
            ));
        }
        rows.push(setting_row(
            "Mode",
            settings.mode.label(),
            SettingAction::Mode,
        ));
        if settings.mode == PracticeMode::Prose {
            rows.push(setting_row(
                "Capitals",
                describe_toggle(settings.prose.capitals),
                SettingAction::Capitals,
            ));
            rows.push(setting_row(
                "Punctuation",
                describe_toggle(settings.prose.punctuation),
                SettingAction::Punctuation,
            ));
        }
        rows.push(match settings.mode {
            PracticeMode::Prose => setting_row(
                "Books",
                describe_filter(&settings.prose.books),
                SettingAction::Books,
            ),
            PracticeMode::Code => setting_row(
                "Languages",
                describe_filter(&settings.code.languages),
                SettingAction::Languages,
            ),
        });
        for row in &mut rows {
            if row.action.find_managed_key(&self.configuration).is_some() {
                row.value = format!("[Nix] {}", row.value);
            }
        }
        rows
    }

    pub fn restart_passage(&mut self) {
        self.active_settings = self.configuration.settings.clone();
        self.settings_opened_at = None;
        self.results_opened_at = None;
        self.is_chart_legend_visible = false;
        self.session = create_session(&self.passage, self.configuration.settings.seconds);
        self.screen = Screen::Typing;
        self.notice = None;
    }

    pub fn start_new_passage(&mut self) -> Result<()> {
        let passage = self
            .corpus
            .choose_passage(&self.configuration.settings)
            .context("Choose text")?;
        let code_highlights = prepare_highlights(&mut self.syntax_highlighter, &passage)?;
        self.configuration.save(&self.directory)?;
        self.passage = passage;
        self.code_highlights = code_highlights;
        self.restart_passage();
        Ok(())
    }
}

fn setting_row(name: &str, value: impl Into<String>, action: SettingAction) -> SettingRow {
    SettingRow {
        name: name.into(),
        value: value.into(),
        action,
    }
}

fn describe_toggle(is_enabled: bool) -> &'static str {
    if is_enabled { "on" } else { "off" }
}

fn describe_filter(filters: &[String]) -> String {
    if filters.is_empty() {
        "all".into()
    } else {
        filters.join(", ")
    }
}

fn passage_matches_mode(passage: &Passage, mode: PracticeMode) -> bool {
    (passage.text_type == TextType::Code) == (mode == PracticeMode::Code)
}

fn prepare_highlights(
    highlighter: &mut SyntaxHighlighter,
    passage: &Passage,
) -> Result<CodeHighlights> {
    if passage.text_type == TextType::Code {
        highlighter.highlight_code(&passage.group, &passage.text)
    } else {
        Ok(CodeHighlights::create_unhighlighted())
    }
}

fn create_session(passage: &Passage, seconds: Option<u64>) -> TypingSession {
    let duration = seconds.map(Duration::from_secs);
    if passage.text_type == TextType::Code {
        TypingSession::create_code(&passage.text, duration)
    } else {
        TypingSession::create(&passage.text, duration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_theme_selection_persists_its_path_and_never_shadows_a_builtin_name() {
        use crate::theme::ThemeSelection;
        let directory = tempfile::tempdir().unwrap();
        let mut native: serde_json::Value =
            serde_json::from_str(include_str!("../assets/themes/catppuccin-mocha.json")).unwrap();
        native["interface"]["accent"] = serde_json::json!("#123456");
        std::fs::write(directory.path().join("custom.json"), native.to_string()).unwrap();
        let mut configuration = Configuration::create_initial();
        configuration.settings.theme = "./custom.json".into();
        let mut themes = Theme::load_bundled().unwrap();
        Theme::load_selected(
            &mut themes,
            ThemeSelection {
                selector: &configuration.settings.theme,
                config_directory: directory.path(),
                home_directory: None,
            },
        )
        .unwrap();
        let mut application = Application::create(ApplicationInput {
            configuration,
            directory: directory.path().into(),
            corpus: Corpus::load(directory.path()).unwrap(),
            themes,
        })
        .unwrap();
        assert_eq!(
            application.current_theme().accent,
            ratatui::style::Color::Rgb(18, 52, 86)
        );
        let first = application.session.target[0];
        press(&mut application, KeyCode::Char(first));
        press(&mut application, KeyCode::F(2));
        application.open_selection(SettingAction::Theme);
        let prompt = &application.selection.as_ref().unwrap().prompt;
        assert_eq!(prompt.focused, 48);
        assert_eq!(prompt.choices[48].label, "file: catppuccin-mocha");
        press(&mut application, KeyCode::Home);
        press(&mut application, KeyCode::Esc);
        assert_eq!(application.configuration.settings.theme, "./custom.json");
        application.open_selection(SettingAction::Theme);
        press(&mut application, KeyCode::Home);
        press(&mut application, KeyCode::Enter);
        press(&mut application, KeyCode::Esc);
        assert_eq!(application.configuration.settings.theme, "catppuccin-mocha");
        assert_eq!(application.session.attempts, 1);
        press(&mut application, KeyCode::F(2));
        application.open_selection(SettingAction::Theme);
        press(&mut application, KeyCode::End);
        press(&mut application, KeyCode::Enter);
        press(&mut application, KeyCode::Esc);
        assert_eq!(
            Configuration::load(directory.path())
                .unwrap()
                .settings
                .theme,
            "./custom.json"
        );
        assert_eq!(application.session.attempts, 1);
    }

    #[test]
    fn selected_languages_start_typing_and_persist_without_resetting_the_passage() {
        for language in ["Rust", "Python", "TypeScript"] {
            let directory = tempfile::tempdir().unwrap();
            let mut configuration = Configuration::create_initial();
            configuration.settings.mode = PracticeMode::Code;
            configuration.settings.code.languages = vec![language.into()];
            let mut application = Application::create(ApplicationInput {
                configuration,
                directory: directory.path().into(),
                corpus: Corpus::load(directory.path()).unwrap(),
                themes: Theme::load_bundled().unwrap(),
            })
            .unwrap();
            assert_eq!(application.screen, Screen::Typing);
            assert_eq!(application.passage.group, language);
            assert_eq!(
                application.active_settings,
                application.configuration.settings
            );
            assert!(application.notice.is_none());
            let passage = application.passage.text.clone();
            let first = application.session.target[0];
            press(&mut application, KeyCode::Char(first));
            press(&mut application, KeyCode::F(2));
            press(&mut application, KeyCode::Esc);
            assert_eq!(application.passage.text, passage);
            assert_eq!(application.session.attempts, 1);
            assert_eq!(application.syntax_highlighter.count_parsed_passages(), 1);
            let saved = Configuration::load(directory.path()).unwrap();
            assert_eq!(saved.settings.code.languages, [language]);
        }
    }

    #[test]
    fn syntax_is_cached_across_edits_retries_theme_changes_and_resizes() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        assert_eq!(application.syntax_highlighter.count_parsed_passages(), 0);
        application.configuration.settings.mode = PracticeMode::Code;
        application.configuration.settings.code.languages = vec!["Rust".into()];
        application.start_new_passage().unwrap();
        assert_eq!(application.syntax_highlighter.count_parsed_passages(), 1);
        assert!(
            application
                .code_highlights
                .resolve_role_at_byte(0)
                .is_some()
        );
        let first_character = application.session.target[0];
        press(&mut application, KeyCode::Char(first_character));
        press(&mut application, KeyCode::Esc);
        press(&mut application, KeyCode::Char('u'));
        application
            .handle_key(
                KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
                Duration::ZERO,
            )
            .unwrap();
        press(&mut application, KeyCode::Char('i'));
        press(&mut application, KeyCode::Backspace);
        press(&mut application, KeyCode::F(5));
        press(&mut application, KeyCode::F(2));
        application.open_selection(SettingAction::Theme);
        press(&mut application, KeyCode::Down);
        press(&mut application, KeyCode::Enter);
        press(&mut application, KeyCode::Esc);
        for width in [24, 34, 70, 24] {
            crate::typing_view::TypingPreview {
                session: &application.session,
                theme: application.current_theme(),
                width,
            }
            .build_highlighted_lines(&application.code_highlights);
        }
        assert_eq!(application.syntax_highlighter.count_parsed_passages(), 1);
        application.configuration.settings.code.languages = vec!["Python".into()];
        application.start_new_passage().unwrap();
        assert_eq!(application.syntax_highlighter.count_parsed_passages(), 2);
        application.configuration.settings.mode = PracticeMode::Prose;
        application.start_new_passage().unwrap();
        assert_eq!(
            application.code_highlights,
            CodeHighlights::create_unhighlighted()
        );
        assert_eq!(application.syntax_highlighter.count_parsed_passages(), 2);
    }

    #[test]
    fn automatic_indent_is_silent_but_enter_and_inline_tabs_count_as_input() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        application.configuration.settings.mode = PracticeMode::Code;
        application.passage.text_type = TextType::Code;
        application.passage.text = "  x\t y\n\tz".into();
        application.restart_passage();
        assert_eq!(
            application.session.target.iter().collect::<String>(),
            "x\t y\nz"
        );
        assert_eq!(application.session.attempts, 0);
        for (index, code) in [
            KeyCode::Char('x'),
            KeyCode::Tab,
            KeyCode::Char(' '),
            KeyCode::Char('y'),
            KeyCode::Enter,
            KeyCode::Char('z'),
        ]
        .into_iter()
        .enumerate()
        {
            let feedback = application
                .handle_key_with_feedback(
                    KeyEvent::new(code, KeyModifiers::NONE),
                    Duration::from_secs(index as u64),
                )
                .unwrap();
            assert_eq!(feedback, InputFeedback::Keystroke);
            assert_eq!(application.session.attempts, index + 1);
        }
        assert_eq!(application.screen, Screen::Results);
        assert_eq!(application.session.calculate_accuracy(), 100.0);
        press_after_results_cooldown(&mut application, KeyCode::F(5));
        assert_eq!(application.screen, Screen::Typing);
        assert_eq!(application.session.attempts, 0);
        assert!(application.session.line_indentation.is_some());
    }

    #[test]
    fn managed_rows_are_read_only_and_do_not_discard_typing_progress() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("managed.json"),
            r#"{"theme":"nord","seconds":null}"#,
        )
        .unwrap();
        let mut application = create_managed_application(directory.path());
        let first = application.session.target[0];
        press(&mut application, KeyCode::Char(first));
        press(&mut application, KeyCode::F(2));
        for action in [SettingAction::Theme, SettingAction::Timer] {
            let row = application
                .build_setting_rows()
                .into_iter()
                .find(|row| row.action == action)
                .unwrap();
            assert!(row.value.starts_with("[Nix] "));
            application.open_selection(action);
            assert_eq!(application.screen, Screen::Settings);
            assert!(application.selection.is_none());
            assert!(
                application
                    .notice
                    .as_ref()
                    .unwrap()
                    .contains("Managed by Nix")
            );
        }
        application.open_selection(SettingAction::Sound);
        assert_eq!(application.screen, Screen::Select);
        press(&mut application, KeyCode::Home);
        press(&mut application, KeyCode::Enter);
        press(&mut application, KeyCode::Esc);
        assert_eq!(application.screen, Screen::Typing);
        assert_eq!(application.session.attempts, 1);
        assert_eq!(application.configuration.settings.theme, "nord");
        assert_eq!(application.configuration.settings.seconds, None);
        let saved: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(directory.path().join("config.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(saved["settings"]["theme"], "catppuccin-mocha");
        assert_eq!(saved["settings"]["seconds"], 30);
    }

    #[test]
    fn managing_one_audio_property_keeps_the_other_editable_in_f2() {
        let directory = tempfile::tempdir().unwrap();
        for (managed, locked, editable) in [
            (
                r#"{"sound":{"is_enabled":false}}"#,
                SettingAction::SoundEnabled,
                SettingAction::Volume,
            ),
            (
                r#"{"sound":{"volume":37}}"#,
                SettingAction::Volume,
                SettingAction::SoundEnabled,
            ),
        ] {
            std::fs::write(directory.path().join("managed.json"), managed).unwrap();
            let mut application = create_managed_application(directory.path());
            press(&mut application, KeyCode::F(2));
            application.open_selection(locked);
            assert!(application.selection.is_none());
            application.open_selection(editable);
            assert_eq!(application.screen, Screen::Select);
            press(&mut application, KeyCode::End);
            press(&mut application, KeyCode::Enter);
            if locked == SettingAction::SoundEnabled {
                assert!(!application.configuration.settings.sound.is_enabled);
                assert_eq!(application.configuration.settings.sound.volume, 100);
            } else {
                assert!(application.configuration.settings.sound.is_enabled);
                assert_eq!(application.configuration.settings.sound.volume, 37);
            }
            press(&mut application, KeyCode::Esc);
        }
    }

    #[test]
    fn unavailable_languages_open_settings_without_changing_the_selection() {
        for is_managed in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let mut configuration = Configuration::create_initial();
            configuration.settings.mode = PracticeMode::Code;
            configuration.settings.code.languages = vec!["Unavailable".into()];
            configuration.save(directory.path()).unwrap();
            if is_managed {
                std::fs::write(
                    directory.path().join("managed.json"),
                    r#"{"code":{"languages":["Unavailable"]}}"#,
                )
                .unwrap();
            }
            let application = create_managed_application(directory.path());
            assert_eq!(application.screen, Screen::Settings);
            assert_eq!(
                application.configuration.settings.code.languages,
                ["Unavailable"]
            );
            assert!(
                application
                    .notice
                    .as_ref()
                    .unwrap()
                    .contains("No Code passages match")
            );
            application.configuration.save(directory.path()).unwrap();
            assert_eq!(
                Configuration::load(directory.path())
                    .unwrap()
                    .settings
                    .code
                    .languages,
                ["Unavailable"]
            );
        }
    }

    fn create_managed_application(directory: &std::path::Path) -> Application {
        Application::create(ApplicationInput {
            configuration: Configuration::load(directory).unwrap(),
            directory: directory.into(),
            corpus: Corpus::load(directory).unwrap(),
            themes: Theme::load_bundled().unwrap(),
        })
        .unwrap()
    }

    fn create_application(directory: &std::path::Path) -> Application {
        Application::create(ApplicationInput {
            configuration: Configuration::create_initial(),
            directory: directory.into(),
            corpus: Corpus::load(directory).unwrap(),
            themes: Theme::load_bundled().unwrap(),
        })
        .unwrap()
    }

    fn press(application: &mut Application, code: KeyCode) {
        application
            .handle_key(
                KeyEvent::new(code, KeyModifiers::NONE),
                Duration::from_secs(1),
            )
            .unwrap();
    }

    fn press_after_results_cooldown(application: &mut Application, code: KeyCode) {
        let now = application.results_opened_at.unwrap() + RESULTS_COOLDOWN;
        application
            .handle_key(KeyEvent::new(code, KeyModifiers::NONE), now)
            .unwrap();
    }

    #[test]
    fn tab_toggles_only_the_legend_and_restart_hides_it() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        application.session = TypingSession::create("ab", None);
        for (second, character) in [(0, 'x'), (1, 'b')] {
            application
                .handle_key(
                    KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
                    Duration::from_secs(second),
                )
                .unwrap();
        }
        assert_eq!(application.screen, Screen::Results);
        let samples = application.session.samples.clone();
        let errors = application.session.errors.clone();
        assert_eq!(errors.len(), 1);
        application
            .handle_key(
                KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
                Duration::from_millis(1999),
            )
            .unwrap();
        assert!(!application.is_chart_legend_visible);
        for is_visible in [true, false, true, false, true] {
            press_after_results_cooldown(&mut application, KeyCode::Tab);
            assert_eq!(application.is_chart_legend_visible, is_visible);
            assert_eq!(application.screen, Screen::Results);
            assert_eq!(application.session.samples, samples);
            assert_eq!(application.session.errors, errors);
            assert_eq!(application.session.attempts, 2);
        }
        press_after_results_cooldown(&mut application, KeyCode::F(5));
        assert_eq!(application.screen, Screen::Typing);
        assert!(application.session.errors.is_empty());
        assert!(!application.is_chart_legend_visible);
    }

    #[test]
    fn results_ignore_stray_input_for_one_second_from_when_they_appear() {
        let directory = tempfile::tempdir().unwrap();
        for mode in PracticeMode::ALL {
            for seconds in [None, Some(15)] {
                let mut application = create_application(directory.path());
                application.configuration.settings.mode = mode;
                application.configuration.settings.seconds = seconds;
                application.passage.text_type = if mode == PracticeMode::Code {
                    TextType::Code
                } else {
                    TextType::Books
                };
                application.passage.text = "ab".into();
                application.restart_passage();
                application
                    .handle_key(
                        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
                        Duration::from_secs(10),
                    )
                    .unwrap();
                let shown_at = if seconds.is_some() {
                    application.tick(Duration::from_secs(100));
                    assert_eq!(application.session.ended_at, Some(Duration::from_secs(25)));
                    Duration::from_secs(100)
                } else {
                    application
                        .handle_key(
                            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE),
                            Duration::from_secs(11),
                        )
                        .unwrap();
                    Duration::from_secs(11)
                };
                let typed = application.session.typed.clone();
                let attempts = application.session.attempts;
                let ended_at = application.session.ended_at;
                let wpm = application.session.calculate_wpm(shown_at);
                let parses = application.syntax_highlighter.count_parsed_passages();
                for milliseconds in [0, 100, 999] {
                    let now = shown_at + Duration::from_millis(milliseconds);
                    application.tick(now);
                    assert!(application.is_results_cooling_down(now));
                    for code in [
                        KeyCode::Enter,
                        KeyCode::Char('n'),
                        KeyCode::Char('r'),
                        KeyCode::Char('s'),
                        KeyCode::Char('q'),
                        KeyCode::Esc,
                        KeyCode::F(1),
                        KeyCode::F(2),
                        KeyCode::F(5),
                        KeyCode::Tab,
                        KeyCode::Backspace,
                        KeyCode::Char('x'),
                    ] {
                        let feedback = application
                            .handle_key_with_feedback(KeyEvent::new(code, KeyModifiers::NONE), now)
                            .unwrap();
                        assert_eq!(feedback, InputFeedback::Silent);
                        assert_eq!(application.screen, Screen::Results);
                        assert!(!application.should_quit);
                        assert_eq!(application.session.typed, typed);
                        assert_eq!(application.session.attempts, attempts);
                        assert_eq!(application.session.ended_at, ended_at);
                        assert_eq!(application.session.calculate_wpm(now), wpm);
                        assert_eq!(
                            application.syntax_highlighter.count_parsed_passages(),
                            parses
                        );
                    }
                }
                let ready_at = shown_at + Duration::from_secs(1);
                assert!(!application.is_results_cooling_down(ready_at));
                application
                    .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), ready_at)
                    .unwrap();
                assert_eq!(application.screen, Screen::Typing);
                assert_eq!(application.session.attempts, 0);
                assert_eq!(application.results_opened_at, None);
            }
        }
    }

    #[test]
    fn each_finish_gets_a_new_cooldown_and_repeat_events_never_activate_results() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        application.passage.text = "a".into();
        application.restart_passage();
        for seconds in [10, 20] {
            let finished_at = Duration::from_secs(seconds);
            application
                .handle_key(
                    KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
                    finished_at,
                )
                .unwrap();
            assert_eq!(application.results_opened_at, Some(finished_at));
            application
                .handle_key(
                    KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE),
                    finished_at + Duration::from_millis(999),
                )
                .unwrap();
            assert_eq!(application.screen, Screen::Results);
            let ready_at = finished_at + Duration::from_secs(1);
            for code in [
                KeyCode::Enter,
                KeyCode::F(2),
                KeyCode::F(5),
                KeyCode::Char('n'),
                KeyCode::Char('r'),
                KeyCode::Char('s'),
                KeyCode::Char('q'),
            ] {
                application
                    .handle_key(
                        KeyEvent::new_with_kind(code, KeyModifiers::NONE, KeyEventKind::Repeat),
                        ready_at,
                    )
                    .unwrap();
                assert_eq!(application.screen, Screen::Results);
                assert!(!application.should_quit);
            }
            application
                .handle_key(KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE), ready_at)
                .unwrap();
            assert_eq!(application.screen, Screen::Typing);
            assert_eq!(application.results_opened_at, None);
        }
    }

    #[test]
    fn control_c_can_quit_during_the_results_cooldown() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        application.passage.text = "a".into();
        application.restart_passage();
        application
            .handle_key(
                KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
                Duration::ZERO,
            )
            .unwrap();
        assert!(application.is_results_cooling_down(Duration::ZERO));
        application
            .handle_key(
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                Duration::ZERO,
            )
            .unwrap();
        assert!(application.should_quit);
        assert_eq!(
            Configuration::load(directory.path()).unwrap().settings,
            application.configuration.settings
        );
    }

    #[test]
    fn every_timer_and_mode_finishes_the_same_passage_without_loading_another_source() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        for mode in PracticeMode::ALL {
            for seconds in Settings::TIMER_OPTIONS {
                application.configuration.settings.mode = mode;
                application.configuration.settings.seconds = seconds;
                application.start_new_passage().unwrap();
                let passage = application.passage.text.clone();
                let target = application.session.target.clone();
                let parses = application.syntax_highlighter.count_parsed_passages();
                for (index, character) in target.iter().enumerate() {
                    let code = match character {
                        '\n' => KeyCode::Enter,
                        '\t' => KeyCode::Tab,
                        character => KeyCode::Char(*character),
                    };
                    let feedback = application
                        .handle_key_with_feedback(
                            KeyEvent::new(code, KeyModifiers::NONE),
                            Duration::from_secs(if index == 0 { 1 } else { 2 }),
                        )
                        .unwrap();
                    assert_eq!(feedback, InputFeedback::Keystroke);
                }
                assert_eq!(application.screen, Screen::Results);
                assert_eq!(application.passage.text, passage);
                assert_eq!(application.session.target, target);
                assert_eq!(application.session.attempts, target.len());
                assert_eq!(application.session.calculate_accuracy(), 100.0);
                assert_eq!(
                    application.session.elapsed(Duration::from_secs(1000)),
                    Duration::from_secs(1)
                );
                assert_eq!(
                    application.session.calculate_wpm(Duration::from_secs(1000)),
                    target.len() as f64 * 12.0
                );
                assert_eq!(
                    application.syntax_highlighter.count_parsed_passages(),
                    parses
                );
            }
        }
    }

    #[test]
    fn timer_off_applies_transactionally_restarts_and_persists_without_length() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        press(&mut application, KeyCode::Char('a'));
        press(&mut application, KeyCode::F(2));
        application.open_selection(SettingAction::Timer);
        assert_eq!(
            application.selection.as_ref().unwrap().prompt.choices[0].label,
            "Off"
        );
        press(&mut application, KeyCode::Home);
        press(&mut application, KeyCode::Esc);
        assert_eq!(application.configuration.settings.seconds, Some(30));
        application.open_selection(SettingAction::Timer);
        press(&mut application, KeyCode::Home);
        press(&mut application, KeyCode::Enter);
        assert_eq!(application.build_setting_rows()[0].value, "Off");
        press(&mut application, KeyCode::Esc);
        assert_eq!(application.session.duration, None);
        assert_eq!(application.session.attempts, 0);
        assert!((600..=900).contains(&application.session.target.len()));
        assert_eq!(
            Configuration::load(directory.path())
                .unwrap()
                .settings
                .seconds,
            None
        );
        let saved = std::fs::read_to_string(directory.path().join("config.json")).unwrap();
        assert!(!saved.contains("length"));
    }

    #[test]
    fn untimed_settings_pause_elapsed_time_and_timeout_enter_cannot_start_another_test() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        application.configuration.settings.seconds = None;
        application.passage.text = "ab".into();
        application.restart_passage();
        for (code, seconds) in [
            (KeyCode::Char('a'), 100),
            (KeyCode::F(2), 102),
            (KeyCode::Esc, 202),
            (KeyCode::Char('b'), 205),
        ] {
            application
                .handle_key(
                    KeyEvent::new(code, KeyModifiers::NONE),
                    Duration::from_secs(seconds),
                )
                .unwrap();
        }
        assert_eq!(application.screen, Screen::Results);
        assert_eq!(
            application.session.elapsed(Duration::from_secs(300)),
            Duration::from_secs(5)
        );
        application.configuration.settings.seconds = Some(15);
        application.restart_passage();
        application
            .handle_key(
                KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
                Duration::from_secs(400),
            )
            .unwrap();
        application
            .handle_key(
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                Duration::from_secs(415),
            )
            .unwrap();
        assert_eq!(application.screen, Screen::Results);
        assert_eq!(application.session.typed, ['a']);
        assert_eq!(application.session.ended_at, Some(Duration::from_secs(415)));
    }

    #[test]
    fn leaving_settings_saves_changes_and_prepares_a_fresh_test() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        press(&mut application, KeyCode::Char('a'));
        press(&mut application, KeyCode::F(2));
        application.selected_setting = 0;
        press(&mut application, KeyCode::Enter);
        assert_eq!(application.screen, Screen::Select);
        press(&mut application, KeyCode::Down);
        assert_eq!(application.configuration.settings.seconds, Some(30));
        press(&mut application, KeyCode::Enter);
        assert_eq!(application.configuration.settings.seconds, Some(60));
        assert_eq!(application.screen, Screen::Settings);
        press(&mut application, KeyCode::Esc);
        assert!(!application.should_quit);
        assert_eq!(
            Configuration::load(directory.path()).unwrap().settings,
            application.configuration.settings
        );
        assert_eq!(application.screen, Screen::Typing);
        assert_eq!(application.session.duration, Some(Duration::from_secs(60)));
        assert!(application.session.started_at.is_none());
        assert!(application.session.typed.is_empty());
    }

    #[test]
    fn starts_on_the_speed_test_and_returns_without_changing_untouched_text() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        assert_eq!(application.screen, Screen::Typing);
        let target = application.session.target.clone();
        press(&mut application, KeyCode::F(2));
        assert_eq!(application.screen, Screen::Settings);
        press(&mut application, KeyCode::Esc);
        assert_eq!(application.screen, Screen::Typing);
        assert_eq!(application.session.target, target);
        assert!(application.session.started_at.is_none());
        assert!(!application.should_quit);
    }

    #[test]
    fn pauses_time_and_preserves_progress_through_settings_and_nested_views() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        let character = application.session.target[0];
        press(&mut application, KeyCode::Char(character));
        application
            .handle_key(
                KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE),
                Duration::from_secs(5),
            )
            .unwrap();
        press(&mut application, KeyCode::Enter);
        press(&mut application, KeyCode::Down);
        press(&mut application, KeyCode::Esc);
        assert_eq!(application.screen, Screen::Settings);
        assert_eq!(application.configuration.settings.seconds, Some(30));
        press(&mut application, KeyCode::F(1));
        application.tick(Duration::from_secs(100));
        assert!(application.session.ended_at.is_none());
        press(&mut application, KeyCode::Esc);
        application
            .handle_key(
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                Duration::from_secs(105),
            )
            .unwrap();
        assert_eq!(application.screen, Screen::Typing);
        assert_eq!(application.session.typed, vec![character]);
        assert_eq!(application.session.attempts, 1);
        assert_eq!(
            application.session.elapsed(Duration::from_secs(105)),
            Duration::from_secs(4)
        );
        assert_eq!(
            application.session.calculate_wpm(Duration::from_secs(105)),
            3.0
        );
        assert_eq!(application.session.samples.len(), 4);
        assert_eq!(
            application
                .session
                .samples
                .iter()
                .map(|sample| sample.seconds)
                .collect::<Vec<_>>(),
            [1.0, 2.0, 3.0, 4.0]
        );
        assert!(
            application.session.samples[1..]
                .iter()
                .all(|sample| sample.burst_wpm == 0.0)
        );
        let incorrect = if application.session.target[application.session.cursor] == 'x' {
            'y'
        } else {
            'x'
        };
        application
            .handle_key(
                KeyEvent::new(KeyCode::Char(incorrect), KeyModifiers::NONE),
                Duration::from_millis(105_500),
            )
            .unwrap();
        assert_eq!(application.session.errors[0].seconds, 4.5);
        assert!(!application.session.errors[0].is_corrected);
        application
            .handle_key(
                KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE),
                Duration::from_secs(106),
            )
            .unwrap();
        application
            .handle_key(
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                Duration::from_secs(206),
            )
            .unwrap();
        assert_eq!(
            application.session.elapsed(Duration::from_secs(206)),
            Duration::from_secs(5)
        );
        application
            .handle_key(
                KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
                Duration::from_millis(206_250),
            )
            .unwrap();
        assert_eq!(application.session.errors[0].seconds, 4.5);
        assert!(application.session.errors[0].is_corrected);
        application.tick(Duration::from_secs(231));
        assert_eq!(application.screen, Screen::Results);
    }

    #[test]
    fn theme_changes_do_not_reset_the_current_test() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        press(&mut application, KeyCode::Char('a'));
        let target = application.session.target.clone();
        press(&mut application, KeyCode::F(2));
        application.open_selection(SettingAction::Theme);
        press(&mut application, KeyCode::Down);
        press(&mut application, KeyCode::Enter);
        press(&mut application, KeyCode::Esc);
        assert_eq!(application.screen, Screen::Typing);
        assert_eq!(application.session.typed, vec!['a']);
        assert_eq!(application.session.target, target);
        assert_eq!(application.current_theme().name, "catppuccin-latte");
    }

    #[test]
    fn f2_opens_settings_from_normal_mode_and_preserves_the_paused_exercise() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        application.passage.text = "alpha beta".into();
        application.restart_passage();
        for (code, seconds) in [(KeyCode::Char('a'), 10), (KeyCode::Esc, 11)] {
            application
                .handle_key(
                    KeyEvent::new(code, KeyModifiers::NONE),
                    Duration::from_secs(seconds),
                )
                .unwrap();
        }
        let target = application.session.target.clone();
        let cursor = application.session.cursor;
        assert_eq!(application.session.mode, EditMode::Normal);
        assert_eq!(
            application
                .handle_key_with_feedback(
                    KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE),
                    Duration::from_secs(12)
                )
                .unwrap(),
            InputFeedback::Silent
        );
        assert_eq!(application.screen, Screen::Settings);
        application.tick(Duration::from_secs(100));
        assert_eq!(application.screen, Screen::Settings);
        application
            .handle_key(
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                Duration::from_secs(112),
            )
            .unwrap();
        assert_eq!(application.screen, Screen::Typing);
        assert_eq!(application.session.mode, EditMode::Normal);
        assert_eq!(application.session.target, target);
        assert_eq!(application.session.typed, ['a']);
        assert_eq!(application.session.attempts, 1);
        assert_eq!(application.session.cursor, cursor);
        assert_eq!(
            application.session.elapsed(Duration::from_secs(114)),
            Duration::from_secs(4)
        );
    }

    #[test]
    fn settings_from_results_return_to_a_fresh_speed_test() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        press(&mut application, KeyCode::Char('a'));
        application.tick(Duration::from_secs(31));
        assert_eq!(application.screen, Screen::Results);
        press_after_results_cooldown(&mut application, KeyCode::F(2));
        press(&mut application, KeyCode::Esc);
        assert_eq!(application.screen, Screen::Typing);
        assert!(application.session.started_at.is_none());
        assert!(application.session.ended_at.is_none());
        assert!(application.session.typed.is_empty());
        assert!(!application.should_quit);
    }

    #[test]
    fn failed_save_keeps_settings_open_and_preserves_the_pause() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        press(&mut application, KeyCode::Char('a'));
        application
            .handle_key(
                KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE),
                Duration::from_secs(5),
            )
            .unwrap();
        let file = directory.path().join("not-a-directory");
        std::fs::write(&file, "occupied").unwrap();
        application.directory = file;
        assert!(
            application
                .handle_key(
                    KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                    Duration::from_secs(10)
                )
                .is_err()
        );
        assert_eq!(application.screen, Screen::Settings);
        assert!(!application.should_quit);
        application.directory = directory.path().into();
        application
            .handle_key(
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                Duration::from_secs(20),
            )
            .unwrap();
        assert_eq!(
            application.session.elapsed(Duration::from_secs(20)),
            Duration::from_secs(4)
        );
    }

    #[test]
    fn shared_settings_precede_direct_mode_controls() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        for mode in PracticeMode::ALL {
            application.configuration.settings.mode = mode;
            let rows = application.build_setting_rows();
            let mut expected = vec!["Timer", "Theme", "Sound", "Mode"];
            if mode == PracticeMode::Prose {
                expected.extend(["Capitals", "Punctuation"]);
            }
            expected.push(if mode == PracticeMode::Prose {
                "Books"
            } else {
                "Languages"
            });
            assert_eq!(
                rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
                expected
            );
        }
    }

    #[test]
    fn changing_mode_exposes_languages_directly_and_cancel_returns_to_the_same_row() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        press(&mut application, KeyCode::F(2));
        application.selected_setting = 3;
        press(&mut application, KeyCode::Enter);
        press(&mut application, KeyCode::End);
        press(&mut application, KeyCode::Enter);
        assert_eq!(application.build_setting_rows()[4].name, "Languages");
        application.selected_setting = 4;
        press(&mut application, KeyCode::Enter);
        assert_eq!(application.screen, Screen::Select);
        let prompt = &application.selection.as_ref().unwrap().prompt;
        assert_eq!(prompt.title, "Languages");
        assert_eq!(prompt.mode, SelectionMode::Multiple);
        assert_eq!(
            prompt
                .choices
                .iter()
                .map(|choice| choice.label.as_str())
                .collect::<Vec<_>>(),
            ["GDScript", "Go", "Python", "Rust", "TypeScript"]
        );
        press(&mut application, KeyCode::Char('a'));
        press(&mut application, KeyCode::Esc);
        assert_eq!(application.screen, Screen::Settings);
        assert_eq!(application.selected_setting, 4);
        assert!(application.configuration.settings.code.languages.is_empty());
        press(&mut application, KeyCode::Esc);
        assert_eq!(application.screen, Screen::Typing);
    }

    #[test]
    fn selection_cancellation_does_not_change_settings() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        press(&mut application, KeyCode::F(2));
        application.open_selection(SettingAction::Theme);
        press(&mut application, KeyCode::Down);
        press(&mut application, KeyCode::Esc);
        assert_eq!(application.configuration.settings.theme, "catppuccin-mocha");
        application.open_selection(SettingAction::Capitals);
        press(&mut application, KeyCode::Home);
        press(&mut application, KeyCode::Esc);
        assert!(application.configuration.settings.prose.capitals);
        application.open_selection(SettingAction::Punctuation);
        press(&mut application, KeyCode::Home);
        press(&mut application, KeyCode::Esc);
        assert!(application.configuration.settings.prose.punctuation);
        assert_eq!(application.screen, Screen::Settings);
        assert!(application.selection.is_none());
    }

    #[test]
    fn mode_preferences_are_independent_and_sources_require_a_selection() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        press(&mut application, KeyCode::F(2));
        application.open_selection(SettingAction::Capitals);
        press(&mut application, KeyCode::Home);
        press(&mut application, KeyCode::Enter);
        application.open_selection(SettingAction::Punctuation);
        press(&mut application, KeyCode::Home);
        press(&mut application, KeyCode::Enter);
        let text = application.configuration.settings.prose.clone();
        application.open_selection(SettingAction::Mode);
        press(&mut application, KeyCode::End);
        press(&mut application, KeyCode::Enter);
        assert_eq!(application.configuration.settings.prose, text);
        application.open_selection(SettingAction::Mode);
        press(&mut application, KeyCode::Home);
        press(&mut application, KeyCode::Enter);
        assert!(!application.configuration.settings.prose.capitals);
        assert!(!application.configuration.settings.prose.punctuation);
        application.selected_setting = 6;
        press(&mut application, KeyCode::Enter);
        press(&mut application, KeyCode::Char('a'));
        press(&mut application, KeyCode::Enter);
        assert_eq!(application.screen, Screen::Select);
        assert!(
            application
                .notice
                .as_ref()
                .unwrap()
                .contains("at least one")
        );
        press(&mut application, KeyCode::Char(' '));
        press(&mut application, KeyCode::Enter);
        assert_eq!(application.screen, Screen::Settings);
        assert_eq!(application.configuration.settings.prose.books.len(), 1);
        assert_eq!(application.build_setting_rows().len(), 7);
        assert_eq!(application.build_setting_rows()[6].name, "Books");
        assert_eq!(application.selected_setting, 6);
    }

    #[test]
    fn group_multiselect_preserves_all_semantics_and_rejects_none() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        application.configuration.settings.mode = PracticeMode::Code;
        application.screen = Screen::Settings;
        application.open_selection(SettingAction::Languages);
        assert!(
            application
                .selection
                .as_ref()
                .unwrap()
                .prompt
                .choices
                .iter()
                .all(|choice| choice.is_selected)
        );
        press(&mut application, KeyCode::Char('a'));
        press(&mut application, KeyCode::Enter);
        assert_eq!(application.screen, Screen::Select);
        assert!(
            application
                .notice
                .as_ref()
                .unwrap()
                .contains("at least one")
        );
        press(&mut application, KeyCode::Char(' '));
        let selected = application.selection.as_ref().unwrap().prompt.choices[0]
            .label
            .clone();
        press(&mut application, KeyCode::Enter);
        assert_eq!(
            application.configuration.settings.code.languages,
            vec![selected]
        );
        application.open_selection(SettingAction::Languages);
        press(&mut application, KeyCode::Char('a'));
        press(&mut application, KeyCode::Enter);
        assert!(application.configuration.settings.code.languages.is_empty());
    }

    #[test]
    fn single_select_applies_mode_and_theme() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        application.open_selection(SettingAction::Mode);
        press(&mut application, KeyCode::End);
        press(&mut application, KeyCode::Enter);
        assert_eq!(application.configuration.settings.mode, PracticeMode::Code);
        application.open_selection(SettingAction::Theme);
        press(&mut application, KeyCode::Down);
        press(&mut application, KeyCode::Enter);
        assert_eq!(application.configuration.settings.theme, "catppuccin-latte");
    }

    #[test]
    fn settings_arrows_do_not_cycle_field_values() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        press(&mut application, KeyCode::F(2));
        application.selected_setting = 3;
        let original = application.configuration.settings.clone();
        press(&mut application, KeyCode::Right);
        press(&mut application, KeyCode::Left);
        assert_eq!(application.configuration.settings, original);
        press(&mut application, KeyCode::Enter);
        assert_eq!(application.screen, Screen::Select);
    }

    #[test]
    fn help_does_not_pause_the_timer_and_results_can_restart() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        press(&mut application, KeyCode::Char('a'));
        press(&mut application, KeyCode::F(1));
        application.tick(Duration::from_secs(32));
        assert_eq!(application.screen, Screen::Results);
        let target = application.session.target.clone();
        press_after_results_cooldown(&mut application, KeyCode::F(5));
        assert_eq!(application.screen, Screen::Typing);
        assert_eq!(application.session.target, target);
        assert_eq!(application.session.attempts, 0);
        assert!(application.session.started_at.is_none());
        assert!(application.session.ended_at.is_none());
        assert!(application.session.typed.is_empty());
    }

    #[test]
    fn enter_chooses_new_text_from_results() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        application.passage.text = "old passage".into();
        application.restart_passage();
        press(&mut application, KeyCode::Char('o'));
        application.tick(Duration::from_secs(31));
        assert_eq!(application.screen, Screen::Results);

        press_after_results_cooldown(&mut application, KeyCode::Enter);

        assert_eq!(application.screen, Screen::Typing);
        assert_ne!(application.passage.text, "old passage");
        assert_eq!(
            application.session.target.iter().collect::<String>(),
            application.passage.text
        );
        assert_eq!(application.session.attempts, 0);
        assert!(application.session.started_at.is_none());
        assert!(application.session.ended_at.is_none());
        assert!(application.session.typed.is_empty());
    }

    #[test]
    fn audio_feedback_tracks_real_insert_edits_not_navigation_or_commands() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        application.passage.text = "ab    cd\nef".into();
        application.restart_passage();
        let mut send = |code, modifiers| {
            application
                .handle_key_with_feedback(KeyEvent::new(code, modifiers), Duration::from_secs(1))
                .unwrap()
        };
        assert_eq!(
            send(KeyCode::Backspace, KeyModifiers::NONE),
            InputFeedback::Silent
        );
        assert_eq!(
            send(KeyCode::Char('a'), KeyModifiers::NONE),
            InputFeedback::Keystroke
        );
        assert_eq!(
            send(KeyCode::Char('x'), KeyModifiers::NONE),
            InputFeedback::Keystroke
        );
        assert_eq!(
            send(KeyCode::Backspace, KeyModifiers::NONE),
            InputFeedback::Keystroke
        );
        assert_eq!(
            send(KeyCode::Char('b'), KeyModifiers::NONE),
            InputFeedback::Keystroke
        );
        assert_eq!(
            send(KeyCode::Tab, KeyModifiers::NONE),
            InputFeedback::Keystroke
        );
        assert_eq!(
            send(KeyCode::Left, KeyModifiers::NONE),
            InputFeedback::Silent
        );
        assert_eq!(
            send(KeyCode::Char('x'), KeyModifiers::ALT),
            InputFeedback::Silent
        );
        assert_eq!(
            send(KeyCode::Char('w'), KeyModifiers::CONTROL),
            InputFeedback::Keystroke
        );
        assert_eq!(
            send(KeyCode::Esc, KeyModifiers::NONE),
            InputFeedback::Silent
        );
        assert_eq!(
            send(KeyCode::Char('x'), KeyModifiers::NONE),
            InputFeedback::Silent
        );
        assert_eq!(
            send(KeyCode::Char('i'), KeyModifiers::NONE),
            InputFeedback::Silent
        );
        assert_eq!(
            send(KeyCode::Enter, KeyModifiers::NONE),
            InputFeedback::Keystroke
        );
        assert_eq!(
            send(KeyCode::F(5), KeyModifiers::NONE),
            InputFeedback::Silent
        );
        assert_eq!(
            send(KeyCode::F(2), KeyModifiers::NONE),
            InputFeedback::Silent
        );
        assert_eq!(
            send(KeyCode::Down, KeyModifiers::NONE),
            InputFeedback::Silent
        );
        assert_eq!(
            send(KeyCode::Enter, KeyModifiers::NONE),
            InputFeedback::Silent
        );
    }

    #[test]
    fn final_character_clicks_but_expired_keys_and_release_events_are_silent() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        application.passage.text = "a".into();
        application.restart_passage();
        let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        assert_eq!(
            application
                .handle_key_with_feedback(key, Duration::ZERO)
                .unwrap(),
            InputFeedback::Keystroke
        );
        assert_eq!(application.screen, Screen::Results);
        let restart = KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE);
        assert_eq!(
            application
                .handle_key_with_feedback(restart, Duration::ZERO)
                .unwrap(),
            InputFeedback::Silent
        );
        let release = KeyEvent::new_with_kind(
            KeyCode::Char('a'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        assert_eq!(
            application
                .handle_key_with_feedback(release, Duration::ZERO)
                .unwrap(),
            InputFeedback::Silent
        );
        application.passage.text = "ab".into();
        application.restart_passage();
        application
            .handle_key_with_feedback(key, Duration::ZERO)
            .unwrap();
        assert_eq!(
            application
                .handle_key_with_feedback(key, Duration::from_secs(31))
                .unwrap(),
            InputFeedback::Silent
        );
        assert_eq!(application.screen, Screen::Results);
    }

    #[test]
    fn unified_sound_picker_applies_transactionally_without_restarting_the_test() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        press(&mut application, KeyCode::Char('a'));
        let target = application.session.target.clone();
        press(&mut application, KeyCode::F(2));
        application.open_selection(SettingAction::Sound);
        let prompt = &application.selection.as_ref().unwrap().prompt;
        assert_eq!(
            prompt
                .choices
                .iter()
                .map(|choice| choice.label.as_str())
                .collect::<Vec<_>>(),
            ["Off", "10%", "25%", "50%", "75%", "100%"]
        );
        assert_eq!(prompt.choices[prompt.focused].label, "50%");
        press(&mut application, KeyCode::Home);
        press(&mut application, KeyCode::Esc);
        assert!(application.configuration.settings.sound.is_enabled);
        application.open_selection(SettingAction::Sound);
        press(&mut application, KeyCode::Home);
        press(&mut application, KeyCode::Enter);
        assert!(!application.configuration.settings.sound.is_enabled);
        application.open_selection(SettingAction::Sound);
        assert_eq!(application.selection.as_ref().unwrap().prompt.focused, 0);
        press(&mut application, KeyCode::End);
        press(&mut application, KeyCode::Esc);
        assert_eq!(application.configuration.settings.sound.volume, 50);
        assert!(!application.configuration.settings.sound.is_enabled);
        application.configuration.settings.sound.volume = 37;
        application.open_selection(SettingAction::Sound);
        let prompt = &application.selection.as_ref().unwrap().prompt;
        assert_eq!(prompt.choices[prompt.focused].label, "Off");
        assert!(prompt.choices.iter().any(|choice| choice.label == "37%"));
        press(&mut application, KeyCode::Home);
        press(&mut application, KeyCode::Down);
        press(&mut application, KeyCode::Enter);
        assert_eq!(application.configuration.settings.sound.volume, 10);
        assert!(application.configuration.settings.sound.is_enabled);
        application
            .handle_key(
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                Duration::from_secs(101),
            )
            .unwrap();
        assert_eq!(application.session.typed, vec!['a']);
        assert_eq!(application.session.target, target);
        assert_eq!(
            application.session.elapsed(Duration::from_secs(101)),
            Duration::ZERO
        );
        assert_eq!(
            Configuration::load(directory.path())
                .unwrap()
                .settings
                .sound,
            application.configuration.settings.sound
        );
    }

    #[test]
    fn key_release_does_not_type_twice() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        let key = KeyEvent::new_with_kind(
            KeyCode::Char('a'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        application.handle_key(key, Duration::ZERO).unwrap();
        assert!(application.session.typed.is_empty());
    }

    #[test]
    fn ctrl_c_can_exit_even_if_saving_fails() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        let file = directory.path().join("not-a-directory");
        std::fs::write(&file, "occupied").unwrap();
        application.directory = file;
        assert!(
            application
                .handle_key(
                    KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                    Duration::ZERO
                )
                .is_err()
        );
        assert!(application.should_quit);
    }

    #[test]
    fn results_use_f2_for_settings_and_escape_never_quits() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        application.passage.text = "a".into();
        application.restart_passage();
        press(&mut application, KeyCode::Char('a'));
        for code in [KeyCode::Char('s'), KeyCode::Esc] {
            press_after_results_cooldown(&mut application, code);
            assert_eq!(application.screen, Screen::Results);
            assert!(!application.should_quit);
        }
        press_after_results_cooldown(&mut application, KeyCode::F(2));
        assert_eq!(application.screen, Screen::Settings);
    }

    #[test]
    fn f5_restarts_the_same_passage_from_typing_results_and_their_help() {
        let directory = tempfile::tempdir().unwrap();
        for mode in [EditMode::Insert, EditMode::Normal] {
            for origin in [Screen::Typing, Screen::Results] {
                for through_help in [false, true] {
                    let mut application = create_application(directory.path());
                    application.passage.text = "alpha beta".into();
                    application.restart_passage();
                    press(&mut application, KeyCode::Char('a'));
                    application.session.mode = mode;
                    let now = if origin == Screen::Results {
                        application.tick(Duration::from_secs(31));
                        Duration::from_secs(32)
                    } else {
                        Duration::from_secs(2)
                    };
                    assert_eq!(application.screen, origin);
                    if through_help {
                        application
                            .handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE), now)
                            .unwrap();
                    }
                    let parses = application.syntax_highlighter.count_parsed_passages();
                    assert_eq!(
                        application
                            .handle_key_with_feedback(
                                KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE),
                                now
                            )
                            .unwrap(),
                        InputFeedback::Silent
                    );
                    assert_eq!(application.screen, Screen::Typing);
                    assert_eq!(application.session.mode, EditMode::Insert);
                    assert_eq!(application.passage.text, "alpha beta");
                    assert!(application.session.typed.is_empty());
                    assert_eq!(application.session.attempts, 0);
                    assert_eq!(application.results_opened_at, None);
                    assert_eq!(
                        application.syntax_highlighter.count_parsed_passages(),
                        parses
                    );
                }
            }
        }
    }

    #[test]
    fn help_allows_practice_shortcuts_without_discarding_nested_settings() {
        let directory = tempfile::tempdir().unwrap();
        for origin in [
            Screen::Typing,
            Screen::Results,
            Screen::Settings,
            Screen::Select,
        ] {
            let mut application = create_application(directory.path());
            application.screen = origin;
            if origin == Screen::Select {
                application.screen = Screen::Settings;
                application.open_selection(SettingAction::Theme);
                press(&mut application, KeyCode::Down);
            }
            press(&mut application, KeyCode::F(1));
            assert_eq!(application.screen, Screen::Help);
            press(&mut application, KeyCode::F(2));
            if matches!(origin, Screen::Typing | Screen::Results) {
                assert_eq!(application.screen, Screen::Settings);
            } else {
                assert_eq!(application.screen, Screen::Help);
                press(&mut application, KeyCode::F(5));
                assert_eq!(application.screen, Screen::Help);
                press(&mut application, KeyCode::Esc);
                assert_eq!(application.screen, origin);
                if origin == Screen::Select {
                    assert_eq!(application.selection.as_ref().unwrap().prompt.focused, 1);
                    assert_eq!(application.configuration.settings.theme, "catppuccin-mocha");
                }
            }
        }
    }

    #[test]
    fn modified_keys_do_not_trigger_plain_shortcuts_or_menu_navigation() {
        let directory = tempfile::tempdir().unwrap();
        for screen in [
            Screen::Typing,
            Screen::Results,
            Screen::Settings,
            Screen::Select,
            Screen::Help,
        ] {
            let mut application = create_application(directory.path());
            if screen == Screen::Select {
                application.screen = Screen::Settings;
                application.open_selection(SettingAction::Theme);
            }
            application.screen = screen;
            let settings = application.configuration.settings.clone();
            let target = application.session.target.clone();
            for modifiers in [
                KeyModifiers::CONTROL,
                KeyModifiers::ALT,
                KeyModifiers::SUPER,
                KeyModifiers::META,
                KeyModifiers::HYPER,
                KeyModifiers::CONTROL | KeyModifiers::ALT,
            ] {
                for code in [
                    KeyCode::Char('a'),
                    KeyCode::Char('j'),
                    KeyCode::Char('k'),
                    KeyCode::Char('r'),
                    KeyCode::Char('q'),
                    KeyCode::Enter,
                    KeyCode::Esc,
                    KeyCode::Home,
                    KeyCode::End,
                    KeyCode::F(1),
                    KeyCode::F(2),
                    KeyCode::F(5),
                ] {
                    assert_eq!(
                        application
                            .handle_key_with_feedback(
                                KeyEvent::new(code, modifiers),
                                Duration::ZERO
                            )
                            .unwrap(),
                        InputFeedback::Silent
                    );
                    assert_eq!(application.screen, screen);
                    assert!(!application.should_quit);
                    assert_eq!(application.configuration.settings, settings);
                    assert_eq!(application.session.target, target);
                    assert_eq!(application.session.attempts, 0);
                    assert_eq!(application.selected_setting, 0);
                    assert_eq!(application.help_offset, 0);
                    if let Some(selection) = &application.selection {
                        assert_eq!(selection.prompt.focused, 0);
                    }
                }
            }
        }
    }

    #[test]
    fn home_and_end_work_in_settings_and_every_help_page() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = create_application(directory.path());
        for mode in PracticeMode::ALL {
            application.configuration.settings.mode = mode;
            application.screen = Screen::Settings;
            press(&mut application, KeyCode::End);
            assert_eq!(
                application.selected_setting,
                application.build_setting_rows().len() - 1
            );
            press(&mut application, KeyCode::Home);
            assert_eq!(application.selected_setting, 0);
        }
        press(&mut application, KeyCode::F(1));
        for index in 1..HELP_PAGES.len() {
            press(&mut application, KeyCode::PageDown);
            assert_eq!(application.help_offset, index);
        }
        press(&mut application, KeyCode::Down);
        assert_eq!(application.help_offset, HELP_PAGES.len() - 1);
        press(&mut application, KeyCode::Home);
        assert_eq!(application.help_offset, 0);
        press(&mut application, KeyCode::End);
        assert_eq!(application.help_offset, HELP_PAGES.len() - 1);
    }

    #[test]
    fn control_c_reports_save_failures_and_exits_from_every_screen() {
        let directory = tempfile::tempdir().unwrap();
        let blocked_directory = directory.path().join("not-a-directory");
        std::fs::write(&blocked_directory, "occupied").unwrap();
        for screen in [
            Screen::Typing,
            Screen::Settings,
            Screen::Select,
            Screen::Help,
            Screen::Results,
        ] {
            let mut application = create_application(directory.path());
            application.directory = blocked_directory.clone();
            application.screen = screen;
            assert!(
                application
                    .handle_key(
                        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                        Duration::ZERO
                    )
                    .is_err()
            );
            assert!(application.should_quit);
        }
    }

    #[test]
    fn settings_can_repair_missing_import_filters() {
        let directory = tempfile::tempdir().unwrap();
        let mut configuration = Configuration::create_initial();
        configuration.settings.mode = PracticeMode::Prose;
        configuration.settings.prose.books = vec!["Removed local book".into()];
        let mut application = Application::create(ApplicationInput {
            configuration,
            directory: directory.path().into(),
            corpus: Corpus::load(directory.path()).unwrap(),
            themes: Theme::load_bundled().unwrap(),
        })
        .unwrap();
        assert_eq!(application.screen, Screen::Settings);
        assert!(
            application
                .notice
                .as_ref()
                .unwrap()
                .contains("passages match")
        );
        assert!(
            application
                .handle_key(
                    KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                    Duration::ZERO
                )
                .is_err()
        );
        assert_eq!(application.screen, Screen::Settings);
        application.open_selection(SettingAction::Books);
        press(&mut application, KeyCode::Char('a'));
        press(&mut application, KeyCode::Enter);
        press(&mut application, KeyCode::Esc);
        assert_eq!(application.screen, Screen::Typing);
        assert_eq!(application.passage.text_type, TextType::Books);
    }
}
