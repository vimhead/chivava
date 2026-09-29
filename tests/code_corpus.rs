use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::Value;

#[test]
fn every_project_has_ten_distinct_pinned_files_with_complete_attribution() {
    let passages: Vec<Value> = serde_json::from_str(include_str!("../assets/code.json")).unwrap();
    let provenance: Value =
        serde_json::from_str(include_str!("../assets/code-provenance.json")).unwrap();
    let licenses = include_str!("../assets/licenses/code-projects.txt");
    assert_eq!(provenance["version"], 2);
    assert_eq!(passages.len(), 100);
    let by_title: HashMap<_, _> = passages
        .iter()
        .map(|passage| (passage["title"].as_str().unwrap(), passage))
        .collect();
    assert_eq!(by_title.len(), 100);
    let mut seen_titles = HashSet::new();
    let mut seen_texts = HashSet::new();
    let mut language_counts = BTreeMap::new();
    let mut repositories = HashSet::new();
    let projects = provenance["projects"].as_array().unwrap();
    assert_eq!(projects.len(), 10);
    for project in projects {
        let repository = project["repository"].as_str().unwrap();
        assert!(repositories.insert(repository));
        let revision = project["revision"].as_str().unwrap();
        assert_eq!(revision.len(), 40);
        assert!(revision.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(licenses.contains(repository));
        assert!(licenses.contains(revision));
        assert!(licenses.contains(project["author"].as_str().unwrap()));
        let samples = project["samples"].as_array().unwrap();
        assert_eq!(samples.len(), 10);
        let mut paths = HashSet::new();
        for sample in samples {
            let title = sample["title"].as_str().unwrap();
            assert!(seen_titles.insert(title));
            let passage = by_title[title];
            assert_eq!(passage["text_type"], "code");
            assert_eq!(passage["group"], project["language"]);
            assert_eq!(passage["author"], project["author"]);
            assert_eq!(passage["source"], sample["source"]);
            assert!(
                passage["license"]
                    .as_str()
                    .unwrap()
                    .starts_with(project["license"].as_str().unwrap())
            );
            let path = sample["path"].as_str().unwrap();
            assert!(paths.insert(path));
            let start = sample["start_line"].as_u64().unwrap();
            let end = sample["end_line"].as_u64().unwrap();
            assert!(start > 0 && end >= start);
            assert_eq!(
                sample["source"],
                format!("https://github.com/{repository}/blob/{revision}/{path}#L{start}-L{end}")
            );
            for field in ["source_sha256", "original_excerpt_sha256", "excerpt_sha256"] {
                let hash = sample[field].as_str().unwrap();
                assert_eq!(hash.len(), 64);
                assert!(hash.bytes().all(|byte| byte.is_ascii_hexdigit()));
            }
            let text = passage["text"].as_str().unwrap();
            assert!(seen_texts.insert(text));
            assert!(text.is_ascii() && !text.contains('\r') && !text.ends_with('\n'));
            assert!(text.lines().count() as u64 <= end - start + 1);
            let mut previous_end = 0;
            let removed_spans = sample["removed_spans"].as_array().unwrap();
            for span in removed_spans {
                let start = span["start_byte"].as_u64().unwrap();
                let end = span["end_byte"].as_u64().unwrap();
                assert!(start >= previous_end && end > start);
                assert!(matches!(
                    span["kind"].as_str().unwrap(),
                    "comment" | "docstring"
                ));
                previous_end = end;
            }
            if removed_spans.is_empty() {
                assert_eq!(sample["excerpt_sha256"], sample["original_excerpt_sha256"]);
            } else {
                assert_ne!(sample["excerpt_sha256"], sample["original_excerpt_sha256"]);
                assert_eq!(
                    passage["modifications"],
                    "Comments and docstrings removed for typing practice."
                );
            }
            let scored: usize = text
                .split('\n')
                .map(|line| line.trim_start_matches([' ', '\t']).len())
                .sum::<usize>()
                + text.matches('\n').count();
            assert_eq!(sample["scored_characters"], scored);
            *language_counts
                .entry(passage["group"].as_str().unwrap())
                .or_insert(0) += 1;
        }
    }
    assert_eq!(seen_titles.len(), 100);
    assert_eq!(
        language_counts,
        BTreeMap::from([
            ("Python", 20),
            ("TypeScript", 20),
            ("Rust", 20),
            ("Go", 20),
            ("GDScript", 20),
        ])
    );
    assert_eq!(
        repositories,
        HashSet::from([
            "django/django",
            "psf/requests",
            "microsoft/vscode",
            "microsoft/TypeScript",
            "tokio-rs/tokio",
            "BurntSushi/ripgrep",
            "golang/go",
            "kubernetes/kubernetes",
            "godotengine/godot-demo-projects",
            "Orama-Interactive/Pixelorama",
        ])
    );
}
