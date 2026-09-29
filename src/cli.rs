use std::path::{Path, PathBuf};

use anyhow::{Result, bail, ensure};
use clap::{Subcommand, ValueEnum};

use crate::{
    content::Corpus,
    licenses,
    settings::{self, Configuration, PracticeMode, SoundSettings, TextType},
    theme::{Theme, ThemeSelection},
};

#[derive(Subcommand)]
pub enum Command {
    /// Inspect or change the settings shared with F2.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Discover built-in themes.
    Themes {
        #[command(subcommand)]
        command: ThemesCommand,
    },
    /// Discover available sources or import local excerpts.
    Sources {
        #[command(subcommand)]
        command: SourcesCommand,
    },
    /// Print bundled content, audio, theme and dependency license notices.
    Licenses,
}

#[derive(Subcommand)]
pub enum ConfigCommand {
    /// Show effective settings, including defaults and managed values.
    List,
    /// Validate and immediately save one setting.
    #[command(
        after_help = "Values:\n  mode: prose or code\n  seconds: off, 15, 30, or 60\n  theme: built-in name or native JSON path (config-relative, absolute, or ~/)\n  sound, prose.capitals, prose.punctuation: on or off\n  volume: 0..100\n  code.languages, prose.books: comma-separated source names, or all\n\nUse sources languages, sources books, and themes list to discover names.\nQuote values containing spaces. A full matching source name is treated as one item, even if it contains commas.\nWith unmanaged audio, positive volume enables sound; 0 mutes and remembers the level.\nSound on restores the remembered volume (50 if zero and volume is unmanaged).\nNix-managed fields are read-only; changing one audio field never changes a managed field.\nInvalid values leave the configuration unchanged."
    )]
    Set {
        #[arg(value_enum)]
        key: ConfigKey,
        value: String,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum ConfigKey {
    Mode,
    Seconds,
    Theme,
    Sound,
    Volume,
    #[value(name = "prose.capitals")]
    ProseCapitals,
    #[value(name = "prose.punctuation")]
    ProsePunctuation,
    #[value(name = "code.languages")]
    CodeLanguages,
    #[value(name = "prose.books")]
    ProseBooks,
}

impl ConfigKey {
    fn resolve_setting_path(self) -> &'static str {
        match self {
            Self::Mode => "mode",
            Self::Seconds => "seconds",
            Self::Theme => "theme",
            Self::Sound => "sound.is_enabled",
            Self::Volume => "sound.volume",
            Self::ProseCapitals => "prose.capitals",
            Self::ProsePunctuation => "prose.punctuation",
            Self::CodeLanguages => "code.languages",
            Self::ProseBooks => "prose.books",
        }
    }
}

#[derive(Subcommand)]
pub enum ThemesCommand {
    /// List built-in theme names.
    List,
}

#[derive(Subcommand)]
pub enum SourcesCommand {
    /// List bundled and imported code languages.
    Languages,
    /// List bundled and imported book titles.
    Books,
    /// Append a JSON array of local book excerpts or code without starting typing.
    #[command(after_help = concat!("Import book or code passages as a JSON array. Only import text you are entitled to use.\nExample JSON:\n", include_str!("../examples/local-corpus.json")))]
    Import { file: PathBuf },
}

impl Command {
    pub fn execute(self) -> Result<()> {
        match self {
            Self::Licenses => licenses::print_notices(),
            Self::Themes {
                command: ThemesCommand::List,
            } => {
                for theme in Theme::load_bundled()? {
                    println!("{}", theme.name);
                }
            }
            Self::Config { command } => {
                let directory = settings::resolve_config_directory()?;
                let home = std::env::var_os("HOME").map(PathBuf::from);
                let editor = ConfigurationEditor {
                    directory: &directory,
                    home_directory: home.as_deref(),
                };
                match command {
                    ConfigCommand::List => editor.print_settings()?,
                    ConfigCommand::Set { key, value } => {
                        editor.save_setting(SettingChange { key, value: &value })?
                    }
                }
            }
            Self::Sources { command } => {
                let directory = settings::resolve_config_directory()?;
                match command {
                    SourcesCommand::Import { file } => {
                        let count = Corpus::import(&directory, &file)?;
                        println!(
                            "Imported {count} passages into {}",
                            directory.join("corpus.json").display()
                        );
                    }
                    SourcesCommand::Languages | SourcesCommand::Books => {
                        let text_type = match command {
                            SourcesCommand::Languages => TextType::Code,
                            _ => TextType::Books,
                        };
                        for group in Corpus::load(&directory)?.list_groups(text_type) {
                            println!("{group}");
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

struct SettingChange<'a> {
    key: ConfigKey,
    value: &'a str,
}

struct ConfigurationEditor<'a> {
    directory: &'a Path,
    home_directory: Option<&'a Path>,
}

impl ConfigurationEditor<'_> {
    fn print_settings(&self) -> Result<()> {
        let configuration = Configuration::load(self.directory)?;
        let settings = &configuration.settings;
        let fields = [
            (
                ConfigKey::Mode,
                match settings.mode {
                    PracticeMode::Prose => "prose",
                    PracticeMode::Code => "code",
                }
                .to_owned(),
            ),
            (
                ConfigKey::Seconds,
                settings
                    .seconds
                    .map_or_else(|| "off".into(), |seconds| seconds.to_string()),
            ),
            (ConfigKey::Theme, settings.theme.clone()),
            (
                ConfigKey::Sound,
                format_toggle(settings.sound.is_enabled).to_owned(),
            ),
            (ConfigKey::Volume, settings.sound.volume.to_string()),
            (
                ConfigKey::ProseCapitals,
                format_toggle(settings.prose.capitals).to_owned(),
            ),
            (
                ConfigKey::ProsePunctuation,
                format_toggle(settings.prose.punctuation).to_owned(),
            ),
            (
                ConfigKey::CodeLanguages,
                format_sources(&settings.code.languages),
            ),
            (ConfigKey::ProseBooks, format_sources(&settings.prose.books)),
        ];
        for (key, value) in fields {
            let label = key.to_possible_value().unwrap();
            let ownership = if configuration.managed.is_managed(key.resolve_setting_path()) {
                " [Nix]"
            } else {
                ""
            };
            println!("{} = {value}{ownership}", label.get_name());
        }
        Ok(())
    }

    fn save_setting(&self, change: SettingChange<'_>) -> Result<()> {
        let mut configuration = Configuration::load(self.directory)?;
        configuration
            .managed
            .ensure_writable(change.key.resolve_setting_path())?;
        let is_sound_managed = configuration.managed.is_managed("sound.is_enabled");
        let is_volume_managed = configuration.managed.is_managed("sound.volume");
        let settings = &mut configuration.settings;
        let value = change.value;
        match change.key {
            ConfigKey::Mode => {
                settings.mode = match value {
                    "prose" => PracticeMode::Prose,
                    "code" => PracticeMode::Code,
                    _ => bail!("Mode must be prose or code"),
                }
            }
            ConfigKey::Seconds => {
                settings.seconds = match value {
                    "off" => None,
                    "15" => Some(15),
                    "30" => Some(30),
                    "60" => Some(60),
                    _ => bail!("Timer: choose off, 15, 30, or 60"),
                }
            }
            ConfigKey::Theme => {
                Theme::load_selected(
                    &mut Theme::load_bundled()?,
                    ThemeSelection {
                        selector: value,
                        config_directory: self.directory,
                        home_directory: self.home_directory,
                    },
                )?;
                settings.theme = value.to_owned();
            }
            ConfigKey::Sound => {
                settings.sound.is_enabled = parse_toggle(value)?;
                if settings.sound.is_enabled && settings.sound.volume == 0 && !is_volume_managed {
                    settings.sound.volume = SoundSettings::create_initial().volume;
                }
            }
            ConfigKey::Volume => {
                let volume = value.parse::<u8>().ok().filter(|volume| *volume <= 100);
                let Some(volume) = volume else {
                    bail!("Volume must be an integer between 0 and 100");
                };
                if is_sound_managed {
                    settings.sound.volume = volume;
                } else {
                    settings.sound.select_volume(volume);
                }
            }
            ConfigKey::ProseCapitals => settings.prose.capitals = parse_toggle(value)?,
            ConfigKey::ProsePunctuation => settings.prose.punctuation = parse_toggle(value)?,
            ConfigKey::CodeLanguages => {
                settings.code.languages = self.parse_sources(SourceSelection {
                    value,
                    text_type: TextType::Code,
                })?
            }
            ConfigKey::ProseBooks => {
                settings.prose.books = self.parse_sources(SourceSelection {
                    value,
                    text_type: TextType::Books,
                })?
            }
        }
        configuration.save(self.directory)?;
        println!("Settings saved.");
        Ok(())
    }

    fn parse_sources(&self, selection: SourceSelection<'_>) -> Result<Vec<String>> {
        let value = selection.value.trim();
        if value == "all" {
            return Ok(vec![]);
        }
        let choices = Corpus::load(self.directory)?.list_groups(selection.text_type);
        let names = if choices.iter().any(|choice| choice == value) {
            vec![value]
        } else {
            value.split(',').map(str::trim).collect()
        };
        let mut selected = vec![];
        for name in names {
            ensure!(
                !name.is_empty(),
                "Source names cannot be empty; use all to select every source"
            );
            ensure!(
                choices.iter().any(|choice| choice == name),
                "Unknown source {name:?}; use chivava sources {} to list valid names",
                match selection.text_type {
                    TextType::Code => "languages",
                    TextType::Books => "books",
                }
            );
            if !selected.iter().any(|selected| selected == name) {
                selected.push(name.to_owned());
            }
        }
        if selected.len() == choices.len() {
            selected.clear();
        }
        Ok(selected)
    }
}

struct SourceSelection<'a> {
    value: &'a str,
    text_type: TextType,
}

fn parse_toggle(value: &str) -> Result<bool> {
    match value {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => bail!("Choose on or off"),
    }
}

fn format_toggle(is_enabled: bool) -> &'static str {
    if is_enabled { "on" } else { "off" }
}

fn format_sources(sources: &[String]) -> String {
    if sources.is_empty() {
        "all".into()
    } else {
        sources.join(",")
    }
}
