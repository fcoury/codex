use arborium_theme::CAPTURE_NAMES;
use arborium_theme::ThemeSlot;
use arborium_theme::capture_to_slot;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;
use tree_sitter_highlight::Highlight;
use tree_sitter_highlight::HighlightConfiguration;
use tree_sitter_highlight::HighlightEvent;
use tree_sitter_highlight::Highlighter;

const MAX_HIGHLIGHT_BYTES: usize = 256 * 1024;
const MAX_HIGHLIGHT_LINES: usize = 4000;

static HIGHLIGHT_CONFIGS: OnceLock<HashMap<&'static str, HighlightConfiguration>> = OnceLock::new();

fn highlight_names() -> &'static [&'static str] {
    CAPTURE_NAMES
}

fn highlight_configs() -> &'static HashMap<&'static str, HighlightConfiguration> {
    HIGHLIGHT_CONFIGS.get_or_init(|| {
        let mut configs: HashMap<&'static str, HighlightConfiguration> = HashMap::new();

        macro_rules! add_config {
            ($key:literal, $module:ident) => {
                if let Ok(mut config) = HighlightConfiguration::new(
                    $module::language().into(),
                    $key,
                    $module::HIGHLIGHTS_QUERY.as_ref(),
                    $module::INJECTIONS_QUERY.as_ref(),
                    $module::LOCALS_QUERY.as_ref(),
                ) {
                    config.configure(highlight_names());
                    configs.insert($key, config);
                }
            };
        }

        add_config!("bash", arborium_bash);
        add_config!("c", arborium_c);
        add_config!("cpp", arborium_cpp);
        add_config!("diff", arborium_diff);
        add_config!("go", arborium_go);
        add_config!("javascript", arborium_javascript);
        add_config!("json", arborium_json);
        add_config!("python", arborium_python);
        add_config!("rust", arborium_rust);
        add_config!("toml", arborium_toml);
        add_config!("tsx", arborium_tsx);
        add_config!("typescript", arborium_typescript);
        add_config!("yaml", arborium_yaml);

        configs
    })
}

fn highlight_config(language: &str) -> Option<&'static HighlightConfiguration> {
    highlight_configs().get(language)
}

pub(crate) fn highlight_bash_to_lines(script: &str) -> Vec<Line<'static>> {
    highlight_code_to_lines(script, Some("bash"), None)
}

pub(crate) fn highlight_code_to_lines(
    source: &str,
    language_hint: Option<&str>,
    filename_hint: Option<&str>,
) -> Vec<Line<'static>> {
    let Some(language) = resolve_language(language_hint, filename_hint, source) else {
        return plain_source_to_lines(source);
    };

    highlight_source_to_lines(source, &language).unwrap_or_else(|| plain_source_to_lines(source))
}

pub(crate) fn highlight_code_line_to_spans(
    source: &str,
    language_hint: Option<&str>,
    filename_hint: Option<&str>,
) -> Vec<Span<'static>> {
    match highlight_code_to_lines(source, language_hint, filename_hint)
        .into_iter()
        .next()
    {
        Some(line) => line.spans,
        None => Vec::new(),
    }
}

fn resolve_language(
    language_hint: Option<&str>,
    filename_hint: Option<&str>,
    source: &str,
) -> Option<String> {
    if let Some(language) = language_hint.and_then(normalize_language_hint)
        && highlight_config(&language).is_some()
    {
        return Some(language);
    }

    if let Some(language) = filename_hint
        .and_then(detect_language_from_filename)
        .map(str::to_string)
        && highlight_config(&language).is_some()
    {
        return Some(language);
    }

    if looks_like_diff(source) && highlight_config("diff").is_some() {
        return Some("diff".to_string());
    }

    None
}

fn normalize_language_hint(language_hint: &str) -> Option<String> {
    let token = language_hint
        .trim()
        .split(|c: char| c.is_whitespace() || c == ',')
        .find(|piece| !piece.is_empty())?;

    let lower = token.trim_matches('`').to_ascii_lowercase();
    if lower.is_empty() {
        return None;
    }

    let normalized = match lower.as_str() {
        "c++" => "cpp",
        "cc" | "cxx" | "hpp" | "hxx" => "cpp",
        "diff" | "patch" => "diff",
        "golang" => "go",
        "js" | "mjs" | "cjs" | "jsx" | "node" => "javascript",
        "jsonc" => "json",
        "py" | "python3" => "python",
        "rs" => "rust",
        "sh" | "shell" | "zsh" => "bash",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "yml" => "yaml",
        other => other,
    };

    Some(normalized.to_string())
}

fn detect_language_from_filename(filename: &str) -> Option<&'static str> {
    let lower = filename.to_ascii_lowercase();
    if lower.ends_with(".d.ts") || lower.ends_with(".d.mts") || lower.ends_with(".d.cts") {
        return Some("typescript");
    }

    let basename = Path::new(&lower).file_name().and_then(|name| name.to_str());
    if basename.is_some_and(|name| name == "justfile") {
        return Some("bash");
    }

    let ext = Path::new(&lower).extension().and_then(|ext| ext.to_str())?;
    match ext {
        "bash" | "sh" | "zsh" => Some("bash"),
        "c" | "h" => Some("c"),
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => Some("cpp"),
        "diff" | "patch" => Some("diff"),
        "go" => Some("go"),
        "js" | "jsx" | "mjs" | "cjs" => Some("javascript"),
        "json" | "jsonc" => Some("json"),
        "py" => Some("python"),
        "rs" => Some("rust"),
        "toml" => Some("toml"),
        "ts" | "mts" | "cts" => Some("typescript"),
        "tsx" => Some("tsx"),
        "yaml" | "yml" => Some("yaml"),
        _ => None,
    }
}

fn looks_like_diff(source: &str) -> bool {
    let trimmed = source.trim_start();
    trimmed.starts_with("diff --git ")
        || trimmed.starts_with("@@ ")
        || (source.contains("\n--- ") && source.contains("\n+++ "))
}

fn plain_source_to_lines(source: &str) -> Vec<Line<'static>> {
    if source.is_empty() {
        return vec![Line::from("")];
    }

    source
        .lines()
        .map(|line| Line::from(line.to_string()))
        .collect()
}

fn highlight_source_to_lines(source: &str, language: &str) -> Option<Vec<Line<'static>>> {
    if source.is_empty() {
        return Some(vec![Line::from("")]);
    }
    if source.len() > MAX_HIGHLIGHT_BYTES || source.lines().count() > MAX_HIGHLIGHT_LINES {
        return None;
    }

    let config = highlight_config(language)?;
    let mut highlighter = Highlighter::new();
    let iterator = highlighter
        .highlight(config, source.as_bytes(), None, |_| None)
        .ok()?;

    let mut lines: Vec<Line<'static>> = vec![Line::from("")];
    let mut highlight_stack: Vec<Highlight> = Vec::new();

    for event in iterator {
        match event {
            Ok(HighlightEvent::HighlightStart(highlight)) => highlight_stack.push(highlight),
            Ok(HighlightEvent::HighlightEnd) => {
                highlight_stack.pop();
            }
            Ok(HighlightEvent::Source { start, end }) => {
                if start >= end || end > source.len() {
                    continue;
                }
                if !source.is_char_boundary(start) || !source.is_char_boundary(end) {
                    continue;
                }

                let style = highlight_stack
                    .last()
                    .and_then(|highlight| style_for(*highlight));
                push_segment(&mut lines, &source[start..end], style);
            }
            Err(_) => return None,
        }
    }

    Some(lines)
}

fn style_for(highlight: Highlight) -> Option<Style> {
    let capture_name = highlight_names().get(highlight.0)?;
    style_for_slot(capture_to_slot(capture_name))
}

fn style_for_slot(slot: ThemeSlot) -> Option<Style> {
    match slot {
        ThemeSlot::Comment | ThemeSlot::Operator | ThemeSlot::Punctuation | ThemeSlot::Embedded => {
            Some(Style::default().dim())
        }
        ThemeSlot::String | ThemeSlot::Constant | ThemeSlot::Number | ThemeSlot::Literal => {
            Some(Style::default().cyan())
        }
        ThemeSlot::Keyword
        | ThemeSlot::Function
        | ThemeSlot::Type
        | ThemeSlot::Property
        | ThemeSlot::Attribute
        | ThemeSlot::Tag
        | ThemeSlot::Macro
        | ThemeSlot::Namespace
        | ThemeSlot::Constructor => Some(Style::default().magenta()),
        ThemeSlot::DiffAdd => Some(Style::default().green()),
        ThemeSlot::DiffDelete | ThemeSlot::Error => Some(Style::default().red()),
        ThemeSlot::Title | ThemeSlot::Strong => Some(Style::default().bold()),
        ThemeSlot::Emphasis => Some(Style::default().italic()),
        ThemeSlot::Link => Some(Style::default().cyan().underlined()),
        ThemeSlot::Strikethrough => Some(Style::default().crossed_out()),
        ThemeSlot::Variable | ThemeSlot::Label | ThemeSlot::None => None,
    }
}

fn push_segment(lines: &mut Vec<Line<'static>>, segment: &str, style: Option<Style>) {
    for (index, part) in segment.split('\n').enumerate() {
        if index > 0 {
            lines.push(Line::from(""));
        }
        if part.is_empty() {
            continue;
        }

        let span = match style {
            Some(style) => Span::styled(part.to_string(), style),
            None => Span::from(part.to_string()),
        };

        if let Some(last) = lines.last_mut() {
            last.spans.push(span);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use ratatui::style::Modifier;

    fn reconstructed(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.clone())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn round_trip_bash_highlighting_keeps_source_text() {
        let source = "echo \"hi\" && printf '%s\\n' \"there\"";
        let lines = highlight_bash_to_lines(source);
        assert_eq!(reconstructed(&lines), source);
    }

    #[test]
    fn normalizes_language_aliases() {
        assert_eq!(normalize_language_hint("rs"), Some("rust".to_string()));
        assert_eq!(normalize_language_hint("patch"), Some("diff".to_string()));
        assert_eq!(normalize_language_hint(" yml "), Some("yaml".to_string()));
    }

    #[test]
    fn markdown_fence_info_is_trimmed_to_first_token() {
        assert_eq!(
            normalize_language_hint("rust no_run"),
            Some("rust".to_string())
        );
    }

    #[test]
    fn produces_non_default_styles_for_code() {
        let lines = highlight_code_to_lines("fn main() { let x = 1; }", Some("rust"), None);
        let has_styled_span = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .any(|span| span.style != Style::default());
        assert!(has_styled_span);
    }

    #[test]
    fn diff_highlighting_marks_add_delete_lines() {
        let source = "diff --git a/a.txt b/a.txt\n@@ -1 +1 @@\n-old value\n+new value\n";
        let lines = highlight_code_to_lines(source, Some("diff"), None);

        let has_non_default_style = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .any(|span| span.style != Style::default());

        assert!(has_non_default_style);
    }

    #[test]
    fn comments_get_dimmed_style() {
        let lines = highlight_code_to_lines("# comment", Some("bash"), None);
        let has_dimmed = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .any(|span| span.style.add_modifier.contains(Modifier::DIM));
        assert!(has_dimmed);
    }

    #[test]
    fn filename_hint_detects_rust() {
        let lines = highlight_code_to_lines("fn main() {}", None, Some("src/main.rs"));
        let has_styled_span = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .any(|span| span.style != Style::default());
        assert!(has_styled_span);
    }
}
