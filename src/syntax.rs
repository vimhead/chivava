use std::collections::HashMap;

use anyhow::{Context, Result};
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyntaxRole {
    Plain,
    Comment,
    Keyword,
    Function,
    Variable,
    String,
    Number,
    Type,
    Operator,
    Punctuation,
}

const CAPTURE_ROLES: [(&str, SyntaxRole); 18] = [
    ("comment", SyntaxRole::Comment),
    ("keyword", SyntaxRole::Keyword),
    ("function", SyntaxRole::Function),
    ("variable", SyntaxRole::Variable),
    ("string", SyntaxRole::String),
    ("number", SyntaxRole::Number),
    ("type", SyntaxRole::Type),
    ("operator", SyntaxRole::Operator),
    ("punctuation", SyntaxRole::Punctuation),
    ("constant", SyntaxRole::Number),
    ("constant.builtin", SyntaxRole::Keyword),
    ("constructor", SyntaxRole::Type),
    ("property", SyntaxRole::Variable),
    ("attribute", SyntaxRole::Type),
    ("label", SyntaxRole::Variable),
    ("escape", SyntaxRole::String),
    ("embedded", SyntaxRole::Plain),
    ("tag", SyntaxRole::Type),
];

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum CodeLanguage {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Go,
    GdScript,
}

impl CodeLanguage {
    fn recognize(label: &str) -> Option<Self> {
        match label.trim().to_ascii_lowercase().as_str() {
            "rust" | "rs" => Some(Self::Rust),
            "python" | "py" => Some(Self::Python),
            "javascript" | "js" => Some(Self::JavaScript),
            "typescript" | "ts" => Some(Self::TypeScript),
            "go" | "golang" => Some(Self::Go),
            "gdscript" | "gd" => Some(Self::GdScript),
            _ => None,
        }
    }

    fn create_configuration(self) -> Result<HighlightConfiguration> {
        let (language, name, highlights, locals) = match self {
            Self::Rust => (
                tree_sitter_rust::LANGUAGE.into(),
                "rust",
                tree_sitter_rust::HIGHLIGHTS_QUERY,
                "",
            ),
            Self::Python => (
                tree_sitter_python::LANGUAGE.into(),
                "python",
                tree_sitter_python::HIGHLIGHTS_QUERY,
                "",
            ),
            Self::JavaScript => (
                tree_sitter_javascript::LANGUAGE.into(),
                "javascript",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::LOCALS_QUERY,
            ),
            Self::TypeScript => (
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                "typescript",
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
                tree_sitter_typescript::LOCALS_QUERY,
            ),
            Self::Go => (
                tree_sitter_go::LANGUAGE.into(),
                "go",
                tree_sitter_go::HIGHLIGHTS_QUERY,
                "",
            ),
            Self::GdScript => (
                tree_sitter_gdscript::LANGUAGE.into(),
                "gdscript",
                include_str!("../assets/queries/gdscript-highlights.scm"),
                "",
            ),
        };
        let highlights = match self {
            Self::Rust => format!(
                "(identifier) @variable\n{highlights}\n(integer_literal) @number\n(float_literal) @number\n"
            ),
            Self::TypeScript => {
                format!("{}\n{highlights}", tree_sitter_javascript::HIGHLIGHT_QUERY)
            }
            Self::Go => format!(
                "{highlights}\n(function_declaration name: (identifier) @function)\n\
                 (method_declaration name: (field_identifier) @function)\n\
                 (call_expression function: (identifier) @function)\n\
                 (call_expression function: (selector_expression field: (field_identifier) @function))\n"
            ),
            _ => highlights.to_owned(),
        };
        let locals = if self == Self::TypeScript {
            format!("{}\n{locals}", tree_sitter_javascript::LOCALS_QUERY)
        } else {
            locals.to_owned()
        };
        let mut configuration =
            HighlightConfiguration::new(language, name, &highlights, "", &locals)
                .with_context(|| format!("Load bundled {name} highlighting queries"))?;
        configuration.configure(&CAPTURE_ROLES.map(|(capture, _)| capture));
        Ok(configuration)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct CodeHighlights {
    byte_roles: Vec<SyntaxRole>,
}

impl CodeHighlights {
    pub fn create_unhighlighted() -> Self {
        Self { byte_roles: vec![] }
    }

    pub fn resolve_role_at_byte(&self, offset: usize) -> Option<SyntaxRole> {
        self.byte_roles.get(offset).copied()
    }
}

pub struct SyntaxHighlighter {
    highlighter: Highlighter,
    configurations: HashMap<CodeLanguage, HighlightConfiguration>,
    #[cfg(test)]
    parsed_passages: usize,
}

impl SyntaxHighlighter {
    pub fn create() -> Self {
        Self {
            highlighter: Highlighter::new(),
            configurations: HashMap::new(),
            #[cfg(test)]
            parsed_passages: 0,
        }
    }

    #[cfg(test)]
    pub fn count_parsed_passages(&self) -> usize {
        self.parsed_passages
    }

    pub fn highlight_code(&mut self, language: &str, code: &str) -> Result<CodeHighlights> {
        let Some(language) = CodeLanguage::recognize(language) else {
            return Ok(CodeHighlights::create_unhighlighted());
        };
        if let std::collections::hash_map::Entry::Vacant(entry) =
            self.configurations.entry(language)
        {
            entry.insert(language.create_configuration()?);
        }
        #[cfg(test)]
        {
            self.parsed_passages += 1;
        }
        let configuration = &self.configurations[&language];
        let events =
            self.highlighter
                .highlight(configuration, code.as_bytes(), None, None, |_| None)?;
        let mut byte_roles = vec![SyntaxRole::Plain; code.len()];
        let mut active_roles = vec![];
        for event in events {
            match event? {
                HighlightEvent::HighlightStart(highlight) => {
                    active_roles.push(CAPTURE_ROLES[highlight.0].1)
                }
                HighlightEvent::HighlightEnd => {
                    active_roles.pop();
                }
                HighlightEvent::Source { start, end } => {
                    byte_roles[start..end]
                        .fill(active_roles.last().copied().unwrap_or(SyntaxRole::Plain));
                }
            }
        }
        Ok(CodeHighlights { byte_roles })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_grammars_recognize_keywords_functions_strings_numbers_and_comments() {
        let mut highlighter = SyntaxHighlighter::create();
        for (language, code, keyword, function, comment) in [
            (
                "Rust",
                "fn greet() { let message = \"hello\"; let count = 42; } // note",
                "fn",
                "greet",
                "// note",
            ),
            (
                "Python",
                "def greet():\n    message = \"hello\"\n    count = 42\n    # note\n",
                "def",
                "greet",
                "# note",
            ),
            (
                "JavaScript",
                "function greet() { const message = \"hello\"; let count = 42; } // note",
                "function",
                "greet",
                "// note",
            ),
            (
                "TypeScript",
                "function greet(): string { const message: string = \"hello\"; const count: number = 42; return message; } // note",
                "function",
                "greet",
                "// note",
            ),
            (
                "Go",
                "package main\nfunc greet() { message := \"hello\"; count := 42 } // note",
                "func",
                "greet",
                "// note",
            ),
            (
                "GDScript",
                "func greet() -> void:\n\tvar message := \"hello\"\n\tvar count := 42\n\t# note\n",
                "func",
                "greet",
                "# note",
            ),
        ] {
            let highlights = highlighter.highlight_code(language, code).unwrap();
            for (token, role) in [
                (keyword, SyntaxRole::Keyword),
                (function, SyntaxRole::Function),
                ("\"hello\"", SyntaxRole::String),
                ("42", SyntaxRole::Number),
                (comment, SyntaxRole::Comment),
            ] {
                let start = code.find(token).unwrap();
                for offset in start..start + token.len() {
                    assert_eq!(
                        highlights.resolve_role_at_byte(offset),
                        Some(role),
                        "{language}: {token}"
                    );
                }
            }
        }
        assert_eq!(highlighter.configurations.len(), 6);
    }

    #[test]
    fn language_aliases_share_queries_and_unknown_languages_need_no_parser() {
        let mut highlighter = SyntaxHighlighter::create();
        for label in ["Rust", " rust ", "RS"] {
            assert_eq!(
                highlighter
                    .highlight_code(label, "fn main() {}")
                    .unwrap()
                    .resolve_role_at_byte(0),
                Some(SyntaxRole::Keyword)
            );
        }
        assert_eq!(highlighter.configurations.len(), 1);
        assert_eq!(
            highlighter
                .highlight_code("Custom import", "fn main() {}")
                .unwrap(),
            CodeHighlights::create_unhighlighted()
        );
        assert_eq!(highlighter.configurations.len(), 1);
        assert_eq!(CodeLanguage::recognize("PY"), Some(CodeLanguage::Python));
        for (aliases, expected) in [
            (["TypeScript", " ts "], CodeLanguage::TypeScript),
            (["Go", "GOLANG"], CodeLanguage::Go),
            (["GDScript", "gd"], CodeLanguage::GdScript),
        ] {
            for alias in aliases {
                assert_eq!(CodeLanguage::recognize(alias), Some(expected));
            }
        }
        assert_eq!(
            CodeLanguage::recognize("js"),
            Some(CodeLanguage::JavaScript)
        );
    }

    #[test]
    fn unicode_multiline_strings_and_incomplete_snippets_keep_byte_offsets() {
        let mut highlighter = SyntaxHighlighter::create();
        let code = "def greet():\n\tmessage = \"\"\"café\n    界\"\"\"\n\treturn message\n";
        let highlights = highlighter.highlight_code("Python", code).unwrap();
        for token in ["café", "    界"] {
            let start = code.find(token).unwrap();
            for offset in start..start + token.len() {
                assert_eq!(
                    highlights.resolve_role_at_byte(offset),
                    Some(SyntaxRole::String)
                );
            }
        }
        assert_eq!(
            highlights.resolve_role_at_byte(code.find("return").unwrap()),
            Some(SyntaxRole::Keyword)
        );
        for language in [
            "Rust",
            "Python",
            "JavaScript",
            "TypeScript",
            "Go",
            "GDScript",
        ] {
            let incomplete = "call(\"café\",\n";
            let highlights = highlighter.highlight_code(language, incomplete).unwrap();
            assert_eq!(highlights.byte_roles.len(), incomplete.len());
            assert!(
                highlighter
                    .highlight_code(language, "")
                    .unwrap()
                    .byte_roles
                    .is_empty()
            );
        }
    }

    #[test]
    fn all_bundled_code_passages_have_offline_highlights() {
        let mut highlighter = SyntaxHighlighter::create();
        let mut count = 0;
        for corpus in [
            include_str!("../assets/corpus.json"),
            include_str!("../assets/code.json"),
        ] {
            let passages: Vec<crate::content::Passage> = serde_json::from_str(corpus).unwrap();
            for passage in passages
                .into_iter()
                .filter(|passage| passage.text_type == crate::settings::TextType::Code)
            {
                let highlights = highlighter
                    .highlight_code(&passage.group, &passage.text)
                    .unwrap();
                assert_eq!(highlights.byte_roles.len(), passage.text.len());
                assert!(
                    highlights.byte_roles.contains(&SyntaxRole::Keyword),
                    "{}",
                    passage.title
                );
                count += 1;
            }
        }
        assert_eq!(count, 100);
        assert_eq!(highlighter.configurations.len(), 5);
    }

    #[test]
    fn new_languages_color_types_calls_annotations_and_operators() {
        let mut highlighter = SyntaxHighlighter::create();
        for (language, code, expected) in [
            (
                "TypeScript",
                "interface Item { value: number }\nfunction first<T>(items: readonly T[]): T | undefined { return items[0]; }",
                vec![
                    ("interface", SyntaxRole::Keyword),
                    ("Item", SyntaxRole::Type),
                    ("number", SyntaxRole::Type),
                    ("first", SyntaxRole::Function),
                    ("readonly", SyntaxRole::Keyword),
                ],
            ),
            (
                "Go",
                "package main\nfunc send[T any](out chan T, value T) { defer fmt.Println(\"done\"); out <- value }",
                vec![
                    ("send", SyntaxRole::Function),
                    ("chan", SyntaxRole::Keyword),
                    ("defer", SyntaxRole::Keyword),
                    ("Println", SyntaxRole::Function),
                    ("<-", SyntaxRole::Operator),
                ],
            ),
            (
                "GDScript",
                "class_name Player\nextends CharacterBody2D\n@export var speed: float = 42.0\nsignal moved(position: Vector2)\nfunc step() -> void:\n\t$Sprite.play(\"run\")\n\tmove_and_slide()\n",
                vec![
                    ("Player", SyntaxRole::Type),
                    ("CharacterBody2D", SyntaxRole::Type),
                    ("@export", SyntaxRole::Type),
                    ("float", SyntaxRole::Type),
                    ("moved", SyntaxRole::Function),
                    ("Vector2", SyntaxRole::Type),
                    ("$Sprite", SyntaxRole::String),
                    ("play", SyntaxRole::Function),
                    ("move_and_slide", SyntaxRole::Function),
                ],
            ),
        ] {
            let highlights = highlighter.highlight_code(language, code).unwrap();
            for (token, expected_role) in expected {
                let start = code.find(token).unwrap();
                for offset in start..start + token.len() {
                    assert_eq!(
                        highlights.resolve_role_at_byte(offset),
                        Some(expected_role),
                        "{language}: {token}"
                    );
                }
            }
        }
    }

    #[test]
    fn upstream_excerpts_remain_syntactically_complete_in_their_declaration_context() {
        let passages: Vec<crate::content::Passage> =
            serde_json::from_str(include_str!("../assets/code.json")).unwrap();
        let provenance: serde_json::Value =
            serde_json::from_str(include_str!("../assets/code-provenance.json")).unwrap();
        let mut highlighter = SyntaxHighlighter::create();
        for project in provenance["projects"].as_array().unwrap() {
            for sample in project["samples"].as_array().unwrap() {
                let passage = passages
                    .iter()
                    .find(|passage| sample["title"] == passage.title)
                    .unwrap();
                highlighter
                    .highlight_code(&passage.group, &passage.text)
                    .unwrap();
                let indent = passage
                    .text
                    .chars()
                    .take_while(|character| matches!(character, ' ' | '\t'))
                    .count();
                let prefix = &passage.text[..indent];
                let source = passage
                    .text
                    .lines()
                    .map(|line| line.strip_prefix(prefix).unwrap_or(line))
                    .collect::<Vec<_>>()
                    .join("\n");
                let source = match passage.group.as_str() {
                    "TypeScript" if sample["syntax_node_kind"] == "method_definition" => {
                        format!("class Excerpt {{\n{source}\n}}")
                    }
                    "Go" => format!("package excerpt\n{source}"),
                    _ => source,
                };
                let tree = highlighter.highlighter.parser.parse(&source, None).unwrap();
                assert!(
                    !tree.root_node().has_error(),
                    "{}: {}",
                    passage.title,
                    tree.root_node().to_sexp()
                );
                let mut pending = vec![tree.root_node()];
                while let Some(node) = pending.pop() {
                    assert!(
                        !matches!(node.kind(), "comment" | "line_comment" | "block_comment"),
                        "{} contains a comment: {}",
                        passage.title,
                        node.utf8_text(source.as_bytes()).unwrap()
                    );
                    if passage.group == "Python"
                        && matches!(
                            node.kind(),
                            "module" | "class_definition" | "function_definition"
                        )
                    {
                        let body = if node.kind() == "module" {
                            node
                        } else {
                            node.child_by_field_name("body").unwrap()
                        };
                        let is_docstring = body.named_child(0).is_some_and(|statement| {
                            statement.kind() == "expression_statement"
                                && statement.named_child(0).is_some_and(|value| {
                                    matches!(value.kind(), "string" | "concatenated_string")
                                })
                        });
                        assert!(!is_docstring, "{} contains a docstring", passage.title);
                    }
                    let mut cursor = node.walk();
                    pending.extend(node.named_children(&mut cursor));
                }
            }
        }
    }

    #[test]
    fn template_expressions_do_not_inherit_the_outer_string_color() {
        let code = "const message = `hello ${name + 42}!`;";
        let highlights = SyntaxHighlighter::create()
            .highlight_code("js", code)
            .unwrap();
        assert_eq!(
            highlights.resolve_role_at_byte(code.find("hello").unwrap()),
            Some(SyntaxRole::String)
        );
        assert_eq!(
            highlights.resolve_role_at_byte(code.find("name").unwrap()),
            Some(SyntaxRole::Variable)
        );
        assert_eq!(
            highlights.resolve_role_at_byte(code.find("42").unwrap()),
            Some(SyntaxRole::Number)
        );
        assert_eq!(
            highlights.resolve_role_at_byte(code.find('!').unwrap()),
            Some(SyntaxRole::String)
        );
    }
}
