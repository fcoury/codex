#[cfg(test)]
use crate::markdown_render::TableColumnWidthMode;
use ratatui::text::Line;

pub(crate) fn append_markdown(
    markdown_source: &str,
    width: Option<usize>,
    lines: &mut Vec<Line<'static>>,
) {
    let rendered = crate::markdown_render::render_markdown_text_with_width(markdown_source, width);
    crate::render::line_utils::push_owned_lines(&rendered.lines, lines);
}

pub(crate) fn append_markdown_agent(
    markdown_source: &str,
    width: Option<usize>,
    lines: &mut Vec<Line<'static>>,
) {
    let normalized = unwrap_markdown_fences(markdown_source);
    append_markdown(&normalized, width, lines);
}

#[cfg(test)]
pub(crate) fn append_markdown_streaming(
    markdown_source: &str,
    width: Option<usize>,
    lines: &mut Vec<Line<'static>>,
) {
    let rendered = crate::markdown_render::render_markdown_text_with_width_and_table_mode(
        markdown_source,
        width,
        TableColumnWidthMode::HeaderOnly,
    );
    crate::render::line_utils::push_owned_lines(&rendered.lines, lines);
}

fn unwrap_markdown_fences(markdown_source: &str) -> String {
    #[derive(Clone, Copy)]
    struct Fence {
        marker: char,
        len: usize,
        is_markdown: bool,
    }

    fn parse_open_fence(line: &str) -> Option<Fence> {
        let without_newline = line.strip_suffix('\n').unwrap_or(line);
        let leading_ws = without_newline
            .as_bytes()
            .iter()
            .take_while(|byte| **byte == b' ')
            .count();
        if leading_ws > 3 {
            return None;
        }
        let trimmed = &without_newline[leading_ws..];
        let marker = trimmed.chars().next()?;
        if marker != '`' && marker != '~' {
            return None;
        }
        let len = trimmed.chars().take_while(|ch| *ch == marker).count();
        if len < 3 {
            return None;
        }
        let rest = trimmed[len..].trim();
        let info = rest.split_whitespace().next().unwrap_or_default();
        let is_markdown = info.eq_ignore_ascii_case("md") || info.eq_ignore_ascii_case("markdown");
        Some(Fence {
            marker,
            len,
            is_markdown,
        })
    }

    fn is_close_fence(line: &str, fence: Fence) -> bool {
        let without_newline = line.strip_suffix('\n').unwrap_or(line);
        let leading_ws = without_newline
            .as_bytes()
            .iter()
            .take_while(|byte| **byte == b' ')
            .count();
        if leading_ws > 3 {
            return false;
        }
        let trimmed = &without_newline[leading_ws..];
        if !trimmed.starts_with(fence.marker) {
            return false;
        }
        let len = trimmed.chars().take_while(|ch| *ch == fence.marker).count();
        if len < fence.len {
            return false;
        }
        trimmed[len..].trim().is_empty()
    }

    fn is_table_candidate_line(line: &str) -> bool {
        let trimmed = line.trim();
        let pipe_count = trimmed.chars().filter(|ch| *ch == '|').count();
        if pipe_count < 2 {
            return false;
        }
        if trimmed.starts_with('|') {
            return true;
        }
        trimmed
            .split('|')
            .map(str::trim)
            .all(|segment| !segment.is_empty())
    }

    fn is_table_delimiter_line(line: &str) -> bool {
        let trimmed = line.trim();
        let pipe_count = trimmed.chars().filter(|ch| *ch == '|').count();
        if pipe_count < 2 {
            return false;
        }

        let mut saw_segment = false;
        let segments: Vec<&str> = if trimmed.starts_with('|') {
            trimmed.trim_matches('|').split('|').collect()
        } else {
            trimmed.split('|').collect()
        };
        for segment in segments {
            let value = segment.trim();
            if value.is_empty() {
                return false;
            }
            if !value.chars().all(|ch| ch == '-' || ch == ':') {
                return false;
            }
            if !value.contains('-') {
                return false;
            }
            saw_segment = true;
        }
        saw_segment
    }

    fn markdown_fence_contains_table(content: &str) -> bool {
        let mut saw_candidate = false;
        let mut saw_delimiter = false;
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if !is_table_candidate_line(trimmed) {
                continue;
            }
            if is_table_delimiter_line(trimmed) {
                saw_delimiter = true;
            } else {
                saw_candidate = true;
            }
        }
        saw_candidate && saw_delimiter
    }

    enum ActiveFence {
        Passthrough(Fence),
        MarkdownCandidate {
            fence: Fence,
            opening_line: String,
            content: String,
        },
    }

    let mut out = String::with_capacity(markdown_source.len());
    let mut active_fence: Option<ActiveFence> = None;

    for line in markdown_source.split_inclusive('\n') {
        if let Some(active) = active_fence.take() {
            match active {
                ActiveFence::Passthrough(fence) => {
                    out.push_str(line);
                    if !is_close_fence(line, fence) {
                        active_fence = Some(ActiveFence::Passthrough(fence));
                    }
                }
                ActiveFence::MarkdownCandidate {
                    fence,
                    opening_line,
                    mut content,
                } => {
                    if is_close_fence(line, fence) {
                        if markdown_fence_contains_table(&content) {
                            out.push_str(&content);
                        } else {
                            out.push_str(&opening_line);
                            out.push_str(&content);
                            out.push_str(line);
                        }
                    } else {
                        content.push_str(line);
                        active_fence = Some(ActiveFence::MarkdownCandidate {
                            fence,
                            opening_line,
                            content,
                        });
                    }
                }
            }
            continue;
        }

        if let Some(fence) = parse_open_fence(line) {
            if fence.is_markdown {
                active_fence = Some(ActiveFence::MarkdownCandidate {
                    fence,
                    opening_line: line.to_string(),
                    content: String::new(),
                });
            } else {
                out.push_str(line);
                active_fence = Some(ActiveFence::Passthrough(fence));
            }
            continue;
        }

        out.push_str(line);
    }

    if let Some(active) = active_fence {
        match active {
            ActiveFence::Passthrough(_) => {}
            ActiveFence::MarkdownCandidate {
                opening_line,
                content,
                ..
            } => {
                out.push_str(&opening_line);
                out.push_str(&content);
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use ratatui::text::Line;

    fn lines_to_strings(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.clone())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn citations_render_as_plain_text() {
        let src = "Before 【F:/x.rs†L1】\nAfter 【F:/x.rs†L3】\n";
        let mut out = Vec::new();
        append_markdown(src, None, &mut out);
        let rendered = lines_to_strings(&out);
        assert_eq!(
            rendered,
            vec![
                "Before 【F:/x.rs†L1】".to_string(),
                "After 【F:/x.rs†L3】".to_string()
            ]
        );
    }

    #[test]
    fn indented_code_blocks_preserve_leading_whitespace() {
        // Basic sanity: indented code with surrounding blank lines should produce the indented line.
        let src = "Before\n\n    code 1\n\nAfter\n";
        let mut out = Vec::new();
        append_markdown(src, None, &mut out);
        let lines = lines_to_strings(&out);
        assert_eq!(lines, vec!["Before", "", "    code 1", "", "After"]);
    }

    #[test]
    fn append_markdown_preserves_full_text_line() {
        let src = "Hi! How can I help with codex-rs today? Want me to explore the repo, run tests, or work on a specific change?\n";
        let mut out = Vec::new();
        append_markdown(src, None, &mut out);
        assert_eq!(
            out.len(),
            1,
            "expected a single rendered line for plain text"
        );
        let rendered: String = out
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.clone())
            .collect::<Vec<_>>()
            .join("");
        assert_eq!(
            rendered,
            "Hi! How can I help with codex-rs today? Want me to explore the repo, run tests, or work on a specific change?"
        );
    }

    #[test]
    fn append_markdown_matches_tui_markdown_for_ordered_item() {
        let mut out = Vec::new();
        append_markdown("1. Tight item\n", None, &mut out);
        let lines = lines_to_strings(&out);
        assert_eq!(lines, vec!["1. Tight item".to_string()]);
    }

    #[test]
    fn append_markdown_keeps_ordered_list_line_unsplit_in_context() {
        let src = "Loose vs. tight list items:\n1. Tight item\n";
        let mut out = Vec::new();
        append_markdown(src, None, &mut out);

        let lines = lines_to_strings(&out);

        // Expect to find the ordered list line rendered as a single line,
        // not split into a marker-only line followed by the text.
        assert!(
            lines.iter().any(|s| s == "1. Tight item"),
            "expected '1. Tight item' rendered as a single line; got: {lines:?}"
        );
        assert!(
            !lines
                .windows(2)
                .any(|w| w[0].trim_end() == "1." && w[1] == "Tight item"),
            "did not expect a split into ['1.', 'Tight item']; got: {lines:?}"
        );
    }

    #[test]
    fn append_markdown_agent_unwraps_markdown_fences_for_table_rendering() {
        let src = "```markdown\n| A | B |\n|---|---|\n| 1 | 2 |\n```\n";
        let mut out = Vec::new();
        append_markdown_agent(src, None, &mut out);
        let rendered = lines_to_strings(&out);
        assert!(rendered.iter().any(|line| line.contains("┌")));
        assert!(rendered.iter().any(|line| line.contains("│ 1   │ 2   │")));
    }

    #[test]
    fn append_markdown_agent_unwraps_markdown_fences_for_no_outer_table_rendering() {
        let src = "```md\nCol A | Col B | Col C\n--- | --- | ---\nx | y | z\n10 | 20 | 30\n```\n";
        let mut out = Vec::new();
        append_markdown_agent(src, None, &mut out);
        let rendered = lines_to_strings(&out);
        assert!(rendered.iter().any(|line| line.contains("┌")));
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("│ Col A │ Col B │ Col C │"))
        );
        assert!(
            !rendered
                .iter()
                .any(|line| line.trim() == "Col A | Col B | Col C")
        );
    }

    #[test]
    fn append_markdown_agent_keeps_non_markdown_fences_as_code() {
        let src = "```rust\n| A | B |\n|---|---|\n| 1 | 2 |\n```\n";
        let mut out = Vec::new();
        append_markdown_agent(src, None, &mut out);
        let rendered = lines_to_strings(&out);
        assert_eq!(
            rendered,
            vec![
                "| A | B |".to_string(),
                "|---|---|".to_string(),
                "| 1 | 2 |".to_string(),
            ]
        );
    }

    #[test]
    fn append_markdown_agent_keeps_markdown_fence_when_content_is_not_table() {
        let src = "```markdown\n**bold**\n```\n";
        let mut out = Vec::new();
        append_markdown_agent(src, None, &mut out);
        let rendered = lines_to_strings(&out);
        assert_eq!(rendered, vec!["**bold**".to_string()]);
    }
}
