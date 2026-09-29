use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use serde_json::{Value, json};

fn run(directory: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_chivava"))
        .env("CHIVAVA_CONFIG_DIR", directory)
        .args(arguments)
        .output()
        .unwrap()
}

fn succeed(directory: &Path, arguments: &[&str]) -> String {
    let result = run(directory, arguments);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}

fn read_preferences(directory: &Path) -> Value {
    serde_json::from_slice::<Value>(&fs::read(directory.join("config.json")).unwrap()).unwrap()["settings"].clone()
}

#[test]
fn every_declared_key_is_marked_and_read_only_even_without_preferences() {
    let directory = tempfile::tempdir().unwrap();
    let managed = json!({"mode":"code", "seconds":null, "theme":"nord", "sound":{"is_enabled":false,"volume":37},
        "prose":{"capitals":false,"punctuation":false,"books":["Jane Eyre"]}, "code":{"languages":["Rust"]}}).to_string();
    fs::write(directory.path().join("managed.json"), &managed).unwrap();
    let listed = succeed(directory.path(), &["config", "list"]);
    assert_eq!(
        listed
            .lines()
            .filter(|line| line.ends_with("[Nix]"))
            .count(),
        9
    );
    assert!(listed.contains("seconds = off [Nix]"));
    for key in [
        "mode",
        "seconds",
        "theme",
        "sound",
        "volume",
        "prose.capitals",
        "prose.punctuation",
        "prose.books",
        "code.languages",
    ] {
        let result = run(directory.path(), &["config", "set", key, "invalid"]);
        assert!(!result.status.success(), "{key}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("managed by Nix"));
        assert!(!directory.path().join("config.json").exists());
        assert_eq!(
            fs::read_to_string(directory.path().join("managed.json")).unwrap(),
            managed
        );
    }
}

#[test]
fn managed_values_override_but_never_replace_personal_preferences() {
    let directory = tempfile::tempdir().unwrap();
    succeed(
        directory.path(),
        &["config", "set", "theme", "catppuccin-mocha"],
    );
    succeed(directory.path(), &["config", "set", "seconds", "60"]);
    let managed = directory.path().join("managed.json");
    fs::write(&managed, r#"{"theme":"nord","seconds":null}"#).unwrap();
    let listed = succeed(directory.path(), &["config", "list"]);
    assert!(listed.contains("theme = nord [Nix]") && listed.contains("seconds = off [Nix]"));
    succeed(directory.path(), &["config", "set", "mode", "code"]);
    succeed(directory.path(), &["config", "set", "volume", "37"]);
    let stored = read_preferences(directory.path());
    assert_eq!(stored["theme"], "catppuccin-mocha");
    assert_eq!(stored["seconds"], 60);
    assert_eq!(stored["mode"], "code");
    assert_eq!(stored["sound"]["volume"], 37);
    assert_eq!(
        fs::read_to_string(&managed).unwrap(),
        r#"{"theme":"nord","seconds":null}"#
    );
    fs::remove_file(managed).unwrap();
    let listed = succeed(directory.path(), &["config", "list"]);
    assert!(listed.contains("theme = catppuccin-mocha") && listed.contains("seconds = 60"));
    assert!(!listed.contains("[Nix]"));
    succeed(directory.path(), &["config", "set", "theme", "nord"]);
}

#[test]
fn explicit_null_is_managed_but_omitted_and_empty_settings_are_not() {
    let directory = tempfile::tempdir().unwrap();
    let managed = directory.path().join("managed.json");
    fs::write(&managed, "{}").unwrap();
    assert!(!succeed(directory.path(), &["config", "list"]).contains("[Nix]"));
    fs::write(&managed, r#"{"seconds":null}"#).unwrap();
    succeed(directory.path(), &["config", "set", "theme", "nord"]);
    assert_eq!(read_preferences(directory.path())["seconds"], 30);
    assert!(succeed(directory.path(), &["config", "list"]).contains("seconds = off [Nix]"));
    fs::write(&managed, r#"{"sound":{},"prose":{},"code":{}}"#).unwrap();
    assert!(!succeed(directory.path(), &["config", "list"]).contains("[Nix]"));
    succeed(directory.path(), &["config", "set", "seconds", "15"]);
}

#[test]
fn managed_audio_properties_do_not_lock_or_change_the_other_property() {
    let directory = tempfile::tempdir().unwrap();
    let managed = directory.path().join("managed.json");
    fs::write(&managed, r#"{"sound":{"is_enabled":false}}"#).unwrap();
    for volume in ["37", "0", "100"] {
        succeed(directory.path(), &["config", "set", "volume", volume]);
        let listed = succeed(directory.path(), &["config", "list"]);
        assert!(listed.contains("sound = off [Nix]"));
        assert!(
            listed
                .lines()
                .any(|line| line == format!("volume = {volume}"))
        );
        assert_eq!(
            read_preferences(directory.path())["sound"]["is_enabled"],
            true
        );
    }
    fs::write(&managed, r#"{"sound":{"volume":0}}"#).unwrap();
    for sound in ["off", "on"] {
        succeed(directory.path(), &["config", "set", "sound", sound]);
        let listed = succeed(directory.path(), &["config", "list"]);
        assert!(listed.contains("volume = 0 [Nix]"));
        assert!(
            listed
                .lines()
                .any(|line| line == format!("sound = {sound}"))
        );
        assert_eq!(read_preferences(directory.path())["sound"]["volume"], 100);
    }
}

#[test]
fn malformed_or_unknown_managed_settings_fail_without_writing_preferences() {
    let directory = tempfile::tempdir().unwrap();
    succeed(directory.path(), &["config", "set", "theme", "nord"]);
    let before = fs::read(directory.path().join("config.json")).unwrap();
    for invalid in [
        "not json",
        "[]",
        "null",
        r#"{"unknown":1}"#,
        r#"{"settings":{}}"#,
        r#"{"prose.capitals":false}"#,
        r#"{"mode":"invalid"}"#,
        r#"{"seconds":17}"#,
        r#"{"sound":false}"#,
        r#"{"sound":{"enabled":false}}"#,
        r#"{"sound":{"volume":101}}"#,
        r#"{"prose":{"books":"all"}}"#,
        r#"{"prose":{"capitals":null}}"#,
        r#"{"code":{"languages":[""]}}"#,
        r#"{"theme":""}"#,
    ] {
        fs::write(directory.path().join("managed.json"), invalid).unwrap();
        assert!(
            !run(directory.path(), &["config", "list"]).status.success(),
            "{invalid}"
        );
        assert!(
            !run(directory.path(), &["config", "set", "mode", "code"])
                .status
                .success(),
            "{invalid}"
        );
        assert_eq!(
            fs::read(directory.path().join("config.json")).unwrap(),
            before
        );
        assert!(!directory.path().join("config.json.tmp").exists());
    }
}

#[cfg(unix)]
#[test]
fn managed_store_symlinks_are_preserved_and_broken_links_are_reported() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let directory = tempfile::tempdir().unwrap();
    let store_file = directory.path().join("store.json");
    fs::write(&store_file, r#"{"theme":"nord"}"#).unwrap();
    fs::set_permissions(&store_file, fs::Permissions::from_mode(0o444)).unwrap();
    let managed = directory.path().join("managed.json");
    symlink(&store_file, &managed).unwrap();
    succeed(directory.path(), &["config", "set", "volume", "37"]);
    assert!(managed.is_symlink());
    assert_eq!(
        fs::read_to_string(&store_file).unwrap(),
        r#"{"theme":"nord"}"#
    );
    assert!(succeed(directory.path(), &["config", "list"]).contains("theme = nord [Nix]"));
    fs::remove_file(&store_file).unwrap();
    let result = run(directory.path(), &["config", "list"]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("link is broken"));
}
