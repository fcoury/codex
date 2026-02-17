//! Shared pipe-table detection helpers.
//!
//! Both the streaming controller (`streaming/controller.rs`) and the
//! markdown-fence unwrapper (`markdown.rs`) need to identify pipe-table
//! structure in raw markdown source. This module provides the canonical
//! implementations so fixes only need to happen in one place.

/// Split a pipe-delimited line into trimmed segments.
///
/// Returns `None` if the line is empty or has fewer than two segments.
/// Leading/trailing pipes are stripped before splitting.
pub(crate) fn parse_table_segments(line: &str) -> Option<Vec<&str>> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut content = trimmed;
    if let Some(without_leading) = content.strip_prefix('|') {
        content = without_leading;
    }
    if let Some(without_trailing) = content.strip_suffix('|') {
        content = without_trailing;
    }

    let segments: Vec<&str> = content.split('|').map(str::trim).collect();
    (segments.len() >= 2).then_some(segments)
}

/// Whether `line` looks like a table header row (has pipe-separated
/// segments with at least one non-empty cell).
pub(crate) fn is_table_header_line(line: &str) -> bool {
    parse_table_segments(line)
        .is_some_and(|segments| segments.iter().any(|segment| !segment.is_empty()))
}

/// Whether a single segment matches the `---`, `:---`, `---:`, or `:---:`
/// alignment-colon syntax used in markdown table delimiter rows.
pub(crate) fn is_table_delimiter_segment(segment: &str) -> bool {
    let trimmed = segment.trim();
    if trimmed.is_empty() {
        return false;
    }
    let without_leading = trimmed.strip_prefix(':').unwrap_or(trimmed);
    let without_ends = without_leading.strip_suffix(':').unwrap_or(without_leading);
    without_ends.len() >= 3 && without_ends.chars().all(|ch| ch == '-')
}

/// Whether `line` is a valid table delimiter row (every segment passes
/// [`is_table_delimiter_segment`]).
pub(crate) fn is_table_delimiter_line(line: &str) -> bool {
    parse_table_segments(line)
        .is_some_and(|segments| segments.into_iter().all(is_table_delimiter_segment))
}

/// A single line and whether table detection should consider it.
///
/// `enabled` lets callers feed full source streams while masking contexts that
/// should not participate in table detection (for example, non-markdown code
/// fences). Disabled non-blank lines still break pending-header state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TableScanLine<'a> {
    pub(crate) text: &'a str,
    pub(crate) enabled: bool,
}

/// Stateful table-pattern outcome for a scan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TablePatternState {
    /// No confirmed table and no trailing header candidate.
    None,
    /// The last non-blank line is a header candidate waiting for a delimiter.
    PendingHeader,
    /// A header row was followed immediately by a delimiter row.
    Confirmed,
}

fn is_header_candidate(line: TableScanLine<'_>) -> bool {
    line.enabled && is_table_header_line(line.text) && !is_table_delimiter_line(line.text)
}

/// Scan a line stream for markdown-table structure.
///
/// `Confirmed` means a header row is immediately followed by a delimiter row.
/// `PendingHeader` means the last non-blank line is a possible table header.
/// `None` means neither condition currently holds.
///
/// Disabled lines never contribute to a table match, but they still influence
/// pending-header state because the scan tracks the last non-blank line in the
/// full stream.
pub(crate) fn scan_table_pattern<'a>(
    lines: impl IntoIterator<Item = TableScanLine<'a>>,
) -> TablePatternState {
    let lines = lines.into_iter().collect::<Vec<_>>();

    for pair in lines.windows(2) {
        let [header, delimiter] = pair else {
            continue;
        };
        if is_header_candidate(*header)
            && delimiter.enabled
            && is_table_delimiter_line(delimiter.text)
        {
            return TablePatternState::Confirmed;
        }
    }

    let last_nonblank = lines.iter().rev().find(|line| !line.text.trim().is_empty());
    if last_nonblank.is_some_and(|line| is_header_candidate(*line)) {
        TablePatternState::PendingHeader
    } else {
        TablePatternState::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn parse_table_segments_basic() {
        assert_eq!(
            parse_table_segments("| A | B | C |"),
            Some(vec!["A", "B", "C"])
        );
    }

    #[test]
    fn parse_table_segments_no_outer_pipes() {
        assert_eq!(parse_table_segments("A | B | C"), Some(vec!["A", "B", "C"]));
    }

    #[test]
    fn parse_table_segments_single_segment_returns_none() {
        assert_eq!(parse_table_segments("| only |"), None);
    }

    #[test]
    fn parse_table_segments_empty_returns_none() {
        assert_eq!(parse_table_segments(""), None);
        assert_eq!(parse_table_segments("   "), None);
    }

    #[test]
    fn is_table_delimiter_segment_valid() {
        assert!(is_table_delimiter_segment("---"));
        assert!(is_table_delimiter_segment(":---"));
        assert!(is_table_delimiter_segment("---:"));
        assert!(is_table_delimiter_segment(":---:"));
        assert!(is_table_delimiter_segment(":-------:"));
    }

    #[test]
    fn is_table_delimiter_segment_invalid() {
        assert!(!is_table_delimiter_segment(""));
        assert!(!is_table_delimiter_segment("--"));
        assert!(!is_table_delimiter_segment("abc"));
        assert!(!is_table_delimiter_segment(":--"));
    }

    #[test]
    fn is_table_delimiter_line_valid() {
        assert!(is_table_delimiter_line("| --- | --- |"));
        assert!(is_table_delimiter_line("|:---:|---:|"));
        assert!(is_table_delimiter_line("--- | --- | ---"));
    }

    #[test]
    fn is_table_delimiter_line_invalid() {
        assert!(!is_table_delimiter_line("| A | B |"));
        assert!(!is_table_delimiter_line("| -- | -- |"));
    }

    #[test]
    fn is_table_header_line_valid() {
        assert!(is_table_header_line("| A | B |"));
        assert!(is_table_header_line("Name | Value"));
    }

    #[test]
    fn is_table_header_line_all_empty_segments() {
        assert!(!is_table_header_line("| | |"));
    }

    #[test]
    fn scan_table_pattern_returns_confirmed_for_header_delimiter_pair() {
        let state = scan_table_pattern([
            TableScanLine {
                text: "| A | B |",
                enabled: true,
            },
            TableScanLine {
                text: "| --- | --- |",
                enabled: true,
            },
        ]);
        assert_eq!(state, TablePatternState::Confirmed);
    }

    #[test]
    fn scan_table_pattern_returns_pending_for_trailing_header() {
        let state = scan_table_pattern([
            TableScanLine {
                text: "intro",
                enabled: true,
            },
            TableScanLine {
                text: "| A | B |",
                enabled: true,
            },
            TableScanLine {
                text: "",
                enabled: true,
            },
        ]);
        assert_eq!(state, TablePatternState::PendingHeader);
    }

    #[test]
    fn scan_table_pattern_returns_none_for_non_table_lines() {
        let state = scan_table_pattern([
            TableScanLine {
                text: "hello world",
                enabled: true,
            },
            TableScanLine {
                text: "still prose",
                enabled: true,
            },
        ]);
        assert_eq!(state, TablePatternState::None);
    }

    #[test]
    fn scan_table_pattern_disabled_line_clears_pending_state() {
        let state = scan_table_pattern([
            TableScanLine {
                text: "| A | B |",
                enabled: true,
            },
            TableScanLine {
                text: "```rust",
                enabled: false,
            },
        ]);
        assert_eq!(state, TablePatternState::None);
    }
}
