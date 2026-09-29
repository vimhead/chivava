use std::{collections::BTreeMap, fs, io::ErrorKind, path::Path};

use anyhow::{Context, Result, ensure};
use serde_json::Value;

use crate::settings::Settings;

#[derive(Clone, Debug, Default)]
pub struct ManagedSettings {
    values: BTreeMap<String, Value>,
}

impl ManagedSettings {
    pub fn load(directory: &Path) -> Result<Self> {
        let path = directory.join("managed.json");
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                ensure!(
                    !path.is_symlink(),
                    "Managed settings link is broken: {}",
                    path.display()
                );
                return Ok(Self::default());
            }
            Err(error) => return Err(error).with_context(|| format!("Read {}", path.display())),
        };
        Self::parse(&text).with_context(|| format!("Read managed settings {}", path.display()))
    }

    fn parse(text: &str) -> Result<Self> {
        let root: Value = serde_json::from_str(text)?;
        let fields = root
            .as_object()
            .context("Managed settings must be an object")?;
        let mut managed = Self::default();
        for (key, value) in fields {
            if matches!(key.as_str(), "sound" | "prose" | "code") {
                let children = value
                    .as_object()
                    .with_context(|| format!("Managed {key} must be an object"))?;
                for (child, value) in children {
                    managed.insert_field(format!("{key}.{child}"), value.clone())?;
                }
            } else {
                ensure!(
                    matches!(key.as_str(), "mode" | "seconds" | "theme"),
                    "Unknown managed setting {key:?}"
                );
                managed.insert_field(key.clone(), value.clone())?;
            }
        }
        Ok(managed)
    }

    fn insert_field(&mut self, key: String, value: Value) -> Result<()> {
        ensure!(
            matches!(
                key.as_str(),
                "mode"
                    | "seconds"
                    | "theme"
                    | "sound.is_enabled"
                    | "sound.volume"
                    | "prose.capitals"
                    | "prose.punctuation"
                    | "prose.books"
                    | "code.languages"
            ),
            "Unknown managed setting {key:?}"
        );
        self.values.insert(key, value);
        Ok(())
    }

    pub fn is_managed(&self, key: &str) -> bool {
        self.values.contains_key(key)
    }

    pub fn ensure_writable(&self, key: &str) -> Result<()> {
        ensure!(
            !self.is_managed(key),
            "{key} is managed by Nix; change your declarative configuration instead"
        );
        Ok(())
    }

    pub fn apply(&self, preferences: &Settings) -> Result<Settings> {
        let mut effective = serde_json::to_value(preferences)?;
        for (key, value) in &self.values {
            *effective
                .pointer_mut(&json_pointer(key))
                .context("Unknown managed field")? = value.clone();
        }
        let settings: Settings =
            serde_json::from_value(effective).context("Invalid managed settings")?;
        settings.validate().context("Invalid managed settings")?;
        Ok(settings)
    }

    pub fn restore_preferences(&self, input: PreferenceSave<'_>) -> Result<Settings> {
        let mut writable = serde_json::to_value(input.effective)?;
        let original = serde_json::to_value(input.original)?;
        for (key, value) in &self.values {
            let pointer = json_pointer(key);
            ensure!(
                writable.pointer(&pointer) == Some(value),
                "{key} is managed by Nix; refusing to save a changed managed value"
            );
            *writable.pointer_mut(&pointer).unwrap() = original.pointer(&pointer).unwrap().clone();
        }
        let preferences: Settings = serde_json::from_value(writable)?;
        preferences.validate()?;
        Ok(preferences)
    }
}

pub struct PreferenceSave<'a> {
    pub effective: &'a Settings,
    pub original: &'a Settings,
}

fn json_pointer(key: &str) -> String {
    format!("/{}", key.replace('.', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Configuration;

    #[test]
    fn saving_rejects_managed_changes_even_when_a_caller_bypasses_the_ui() {
        let directory = tempfile::tempdir().unwrap();
        Configuration::create_initial()
            .save(directory.path())
            .unwrap();
        let before = fs::read(directory.path().join("config.json")).unwrap();
        fs::write(directory.path().join("managed.json"), r#"{"theme":"nord","sound":{"is_enabled":false,"volume":37},"code":{"languages":["Rust"]}}"#).unwrap();
        for (pointer, value) in [
            ("/theme", serde_json::json!("catppuccin-mocha")),
            ("/sound/is_enabled", serde_json::json!(true)),
            ("/sound/volume", serde_json::json!(50)),
            ("/code/languages", serde_json::json!([])),
        ] {
            let mut configuration = Configuration::load(directory.path()).unwrap();
            let mut changed = serde_json::to_value(&configuration.settings).unwrap();
            *changed.pointer_mut(pointer).unwrap() = value;
            configuration.settings = serde_json::from_value(changed).unwrap();
            assert!(
                configuration
                    .save(directory.path())
                    .unwrap_err()
                    .to_string()
                    .contains("managed by Nix")
            );
            assert_eq!(
                fs::read(directory.path().join("config.json")).unwrap(),
                before
            );
            assert!(!directory.path().join("config.json.tmp").exists());
        }
    }
}
