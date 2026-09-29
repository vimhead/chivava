use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use ratatui::style::Color;
use serde::Deserialize;

use crate::{bundled_themes::BUNDLED_THEME_JSON, syntax::SyntaxRole};

#[derive(Clone, Debug)]
pub struct Theme {
    pub name: String,
    pub selector: String,
    pub text: Color,
    pub accent: Color,
    pub success: Color,
    pub error: Color,
    pub muted: Color,
    pub dim: Color,
    pub selected_background: Color,
    syntax: SyntaxPalette,
}

#[derive(Clone, Debug)]
struct SyntaxPalette {
    default: Color,
    comment: Color,
    keyword: Color,
    function: Color,
    variable: Color,
    string: Color,
    number: Color,
    type_name: Color,
    operator: Color,
    punctuation: Color,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ThemeColor {
    Indexed(u8),
    Named(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeTheme {
    version: u32,
    name: String,
    palette: HashMap<String, ThemeColor>,
    interface: InterfaceColors,
    syntax: SyntaxColors,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InterfaceColors {
    text: ThemeColor,
    accent: ThemeColor,
    correct: ThemeColor,
    error: ThemeColor,
    muted: ThemeColor,
    pending: ThemeColor,
    selection_background: ThemeColor,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SyntaxColors {
    default: ThemeColor,
    comment: ThemeColor,
    keyword: ThemeColor,
    function: ThemeColor,
    variable: ThemeColor,
    string: ThemeColor,
    number: ThemeColor,
    #[serde(rename = "type")]
    type_name: ThemeColor,
    operator: ThemeColor,
    punctuation: ThemeColor,
}

pub struct ThemeSelection<'a> {
    pub selector: &'a str,
    pub config_directory: &'a Path,
    pub home_directory: Option<&'a Path>,
}

impl ThemeSelection<'_> {
    fn resolve_path(&self) -> Result<PathBuf> {
        ensure!(
            !self.selector.trim().is_empty() && !self.selector.chars().any(char::is_control),
            "Theme paths must be nonempty and contain no terminal controls"
        );
        if let Some(relative) = self.selector.strip_prefix("~/") {
            let home = self
                .home_directory
                .context("Set HOME or use an absolute theme path instead of ~/")?;
            return Ok(home.join(relative));
        }
        ensure!(
            !self.selector.starts_with('~'),
            "Use ~/ for home-relative theme paths"
        );
        let path = Path::new(self.selector);
        Ok(if path.is_absolute() {
            path.to_owned()
        } else {
            self.config_directory.join(path)
        })
    }
}

impl NativeTheme {
    fn resolve_color(&self, value: &ThemeColor) -> Result<Color> {
        let mut current = value;
        for _ in 0..64 {
            match current {
                ThemeColor::Indexed(index) => return Ok(Color::Indexed(*index)),
                ThemeColor::Named(name) if name == "default" => return Ok(Color::Reset),
                ThemeColor::Named(name) => {
                    ensure!(
                        !name.chars().any(char::is_control),
                        "Theme colors cannot contain terminal controls"
                    );
                    if let Some(hex) = name.strip_prefix('#') {
                        ensure!(
                            hex.len() == 6 && hex.is_ascii(),
                            "Use six-digit RGB colors: {name}"
                        );
                        let rgb = u32::from_str_radix(hex, 16)
                            .with_context(|| format!("Invalid RGB color: {name}"))?;
                        return Ok(Color::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8));
                    }
                    let reference = name.strip_prefix('$').with_context(|| format!(
                        "Invalid color '{name}': use #RRGGBB, $palette-name, an ANSI index, or default"))?;
                    current = self
                        .palette
                        .get(reference)
                        .with_context(|| format!("Unknown palette reference: ${reference}"))?;
                }
            }
        }
        bail!("Palette alias cycle or more than 64 nested references")
    }

    fn resolve_interface(&self, field: &str, color: &ThemeColor) -> Result<Color> {
        self.resolve_color(color)
            .with_context(|| format!("interface.{field}"))
    }

    fn resolve_syntax(&self, field: &str, color: &ThemeColor) -> Result<Color> {
        self.resolve_color(color)
            .with_context(|| format!("syntax.{field}"))
    }
}

impl Theme {
    pub fn parse(json: &str) -> Result<Self> {
        let source: NativeTheme = serde_json::from_str(json).map_err(|error| {
            anyhow::anyhow!(
                "Invalid Chivava theme JSON: {}",
                error.to_string().escape_debug()
            )
        })?;
        ensure!(
            source.version == 1,
            "Unsupported Chivava theme version {}; expected 1",
            source.version
        );
        ensure!(
            !source.name.trim().is_empty() && !source.name.chars().any(char::is_control),
            "Theme names must be nonempty and contain no terminal controls"
        );
        for (name, color) in &source.palette {
            ensure!(
                !name.is_empty()
                    && name
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric()
                            || matches!(character, '_' | '-' | '.')),
                "Invalid palette name: {name:?}"
            );
            source
                .resolve_color(color)
                .with_context(|| format!("palette.{name}"))?;
        }
        Ok(Self {
            name: source.name.clone(),
            selector: source.name.clone(),
            text: source.resolve_interface("text", &source.interface.text)?,
            accent: source.resolve_interface("accent", &source.interface.accent)?,
            success: source.resolve_interface("correct", &source.interface.correct)?,
            error: source.resolve_interface("error", &source.interface.error)?,
            muted: source.resolve_interface("muted", &source.interface.muted)?,
            dim: source.resolve_interface("pending", &source.interface.pending)?,
            selected_background: source.resolve_interface(
                "selection_background",
                &source.interface.selection_background,
            )?,
            syntax: SyntaxPalette {
                default: source.resolve_syntax("default", &source.syntax.default)?,
                comment: source.resolve_syntax("comment", &source.syntax.comment)?,
                keyword: source.resolve_syntax("keyword", &source.syntax.keyword)?,
                function: source.resolve_syntax("function", &source.syntax.function)?,
                variable: source.resolve_syntax("variable", &source.syntax.variable)?,
                string: source.resolve_syntax("string", &source.syntax.string)?,
                number: source.resolve_syntax("number", &source.syntax.number)?,
                type_name: source.resolve_syntax("type", &source.syntax.type_name)?,
                operator: source.resolve_syntax("operator", &source.syntax.operator)?,
                punctuation: source.resolve_syntax("punctuation", &source.syntax.punctuation)?,
            },
        })
    }

    pub fn resolve_syntax_color(&self, role: SyntaxRole) -> Color {
        match role {
            SyntaxRole::Plain => self.syntax.default,
            SyntaxRole::Comment => self.syntax.comment,
            SyntaxRole::Keyword => self.syntax.keyword,
            SyntaxRole::Function => self.syntax.function,
            SyntaxRole::Variable => self.syntax.variable,
            SyntaxRole::String => self.syntax.string,
            SyntaxRole::Number => self.syntax.number,
            SyntaxRole::Type => self.syntax.type_name,
            SyntaxRole::Operator => self.syntax.operator,
            SyntaxRole::Punctuation => self.syntax.punctuation,
        }
    }

    pub fn format_label(&self) -> String {
        if self.selector == self.name {
            self.name.clone()
        } else {
            format!("file: {}", self.name)
        }
    }

    pub fn load_bundled() -> Result<Vec<Self>> {
        BUNDLED_THEME_JSON
            .iter()
            .map(|json| Self::parse(json))
            .collect()
    }

    pub fn load_selected(themes: &mut Vec<Self>, selection: ThemeSelection<'_>) -> Result<()> {
        if themes
            .iter()
            .any(|theme| theme.selector == selection.selector)
        {
            return Ok(());
        }
        let path = selection.resolve_path()?;
        let json = fs::read_to_string(&path).with_context(|| {
            format!(
                "Read theme {}. Use chivava themes list for built-in names",
                path.display()
            )
        })?;
        let mut theme =
            Self::parse(&json).with_context(|| format!("Load native theme {}", path.display()))?;
        theme.selector = selection.selector.to_owned();
        themes.push(theme);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn sample() -> Value {
        serde_json::from_str(include_str!("../assets/themes/catppuccin-mocha.json")).unwrap()
    }

    #[test]
    fn all_forty_eight_native_themes_are_embedded_and_distinct() {
        let themes = Theme::load_bundled().unwrap();
        assert_eq!(themes.len(), 48);
        let names: std::collections::HashSet<_> =
            themes.iter().map(|theme| theme.name.as_str()).collect();
        assert_eq!(names.len(), 48);
        for name in [
            "catppuccin-frappe",
            "catppuccin-macchiato",
            "everforest-light-soft",
            "gruvbox-material",
            "rose-pine-dawn",
            "tokyo-night-moon",
            "white",
        ] {
            assert!(names.contains(name));
        }
        assert_eq!(themes[0].name, "catppuccin-mocha");
        assert_eq!(themes[1].name, "catppuccin-latte");
        assert_eq!(themes[2].name, "nord");
        assert_eq!(themes[0].accent, Color::Rgb(203, 166, 247));
        assert_eq!(themes[0].selected_background, Color::Rgb(49, 50, 68));
        assert_eq!(
            themes[0].resolve_syntax_color(SyntaxRole::Function),
            Color::Rgb(137, 180, 250)
        );
        assert_eq!(
            themes[0].resolve_syntax_color(SyntaxRole::Number),
            Color::Rgb(250, 179, 135)
        );
        let files: Vec<_> =
            fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/themes"))
                .unwrap()
                .collect();
        assert_eq!(files.len(), themes.len());
    }

    #[test]
    fn native_colors_support_explicit_aliases_ansi_and_terminal_defaults() {
        let mut source = sample();
        source["palette"]["first"] = json!("$second");
        source["palette"]["second"] = json!(123);
        source["syntax"]["keyword"] = json!("$first");
        source["syntax"]["default"] = json!("default");
        let theme = Theme::parse(&source.to_string()).unwrap();
        assert_eq!(
            theme.resolve_syntax_color(SyntaxRole::Keyword),
            Color::Indexed(123)
        );
        assert_eq!(theme.resolve_syntax_color(SyntaxRole::Plain), Color::Reset);
    }

    #[test]
    fn native_schema_rejects_pi_themes_unknown_fields_and_missing_roles() {
        assert!(
            Theme::parse(
                r##"{"name":"old","vars":{"accent":"#ffffff"},"colors":{"accent":"accent"}}"##
            )
            .is_err()
        );
        for (section, field) in [("interface", "pending"), ("syntax", "keyword")] {
            let mut source = sample();
            source[section].as_object_mut().unwrap().remove(field);
            assert!(Theme::parse(&source.to_string()).is_err());
        }
        let mut source = sample();
        source["colors"] = json!({});
        assert!(Theme::parse(&source.to_string()).is_err());
        source.as_object_mut().unwrap().remove("colors");
        source["version"] = json!(2);
        assert!(
            Theme::parse(&source.to_string())
                .unwrap_err()
                .to_string()
                .contains("version 2")
        );
    }

    #[test]
    fn rejects_bad_colors_cycles_and_terminal_controls_with_context() {
        for value in [
            json!(256),
            json!(-1),
            json!("#zzzzzz"),
            json!("#é1234"),
            json!("$missing"),
            json!("mauve"),
            json!(""),
        ] {
            let mut source = sample();
            source["syntax"]["keyword"] = value;
            assert!(Theme::parse(&source.to_string()).is_err());
        }
        let mut source = sample();
        source["palette"]["a"] = json!("$b");
        source["palette"]["b"] = json!("$a");
        assert!(format!("{:#}", Theme::parse(&source.to_string()).unwrap_err()).contains("cycle"));
        let mut source = sample();
        source["name"] = json!("bad\u{1b}[31m");
        assert!(Theme::parse(&source.to_string()).is_err());
        let mut source = sample();
        source["syntax"]["string"] = json!("$missing");
        let message = format!("{:#}", Theme::parse(&source.to_string()).unwrap_err());
        assert!(message.contains("syntax.string") && message.contains("$missing"));
    }

    #[test]
    fn invalid_external_theme_values_cannot_inject_terminal_controls_into_errors() {
        let mut source = sample();
        source["syntax"]["keyword"] = json!("\u{1b}[2J");
        let error = format!("{:#}", Theme::parse(&source.to_string()).unwrap_err());
        assert!(!error.contains('\u{1b}'));
        source["syntax"]["keyword"] = json!("$mauve");
        source["\u{1b}[2J"] = json!("unknown key");
        let error = format!("{:#}", Theme::parse(&source.to_string()).unwrap_err());
        assert!(!error.contains('\u{1b}'));
    }

    #[test]
    fn file_paths_are_config_relative_or_explicitly_home_relative_and_cannot_shadow_builtins() {
        let directory = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("custom.json"), sample().to_string()).unwrap();
        fs::write(home.path().join("home.json"), sample().to_string()).unwrap();
        let absolute = directory
            .path()
            .join("custom.json")
            .to_string_lossy()
            .into_owned();
        for selector in [
            "custom.json",
            "./custom.json",
            "~/home.json",
            absolute.as_str(),
        ] {
            let mut themes = Theme::load_bundled().unwrap();
            Theme::load_selected(
                &mut themes,
                ThemeSelection {
                    selector,
                    config_directory: directory.path(),
                    home_directory: Some(home.path()),
                },
            )
            .unwrap();
            assert_eq!(themes.len(), 49);
            let custom = themes.last().unwrap();
            assert_eq!(custom.name, "catppuccin-mocha");
            assert_eq!(custom.selector, selector);
            assert_eq!(custom.format_label(), "file: catppuccin-mocha");
            assert_eq!(themes[0].selector, "catppuccin-mocha");
        }
        let mut themes = Theme::load_bundled().unwrap();
        for selector in [
            "missing",
            "missing.json",
            "~other/theme.json",
            "~/home.json",
        ] {
            assert!(
                Theme::load_selected(
                    &mut themes,
                    ThemeSelection {
                        selector,
                        config_directory: directory.path(),
                        home_directory: None
                    }
                )
                .is_err()
            );
        }
        fs::write(directory.path().join("bad.json"), "{}").unwrap();
        let error = Theme::load_selected(
            &mut themes,
            ThemeSelection {
                selector: "bad.json",
                config_directory: directory.path(),
                home_directory: None,
            },
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("bad.json"));
    }
}
