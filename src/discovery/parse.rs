use super::{Config, FileType};
use globset::GlobSetBuilder;
use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use regex::Regex;
use std::{collections::BTreeSet, sync::OnceLock};

fn regex(pattern: &'static str, slot: &'static OnceLock<Regex>) -> &'static Regex {
    slot.get_or_init(|| Regex::new(pattern).expect("constant discovery expression"))
}
pub fn extract(text: &str, config: &Config) -> Vec<String> {
    static ANSI: OnceLock<Regex> = OnceLock::new();
    let text = regex(
        r"\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07]*(?:\x07|\x1b\\))",
        &ANSI,
    )
    .replace_all(text, "");
    let text: String = text
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .take(128 * 1024)
        .collect();
    let mut excluded = GlobSetBuilder::new();
    for pattern in &config.exclude_paths {
        if let Ok(glob) = globset::Glob::new(pattern) {
            excluded.add(glob);
        }
    }
    let excluded = excluded.build().expect("validated glob patterns");
    let mut found = BTreeSet::new();
    let mut code = false;
    let mut code_start = 0;
    let mut blocks = Vec::new();
    let mut link = false;
    for (event, range) in Parser::new(&text).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_)) => {
                code = true;
                code_start = range.start;
            }
            Event::End(TagEnd::CodeBlock) => {
                code = false;
                blocks.push(code_start..range.end);
            }
            Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. }) => {
                link = true;
                if !code {
                    candidate(&dest_url, &mut found);
                }
            }
            Event::End(TagEnd::Link | TagEnd::Image) => link = false,
            Event::Code(value) if !code && !link => candidate(&value, &mut found),
            Event::Text(value) if !code && !link => scan(&value, &mut found),
            _ => (),
        }
    }
    // Markdown can split bare URLs at emphasis markers such as underscores.
    // Scan intact URL spans separately, outside the parsed code-block ranges.
    static URLS: OnceLock<Regex> = OnceLock::new();
    for token in regex(r#"https?://[^\s<>"'`]+"#, &URLS).find_iter(&text) {
        let start = text[..token.start()].rfind('\n').map_or(0, |n| n + 1);
        let end = text[token.start()..]
            .find('\n')
            .map_or(text.len(), |n| token.start() + n);
        if !blocks.iter().any(|range| range.contains(&token.start())) && !noisy(&text[start..end]) {
            candidate(token.as_str(), &mut found);
        }
    }
    found
        .into_iter()
        .filter(|locator| {
            let normalized = locator.trim_start_matches("file://");
            let path = normalized.trim_start_matches("./");
            !excluded.is_match(normalized) && !excluded.is_match(format!("/{path}"))
        })
        .collect()
}
fn candidate(raw: &str, found: &mut BTreeSet<String>) {
    let mut raw = raw
        .trim()
        .trim_matches(['`', '"', '\'', '<', '>'])
        .trim_end_matches([',', ';', '.', '!'])
        .to_string();
    while raw.ends_with(')') && raw.matches(')').count() > raw.matches('(').count() {
        raw.pop();
    }
    while raw.ends_with(']') && raw.matches(']').count() > raw.matches('[').count() {
        raw.pop();
    }
    if raw.len() > 8192 || raw.is_empty() || raw.contains(['\n', '\0']) {
        return;
    }
    let web = raw.starts_with("http://") || raw.starts_with("https://");
    let path = raw.starts_with('/')
        || raw.starts_with("./")
        || raw.starts_with("../")
        || raw.starts_with("~/")
        || raw.starts_with("file://")
        || (!raw.contains(char::is_whitespace)
            && raw.rsplit('/').next().is_some_and(|name| {
                name.rsplit_once('.').is_some_and(|(name, extension)| {
                    !name.is_empty()
                        && (1..=12).contains(&extension.len())
                        && extension.chars().all(char::is_alphanumeric)
                })
            }));
    if (web || path)
        && !raw.starts_with("//")
        && !raw.contains("${")
        && !raw.contains("$HOME")
        && !raw.contains("<path")
    {
        found.insert(raw);
    }
}
fn scan(text: &str, found: &mut BTreeSet<String>) {
    static TOKENS: OnceLock<Regex> = OnceLock::new();
    let tokens = regex(
        r#"https?://[^\s<>"'`]+|file://[^\s<>"'`]+|"[^"\n]+"|'[^'\n]+'|(?:\.{1,2}/|~/|/)[^\s<>"'`]+|[\w.-]+(?:/[\w.-]+)*\.[A-Za-z][A-Za-z0-9]{0,11}\b"#,
        &TOKENS,
    );
    for line in text.lines() {
        if noisy(line) {
            continue;
        }
        for token in tokens.find_iter(line) {
            if token.as_str().starts_with("http://") || token.as_str().starts_with("https://") {
                continue;
            }
            let raw = token.as_str();
            if !raw.starts_with(['/', '~', '.', '"', '\''])
                && !raw.starts_with("file://")
                && FileType::path(raw) == FileType::Other
                && !matches!(
                    std::path::Path::new(raw)
                        .extension()
                        .and_then(|value| value.to_str())
                        .unwrap_or("")
                        .to_ascii_lowercase()
                        .as_str(),
                    "zip"
                        | "tar"
                        | "gz"
                        | "7z"
                        | "doc"
                        | "docx"
                        | "odt"
                        | "xls"
                        | "xlsx"
                        | "ods"
                        | "ppt"
                        | "pptx"
                        | "odp"
                        | "sqlite"
                        | "db"
                )
            {
                continue;
            }
            candidate(token.as_str(), found);
        }
    }
}
fn noisy(line: &str) -> bool {
    static NOISE: OnceLock<Regex> = OnceLock::new();
    regex(r"^\s*(?:Compiling |Checking |Downloading |Downloaded |Finished |warning:|error:|npm |cargo |pip |\$ |% |at )", &NOISE).is_match(line)
}

/// Only completed assistant text from known JSONL envelopes is considered.
pub fn log_messages(line: &str, format: &str) -> Vec<String> {
    if format == "plain" {
        return vec![line.to_string()];
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return vec![];
    };
    let kind = value["type"].as_str().unwrap_or("");
    if matches!(format, "codex" | "jsonl") {
        if kind == "event_msg" && value["payload"]["type"] == "agent_message" {
            return value["payload"]["message"]
                .as_str()
                .map(str::to_string)
                .into_iter()
                .collect();
        }
        if kind == "response_item"
            && value["payload"]["type"] == "message"
            && value["payload"]["role"] == "assistant"
        {
            return content(&value["payload"]["content"]);
        }
    }
    if matches!(format, "claude" | "jsonl")
        && kind == "assistant"
        && value["message"]["role"] == "assistant"
    {
        return content(&value["message"]["content"]);
    }
    vec![]
}
fn content(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter(|part| matches!(part["type"].as_str(), Some("text" | "output_text")))
        .filter_map(|part| part["text"].as_str().map(str::to_string))
        .collect()
}
