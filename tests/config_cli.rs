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
    let output = run(directory, arguments);
    assert!(
        output.status.success(),
        "{arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn load_settings(directory: &Path) -> Value {
    serde_json::from_slice::<Value>(&fs::read(directory.join("config.json")).unwrap()).unwrap()["settings"].clone()
}

#[test]
fn config_list_prints_defaults_without_creating_files() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing");
    let text = succeed(&missing, &["config", "list"]);
    assert_eq!(
        text,
        "mode = prose\nseconds = 30\ntheme = catppuccin-mocha\nsound = on\nvolume = 50\nprose.capitals = on\nprose.punctuation = on\ncode.languages = all\nprose.books = all\n"
    );
    assert!(!missing.exists());
}

#[test]
fn every_config_key_persists_and_list_uses_the_same_names() {
    let directory = tempfile::tempdir().unwrap();
    for (key, value) in [
        ("mode", "code"),
        ("seconds", "off"),
        ("theme", "nord"),
        ("volume", "37"),
        ("sound", "off"),
        ("prose.capitals", "off"),
        ("prose.punctuation", "off"),
        ("code.languages", "Rust, Python,Rust"),
        ("prose.books", "Jane Eyre,Pride and Prejudice"),
    ] {
        succeed(directory.path(), &["config", "set", key, value]);
    }
    let settings = load_settings(directory.path());
    assert_eq!(
        settings,
        json!({
            "mode": "code", "seconds": null, "theme": "nord",
            "sound": {"is_enabled": false, "volume": 37},
            "prose": {"capitals": false, "punctuation": false, "books": ["Jane Eyre", "Pride and Prejudice"]},
            "code": {"languages": ["Rust", "Python"]}
        })
    );
    let text = succeed(directory.path(), &["config", "list"]);
    for line in [
        "mode = code",
        "seconds = off",
        "theme = nord",
        "sound = off",
        "volume = 37",
        "prose.capitals = off",
        "prose.punctuation = off",
        "code.languages = Rust,Python",
        "prose.books = Jane Eyre,Pride and Prejudice",
    ] {
        assert!(text.lines().any(|candidate| candidate == line), "{text}");
    }
    for seconds in ["15", "30", "60", "off"] {
        succeed(directory.path(), &["config", "set", "seconds", seconds]);
        let stored = load_settings(directory.path());
        assert_eq!(
            stored["seconds"],
            seconds
                .parse::<u64>()
                .map_or(Value::Null, |seconds| json!(seconds))
        );
        assert_eq!(stored["code"], settings["code"]);
    }
    for key in ["code.languages", "prose.books"] {
        succeed(directory.path(), &["config", "set", key, "all"]);
    }
    let settings = load_settings(directory.path());
    assert_eq!(settings["code"]["languages"], json!([]));
    assert_eq!(settings["prose"]["books"], json!([]));
}

#[test]
fn invalid_keys_and_values_never_modify_or_create_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing");
    let invalid = [
        ("unknown", "value"),
        ("mode", "text"),
        ("mode", "quotes"),
        ("seconds", "17"),
        ("seconds", "0"),
        ("seconds", "null"),
        ("volume", "101"),
        ("volume", "-1"),
        ("volume", "1.5"),
        ("volume", "loud"),
        ("sound", "true"),
        ("prose.capitals", "false"),
        ("prose.punctuation", "yes"),
        ("theme", "does-not-exist.json"),
        ("theme", ""),
        ("theme", "\u{1b}[31m"),
        ("code.languages", "Python,missing"),
        ("code.languages", "Rust,"),
        ("code.languages", "all,Rust"),
        ("code.languages", ""),
        ("prose.books", "missing"),
        ("prose.books", " , "),
    ];
    for (key, value) in invalid {
        assert!(
            !run(&missing, &["config", "set", key, value])
                .status
                .success()
        );
        assert!(!missing.exists(), "{key} {value:?}");
    }
    succeed(directory.path(), &["config", "set", "theme", "nord"]);
    let before = fs::read(directory.path().join("config.json")).unwrap();
    for (key, value) in invalid {
        assert!(
            !run(directory.path(), &["config", "set", key, value])
                .status
                .success()
        );
        assert_eq!(
            fs::read(directory.path().join("config.json")).unwrap(),
            before
        );
        assert!(!directory.path().join("config.json.tmp").exists());
    }
}

#[test]
fn sound_and_volume_follow_f2_mute_and_restore_semantics() {
    let directory = tempfile::tempdir().unwrap();
    for (key, value, is_enabled, volume) in [
        ("volume", "37", true, 37),
        ("sound", "off", false, 37),
        ("sound", "on", true, 37),
        ("volume", "0", false, 37),
        ("sound", "on", true, 37),
        ("volume", "100", true, 100),
    ] {
        succeed(directory.path(), &["config", "set", key, value]);
        assert_eq!(
            load_settings(directory.path())["sound"],
            json!({"is_enabled": is_enabled, "volume": volume})
        );
    }
    let mut stored = json!({"settings": load_settings(directory.path())});
    stored["settings"]["sound"] = json!({"is_enabled": false, "volume": 0});
    fs::write(directory.path().join("config.json"), stored.to_string()).unwrap();
    succeed(directory.path(), &["config", "set", "sound", "on"]);
    assert_eq!(
        load_settings(directory.path())["sound"],
        json!({"is_enabled": true, "volume": 50})
    );
}

#[test]
fn sources_include_local_imports_and_allow_names_with_commas() {
    let directory = tempfile::tempdir().unwrap();
    assert_eq!(
        succeed(directory.path(), &["sources", "languages"]),
        "GDScript\nGo\nPython\nRust\nTypeScript\n"
    );
    assert_eq!(
        succeed(directory.path(), &["sources", "books"])
            .lines()
            .count(),
        20
    );
    let imports = json!([
        {"text_type": "books", "group": "One, Two", "title": "Local book", "author": "Test", "source": "Local", "license": "CC0", "text": "One complete sentence."},
        {"text_type": "code", "group": "JavaScript", "title": "Local code", "author": "Test", "source": "Local", "license": "CC0", "text": "const value = 42;"}
    ]);
    let file = directory.path().join("input.json");
    fs::write(&file, imports.to_string()).unwrap();
    succeed(
        directory.path(),
        &["sources", "import", file.to_str().unwrap()],
    );
    assert!(succeed(directory.path(), &["sources", "languages"]).contains("JavaScript\n"));
    assert!(succeed(directory.path(), &["sources", "books"]).contains("One, Two\n"));
    succeed(
        directory.path(),
        &["config", "set", "prose.books", "One, Two"],
    );
    succeed(
        directory.path(),
        &["config", "set", "code.languages", "JavaScript"],
    );
    assert_eq!(
        load_settings(directory.path())["prose"]["books"],
        json!(["One, Two"])
    );
    assert_eq!(
        load_settings(directory.path())["code"]["languages"],
        json!(["JavaScript"])
    );
    let before = fs::read(directory.path().join("corpus.json")).unwrap();
    succeed(directory.path(), &["config", "set", "mode", "code"]);
    assert_eq!(
        fs::read(directory.path().join("corpus.json")).unwrap(),
        before
    );
}

#[test]
fn selecting_every_language_normalizes_to_all() {
    let directory = tempfile::tempdir().unwrap();
    succeed(
        directory.path(),
        &[
            "config",
            "set",
            "code.languages",
            "Rust,Go,Python,GDScript,TypeScript",
        ],
    );
    assert_eq!(
        load_settings(directory.path())["code"]["languages"],
        json!([])
    );
}

#[test]
fn every_command_has_help_and_missing_arguments_do_not_start_typing() {
    let directory = tempfile::tempdir().unwrap();
    for command in [
        vec![],
        vec!["config"],
        vec!["config", "list"],
        vec!["config", "set"],
        vec!["themes"],
        vec!["themes", "list"],
        vec!["sources"],
        vec!["sources", "languages"],
        vec!["sources", "books"],
        vec!["sources", "import"],
        vec!["licenses"],
    ] {
        let mut arguments = command.clone();
        arguments.push("--help");
        let help = succeed(directory.path(), &arguments);
        assert!(help.contains("Usage:"), "{command:?}: {help}");
    }
    for command in [
        vec!["config"],
        vec!["themes"],
        vec!["sources"],
        vec!["config", "set"],
        vec!["config", "set", "mode"],
        vec!["sources", "import"],
    ] {
        assert_eq!(run(directory.path(), &command).status.code(), Some(2));
    }
    assert!(succeed(directory.path(), &["--version"]).starts_with("chivava "));
    assert!(!directory.path().join("config.json").exists());
}

#[test]
fn discovery_and_licenses_do_not_depend_on_valid_settings_or_open_a_terminal() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("config.json"),
        "invalid configuration",
    )
    .unwrap();
    for command in [
        vec!["themes", "list"],
        vec!["sources", "languages"],
        vec!["sources", "books"],
        vec!["licenses"],
    ] {
        succeed(directory.path(), &command);
    }
    assert!(!run(directory.path(), &["config", "list"]).status.success());
    assert!(
        !run(directory.path(), &["config", "set", "theme", "nord"])
            .status
            .success()
    );
    assert_eq!(
        fs::read_to_string(directory.path().join("config.json")).unwrap(),
        "invalid configuration"
    );
}

#[test]
fn config_directory_environment_precedence_and_empty_values_are_safe() {
    let directory = tempfile::tempdir().unwrap();
    let explicit = directory.path().join("explicit");
    let xdg = directory.path().join("xdg");
    let home = directory.path().join("home");
    for (override_value, xdg_value, expected) in [
        (
            Some(explicit.as_path()),
            Some(xdg.as_path()),
            explicit.clone(),
        ),
        (None, Some(xdg.as_path()), xdg.join("chivava")),
        (None, None, home.join(".config/chivava")),
        (None, Some(Path::new("")), home.join(".config/chivava")),
    ] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_chivava"));
        command
            .env_remove("CHIVAVA_CONFIG_DIR")
            .env_remove("XDG_CONFIG_HOME")
            .env("HOME", &home);
        if let Some(value) = override_value {
            command.env("CHIVAVA_CONFIG_DIR", value);
        }
        if let Some(value) = xdg_value {
            command.env("XDG_CONFIG_HOME", value);
        }
        let output = command
            .args(["config", "set", "theme", "nord"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(load_settings(&expected)["theme"], "nord");
    }
    let output = Command::new(env!("CARGO_BIN_EXE_chivava"))
        .env("CHIVAVA_CONFIG_DIR", "")
        .env("HOME", &home)
        .args(["config", "set", "theme", "nord"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("CHIVAVA_CONFIG_DIR must not be empty")
    );
    let output = Command::new(env!("CARGO_BIN_EXE_chivava"))
        .env_remove("CHIVAVA_CONFIG_DIR")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("HOME")
        .args(["config", "list"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("Set HOME, XDG_CONFIG_HOME, or CHIVAVA_CONFIG_DIR")
    );
}
