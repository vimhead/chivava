use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::managed_settings::{ManagedSettings, PreferenceSave};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TextType {
    Books,
    Code,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PracticeMode {
    Prose,
    Code,
}

impl PracticeMode {
    pub const ALL: [Self; 2] = [Self::Prose, Self::Code];
    pub fn label(self) -> &'static str {
        match self {
            Self::Prose => "Prose",
            Self::Code => "Code",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SoundSettings {
    pub is_enabled: bool,
    pub volume: u8,
}

impl SoundSettings {
    pub fn create_initial() -> Self {
        Self {
            is_enabled: true,
            volume: 50,
        }
    }
    pub fn calculate_effective_volume(self) -> u8 {
        if self.is_enabled {
            self.volume.min(100)
        } else {
            0
        }
    }
    pub fn format_volume_level(volume: u8) -> String {
        if volume == 0 {
            "Off".into()
        } else {
            format!("{volume}%")
        }
    }
    pub fn select_volume(&mut self, volume: u8) {
        self.is_enabled = volume > 0;
        if self.is_enabled {
            self.volume = volume;
        }
    }
    pub fn list_volume_levels(self) -> Vec<u8> {
        let mut levels = vec![0, 10, 25, 50, 75, 100, self.volume];
        levels.sort_unstable();
        levels.dedup();
        levels
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProseSettings {
    pub capitals: bool,
    pub punctuation: bool,
    pub books: Vec<String>,
}

impl ProseSettings {
    pub fn create_initial() -> Self {
        Self {
            capitals: true,
            punctuation: true,
            books: vec![],
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CodeSettings {
    pub languages: Vec<String>,
}

impl CodeSettings {
    pub fn create_initial() -> Self {
        Self { languages: vec![] }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub mode: PracticeMode,
    #[serde(deserialize_with = "Option::<u64>::deserialize")]
    pub seconds: Option<u64>,
    pub theme: String,
    pub sound: SoundSettings,
    pub prose: ProseSettings,
    pub code: CodeSettings,
}

impl Settings {
    pub fn create_initial() -> Self {
        Self {
            mode: PracticeMode::Prose,
            seconds: Some(30),
            theme: "catppuccin-mocha".into(),
            sound: SoundSettings::create_initial(),
            prose: ProseSettings::create_initial(),
            code: CodeSettings::create_initial(),
        }
    }

    pub const TIMER_OPTIONS: [Option<u64>; 4] = [None, Some(15), Some(30), Some(60)];

    pub fn format_timer(&self) -> String {
        self.seconds
            .map_or_else(|| "Off".into(), |seconds| format!("{seconds} seconds"))
    }

    pub fn matches_exercise(&self, other: &Self) -> bool {
        self.mode == other.mode
            && self.seconds == other.seconds
            && match self.mode {
                PracticeMode::Prose => self.prose == other.prose,
                PracticeMode::Code => self.code == other.code,
            }
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            Self::TIMER_OPTIONS.contains(&self.seconds),
            "Timer must be off (null), 15, 30, or 60 seconds"
        );
        ensure!(
            self.sound.volume <= 100,
            "Sound volume must be between 0 and 100"
        );
        for label in std::iter::once(&self.theme)
            .chain(&self.prose.books)
            .chain(&self.code.languages)
        {
            ensure!(
                !label.trim().is_empty() && !label.chars().any(char::is_control),
                "Theme and filter names must be nonempty and contain no terminal controls"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub settings: Settings,
    #[serde(skip)]
    pub managed: ManagedSettings,
    #[serde(skip)]
    original_preferences: Option<Settings>,
}

impl Configuration {
    pub fn create_initial() -> Self {
        Self {
            settings: Settings::create_initial(),
            managed: ManagedSettings::default(),
            original_preferences: None,
        }
    }
    pub fn load(directory: &Path) -> Result<Self> {
        let path = directory.join("config.json");
        let mut configuration: Self = if path.exists() {
            serde_json::from_str(&fs::read_to_string(&path)?)
                .with_context(|| format!("Read {} (fix or rename this file)", path.display()))?
        } else {
            Self::create_initial()
        };
        configuration.settings.validate()?;
        configuration.original_preferences = Some(configuration.settings.clone());
        configuration.managed = ManagedSettings::load(directory)?;
        configuration.settings = configuration.managed.apply(&configuration.settings)?;
        Ok(configuration)
    }
    pub fn save(&self, directory: &Path) -> Result<()> {
        self.settings.validate()?;
        let preferences = self.managed.restore_preferences(PreferenceSave {
            effective: &self.settings,
            original: self.original_preferences.as_ref().unwrap_or(&self.settings),
        })?;
        fs::create_dir_all(directory)?;
        let temporary_path = directory.join("config.json.tmp");
        fs::write(
            &temporary_path,
            serde_json::to_string_pretty(&serde_json::json!({ "settings": preferences }))?,
        )?;
        fs::rename(temporary_path, directory.join("config.json"))?;
        Ok(())
    }
}

pub fn resolve_config_directory() -> Result<PathBuf> {
    if let Some(directory) = std::env::var_os("CHIVAVA_CONFIG_DIR") {
        ensure!(
            !directory.is_empty(),
            "CHIVAVA_CONFIG_DIR must not be empty"
        );
        return Ok(PathBuf::from(directory));
    }
    if let Some(directory) = std::env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(directory).join("chivava"));
    }
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .context("Set HOME, XDG_CONFIG_HOME, or CHIVAVA_CONFIG_DIR")?;
    Ok(PathBuf::from(home).join(".config/chivava"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sound_off_preserves_the_saved_level_and_positive_percentages_enable_playback() {
        let mut sound = SoundSettings {
            is_enabled: true,
            volume: 37,
        };
        assert_eq!(sound.list_volume_levels(), [0, 10, 25, 37, 50, 75, 100]);
        assert_eq!(
            SoundSettings::format_volume_level(sound.calculate_effective_volume()),
            "37%"
        );
        sound.select_volume(0);
        assert!(!sound.is_enabled);
        assert_eq!(sound.volume, 37);
        assert_eq!(
            SoundSettings::format_volume_level(sound.calculate_effective_volume()),
            "Off"
        );
        sound.select_volume(25);
        assert!(sound.is_enabled);
        assert_eq!(sound.calculate_effective_volume(), 25);
        assert_eq!(sound.list_volume_levels(), [0, 10, 25, 50, 75, 100]);
    }

    #[test]
    fn persists_independent_mode_and_audio_settings() {
        let directory = tempfile::tempdir().unwrap();
        let mut configuration = Configuration::create_initial();
        configuration.settings.prose.punctuation = false;
        configuration.settings.seconds = None;
        configuration.settings.code.languages = vec!["Rust".into()];
        configuration.settings.sound = SoundSettings {
            is_enabled: false,
            volume: 25,
        };
        configuration.save(directory.path()).unwrap();
        let loaded = Configuration::load(directory.path()).unwrap();
        assert_eq!(loaded.settings, configuration.settings);
        assert_eq!(loaded.settings.sound.calculate_effective_volume(), 0);
    }

    #[test]
    fn configuration_requires_every_field_and_rejects_unknown_fields() {
        let initial = serde_json::to_value(Configuration::create_initial()).unwrap();
        for pointer in [
            "",
            "/settings",
            "/settings/sound",
            "/settings/prose",
            "/settings/code",
        ] {
            let fields = initial.pointer(pointer).unwrap().as_object().unwrap();
            for key in fields.keys() {
                let mut stored = initial.clone();
                stored
                    .pointer_mut(pointer)
                    .unwrap()
                    .as_object_mut()
                    .unwrap()
                    .remove(key);
                assert!(
                    serde_json::from_value::<Configuration>(stored).is_err(),
                    "{pointer}/{key}"
                );
            }
            let mut stored = initial.clone();
            stored
                .pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("unknown".into(), serde_json::json!(true));
            assert!(
                serde_json::from_value::<Configuration>(stored).is_err(),
                "{pointer}"
            );
        }
    }

    #[test]
    fn loading_rejects_invalid_settings_without_changing_the_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        for (pointer, value) in [
            ("/settings/mode", serde_json::json!("unknown")),
            ("/settings/seconds", serde_json::json!(17)),
            ("/settings/sound/volume", serde_json::json!(101)),
            ("/settings/prose/books", serde_json::json!(["bad\u{1b}"])),
            ("/settings/code", serde_json::Value::Null),
        ] {
            let mut stored = serde_json::to_value(Configuration::create_initial()).unwrap();
            *stored.pointer_mut(pointer).unwrap() = value;
            let text = stored.to_string();
            fs::write(&path, &text).unwrap();
            assert!(Configuration::load(directory.path()).is_err(), "{pointer}");
            assert_eq!(fs::read_to_string(&path).unwrap(), text);
        }
        fs::write(&path, "not json").unwrap();
        assert!(Configuration::load(directory.path()).is_err());
    }

    #[test]
    fn timer_round_trips_with_explicit_null_for_off() {
        for seconds in Settings::TIMER_OPTIONS {
            let mut settings = Settings::create_initial();
            settings.seconds = seconds;
            let stored = serde_json::to_value(&settings).unwrap();
            assert_eq!(stored["seconds"], serde_json::json!(seconds));
            assert_eq!(
                serde_json::from_value::<Settings>(stored).unwrap(),
                settings
            );
        }
        let mut settings = Settings::create_initial();
        let timed = settings.clone();
        settings.seconds = None;
        assert!(!settings.matches_exercise(&timed));
        assert_eq!(settings.format_timer(), "Off");
    }

    #[test]
    fn appearance_audio_and_inactive_mode_changes_do_not_change_the_exercise() {
        let initial = Settings::create_initial();
        let mut changed = initial.clone();
        changed.theme = "nord".into();
        changed.sound.is_enabled = false;
        changed.code.languages = vec!["Python".into()];
        assert!(initial.matches_exercise(&changed));
        changed.prose.capitals = false;
        assert!(!initial.matches_exercise(&changed));
        changed.prose.capitals = true;
        changed.prose.punctuation = false;
        assert!(!initial.matches_exercise(&changed));
        let mut code = initial.clone();
        code.mode = PracticeMode::Code;
        changed = code.clone();
        changed.prose.punctuation = false;
        changed.prose.capitals = false;
        assert!(code.matches_exercise(&changed));
    }
}
