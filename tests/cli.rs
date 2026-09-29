use std::process::Command;

fn create_command(directory: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_chivava"));
    command.env("CHIVAVA_CONFIG_DIR", directory);
    command
}

#[test]
fn lists_bundled_themes_without_a_terminal() {
    let directory = tempfile::tempdir().unwrap();
    let output = create_command(directory.path())
        .args(["themes", "list"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let themes = String::from_utf8(output.stdout).unwrap();
    assert!(themes.contains("catppuccin-mocha"));
    assert!(themes.contains("catppuccin-latte"));
    assert!(themes.contains("nord"));
    assert_eq!(themes.lines().count(), 48);
    assert!(themes.contains("tokyo-night-moon"));
    assert!(themes.contains("white"));
}

#[test]
fn imports_local_passages_and_preserves_existing_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let example =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/local-corpus.json");
    let mut stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&example).unwrap()).unwrap();
    stored[0]["custom_note"] = serde_json::json!("User metadata");
    let existing = stored[0].clone();
    let path = directory.path().join("corpus.json");
    std::fs::write(&path, stored.to_string()).unwrap();
    for expected_count in [2, 3] {
        let output = create_command(directory.path())
            .args(["sources", "import"])
            .arg(&example)
            .output()
            .unwrap();
        assert!(output.status.success());
        let stored: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(stored.as_array().unwrap().len(), expected_count);
        assert_eq!(stored[0], existing);
    }
}

#[test]
fn invalid_imports_and_existing_corpora_are_reported_without_writing() {
    let directory = tempfile::tempdir().unwrap();
    let example =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/local-corpus.json");
    let valid = std::fs::read_to_string(&example).unwrap();
    let existing = directory.path().join("corpus.json");
    let imported = directory.path().join("import.json");
    let mut invalid_type: serde_json::Value = serde_json::from_str(&valid).unwrap();
    invalid_type[0]["text_type"] = serde_json::json!("unknown");
    for invalid in [
        "not json".to_owned(),
        "{}".to_owned(),
        invalid_type.to_string(),
    ] {
        std::fs::write(&existing, &valid).unwrap();
        std::fs::write(&imported, &invalid).unwrap();
        let output = create_command(directory.path())
            .args(["sources", "import"])
            .arg(&imported)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("JSON array of book or code passages")
        );
        assert_eq!(std::fs::read_to_string(&existing).unwrap(), valid);

        std::fs::write(&existing, &invalid).unwrap();
        for arguments in [
            vec!["sources", "books"],
            vec!["sources", "import", example.to_str().unwrap()],
        ] {
            let output = create_command(directory.path())
                .args(arguments)
                .output()
                .unwrap();
            assert!(!output.status.success());
            assert!(!output.stderr.is_empty());
            assert_eq!(std::fs::read_to_string(&existing).unwrap(), invalid);
        }
        assert!(!directory.path().join("corpus.json.tmp").exists());
    }
}

#[test]
fn rejects_invalid_timer_and_noninteractive_typing() {
    let directory = tempfile::tempdir().unwrap();
    let output = create_command(directory.path())
        .args(["config", "set", "seconds", "17"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("choose off, 15, 30, or 60"));
    let output = create_command(directory.path()).output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("interactive terminal"));
    assert!(output.stdout.is_empty());
}

#[test]
fn help_exposes_commands_and_typing_controls_without_reading_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    std::fs::write(&path, "invalid configuration").unwrap();
    for flag in ["-h", "--help"] {
        let output = create_command(directory.path()).arg(flag).output().unwrap();
        assert!(output.status.success());
        let help = String::from_utf8(output.stdout).unwrap();
        for entry in [
            "config",
            "themes",
            "sources",
            "licenses",
            "--export-stats",
            "F1",
            "F2",
            "F5",
            "Ctrl-C",
            "CHIVAVA_CONFIG_DIR",
        ] {
            assert!(help.contains(entry), "{entry}");
        }
    }
    for command in [
        vec!["config", "list"],
        vec!["config", "set"],
        vec!["themes", "list"],
        vec!["sources", "books"],
        vec!["sources", "languages"],
        vec!["sources", "import"],
        vec!["licenses"],
    ] {
        let output = create_command(directory.path())
            .args(command)
            .arg("--help")
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("Usage:"));
    }
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "invalid configuration"
    );
}

#[test]
fn export_stats_is_only_available_for_interactive_typing() {
    let directory = tempfile::tempdir().unwrap();
    for arguments in [
        vec!["--export-stats", "stats.json", "config", "list"],
        vec!["config", "list", "--export-stats", "stats.json"],
    ] {
        let output = create_command(directory.path())
            .args(arguments)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
    }
    let path = directory.path().join("stats.json");
    let output = create_command(directory.path())
        .arg("--export-stats")
        .arg(&path)
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&output.stderr).contains("interactive terminal"));
    assert!(!path.exists());
    assert!(!directory.path().join("config.json").exists());
}

#[test]
fn loads_native_theme_files_from_the_config_directory_or_an_explicit_path() {
    let directory = tempfile::tempdir().unwrap();
    let example =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/custom-theme.json");
    std::fs::copy(&example, directory.path().join("mine.json")).unwrap();
    std::fs::copy(&example, directory.path().join("theme-file")).unwrap();
    for selector in [
        "mine.json",
        "theme-file",
        "./mine.json",
        example.to_str().unwrap(),
        "~/mine.json",
        "rose-pine-moon",
    ] {
        let output = create_command(directory.path())
            .env("HOME", directory.path())
            .args(["config", "set", "theme", selector])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{selector}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let output = create_command(directory.path())
        .args(["config", "set", "theme", "examples/custom-theme.json"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Read theme"));

    std::fs::write(directory.path().join("invalid.json"), "{}").unwrap();
    let output = create_command(directory.path())
        .args(["config", "set", "theme", "invalid.json"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Invalid Chivava theme JSON"));
}

#[test]
fn embeds_current_asset_and_dependency_licenses_in_the_executable() {
    let directory = tempfile::tempdir().unwrap();
    let output = create_command(directory.path())
        .arg("licenses")
        .output()
        .unwrap();
    assert!(output.status.success());
    let licenses = String::from_utf8(output.stdout).unwrap();
    for notice in [
        "THE SOFTWARE IS PROVIDED",
        "StrikeWhistler",
        "Single clicks Keyboard Sound",
        "https://freesound.org/people/StrikeWhistler/sounds/580786/",
        "CC0 1.0 Universal",
        "MIT License",
        "Tree-sitter Rust grammar",
        "Tree-sitter Python grammar",
        "Tree-sitter JavaScript grammar",
        "Tree-sitter TypeScript grammar",
        "Tree-sitter Go grammar",
        "Tree-sitter GDScript grammar",
        "=== Django (BSD-3-Clause) ===",
        "=== Requests (Apache-2.0) ===",
        "=== VS Code (MIT) ===",
        "=== TypeScript (Apache-2.0) ===",
        "=== Tokio (MIT) ===",
        "=== ripgrep (MIT) ===",
        "=== Go standard library (BSD-3-Clause) ===",
        "=== Kubernetes (Apache-2.0) ===",
        "=== Godot demos (MIT) ===",
        "=== Pixelorama (MIT) ===",
        "Copyright (c) 2017 Max Brunsfeld",
        "Copyright (c) 2016 Max Brunsfeld",
        "Copyright (c) 2017 Maxim Sokolov",
        "Copyright (c) 2014 Max Brunsfeld",
        "Copyright (c) 2016 Steven Fackler",
        "Copyright (c) 2015 Andrew Gallant",
        "JetBrains Mono",
        "Copyright 2020 The JetBrains Mono Project Authors",
        "SIL OPEN FONT LICENSE",
        "fontdue:",
        "ttf-parser:",
        "core_maths:",
        "libm:",
        "Copyright (c) 2024 Robert Bastian",
    ] {
        assert!(licenses.contains(notice), "{notice}");
    }
}
