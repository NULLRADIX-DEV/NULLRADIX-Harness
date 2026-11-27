#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "integration tests fail loudly"
)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use nullradix_api::HookKindDto;
use serde_json::Value;

const ROOT_VARIABLE: &str = "${CLAUDE_PLUGIN_ROOT}/";
/// The binary is copied into the plugin by the installer, so the source folder has none.
const INSTALLED_BINARY: &str = "bin/nullradix";
const SKILLS: [&str; 8] = [
    "board",
    "customize",
    "grill",
    "implement",
    "question",
    "setup",
    "tasks",
    "wayfinder",
];
const SKILL_LIMIT_BYTES: usize = 10_000;
const GERMAN_WORDS: [&str; 8] = [
    "und", "nicht", "wird", "oder", "ist", "eine", "einen", "dass",
];

fn plugin_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugin")
}

fn json(relative: &str) -> Value {
    let text = fs::read_to_string(plugin_dir().join(relative)).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn files_below(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files_below(&path, found);
        } else {
            found.push(path);
        }
    }
}

fn front_matter(text: &str) -> Vec<(String, String)> {
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some("---"), "front matter must open the file");
    let mut pairs = Vec::new();
    for line in lines {
        if line == "---" {
            return pairs;
        }
        let (key, value) = line.split_once(':').unwrap();
        pairs.push((key.trim().to_owned(), value.trim().to_owned()));
    }
    panic!("front matter is not closed");
}

fn value<'a>(pairs: &'a [(String, String)], key: &str) -> &'a str {
    pairs
        .iter()
        .find(|(name, _)| name == key)
        .map_or("", |(_, value)| value.as_str())
}

/// Every kind the daemon accepts; the match breaks when a kind is added.
fn all_kinds() -> [HookKindDto; 7] {
    let kinds = [
        HookKindDto::SessionStart,
        HookKindDto::UserPrompt,
        HookKindDto::Notification,
        HookKindDto::Stop,
        HookKindDto::SubagentStart,
        HookKindDto::SubagentStop,
        HookKindDto::SessionEnd,
    ];
    for kind in kinds {
        match kind {
            HookKindDto::SessionStart
            | HookKindDto::UserPrompt
            | HookKindDto::Notification
            | HookKindDto::Stop
            | HookKindDto::SubagentStart
            | HookKindDto::SubagentStop
            | HookKindDto::SessionEnd => {}
        }
    }
    kinds
}

fn claude_event(kind: HookKindDto) -> &'static str {
    match kind {
        HookKindDto::SessionStart => "SessionStart",
        HookKindDto::UserPrompt => "UserPromptSubmit",
        HookKindDto::Notification => "Notification",
        HookKindDto::Stop => "Stop",
        HookKindDto::SubagentStart => "SubagentStart",
        HookKindDto::SubagentStop => "SubagentStop",
        HookKindDto::SessionEnd => "SessionEnd",
    }
}

#[test]
fn the_manifest_matches_the_workspace() {
    let manifest = json(".claude-plugin/plugin.json");
    assert_eq!(manifest["name"], "nullradix");
    assert_eq!(manifest["version"], env!("CARGO_PKG_VERSION"));
    assert_ne!(manifest["description"], "");
}

#[test]
fn the_manifest_names_only_existing_files() {
    let manifest = json(".claude-plugin/plugin.json");
    for key in [
        "skills",
        "commands",
        "agents",
        "hooks",
        "outputStyles",
        "mcpServers",
    ] {
        let paths: Vec<&str> = match &manifest[key] {
            Value::String(path) => vec![path.as_str()],
            Value::Array(items) => items.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        for path in paths {
            let relative = path.trim_start_matches("./");
            assert!(plugin_dir().join(relative).exists(), "{key}: {path}");
        }
    }
}

#[test]
fn the_hooks_call_existing_hook_commands() {
    let hooks = json("hooks/hooks.json");
    let events = hooks["hooks"].as_object().unwrap();
    let mut called = BTreeSet::new();
    for (event, groups) in events {
        for group in groups.as_array().unwrap() {
            for hook in group["hooks"].as_array().unwrap() {
                assert_eq!(hook["type"], "command", "{event}");
                let command = hook["command"].as_str().unwrap();
                let relative = command.strip_prefix(ROOT_VARIABLE).unwrap();
                assert!(
                    relative == INSTALLED_BINARY || plugin_dir().join(relative).exists(),
                    "{event}: {command} does not exist"
                );
                let args: Vec<&str> = hook["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|arg| arg.as_str().unwrap())
                    .collect();
                assert_eq!(args.len(), 2, "{event}");
                assert_eq!(args[0], "hook", "{event}");
                let kind: HookKindDto = serde_json::from_value(Value::from(args[1])).unwrap();
                assert_eq!(claude_event(kind), event, "{event} calls {}", args[1]);
                assert!(hook["timeout"].as_u64().unwrap() <= 5, "{event}");
                called.insert(args[1].to_owned());
            }
        }
    }
    let expected: BTreeSet<String> = all_kinds()
        .iter()
        .map(|kind| kind.as_str().to_owned())
        .collect();
    assert_eq!(called, expected);
}

#[test]
fn the_skills_are_the_eight_shortened_ones() {
    let mut found = BTreeSet::new();
    for entry in fs::read_dir(plugin_dir().join("skills")).unwrap() {
        let dir = entry.unwrap().path();
        let folder = dir.file_name().unwrap().to_str().unwrap().to_owned();
        let text = fs::read_to_string(dir.join("SKILL.md")).unwrap();
        let head = front_matter(&text);
        assert_eq!(value(&head, "name"), folder);
        assert!(!value(&head, "description").is_empty(), "{folder}");
        assert!(value(&head, "description").len() <= 1024, "{folder}");
        assert!(text.len() <= SKILL_LIMIT_BYTES, "{folder} is too long");
        found.insert(folder);
    }
    let expected: BTreeSet<String> = SKILLS.iter().map(|name| (*name).to_owned()).collect();
    assert_eq!(found, expected);
}

#[test]
fn the_output_style_has_a_name_and_a_description() {
    let text = fs::read_to_string(plugin_dir().join("output-styles/software-engineer.md")).unwrap();
    let head = front_matter(&text);
    assert_eq!(value(&head, "name"), "software-engineer");
    assert_ne!(value(&head, "description"), "");
}

#[test]
fn every_plugin_file_is_ascii_english() {
    let mut files = Vec::new();
    files_below(&plugin_dir(), &mut files);
    assert!(files.len() >= 13, "{files:?}");
    for file in files {
        let text = fs::read_to_string(&file).unwrap();
        assert!(text.is_ascii(), "{} is not ASCII", file.display());
        for word in text.split(|letter: char| !letter.is_ascii_alphabetic()) {
            assert!(
                !GERMAN_WORDS.contains(&word.to_ascii_lowercase().as_str()),
                "{} has the German word {word}",
                file.display()
            );
        }
    }
}
