//! Theme types for `[tui.theme]` in `config.toml`.
//!
//! Colors are expressed as strings, either named (for example `"cyan"` or
//! `"default"`) or hex (`"#RRGGBB"`). Object forms like
//! `{ hex = "#7aa2f7" }` are intentionally rejected.

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde::Serializer;

/// A single theme color value.
///
/// On the wire this serializes as a string:
/// - `Named`: `"cyan"`, `"light_blue"`, `"default"`, etc.
/// - `Hex`: `"#RRGGBB"` (exactly six hex digits).
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema)]
pub enum ThemeColor {
    /// A named terminal color keyword.
    Named(String),
    /// A six-digit hex color (`#RRGGBB`).
    Hex(String),
}

impl Serialize for ThemeColor {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            ThemeColor::Named(value) | ThemeColor::Hex(value) => serializer.serialize_str(value),
        }
    }
}

impl<'de> Deserialize<'de> for ThemeColor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        if let Some(hex) = s.strip_prefix('#') {
            let is_valid = hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit());
            if is_valid {
                Ok(ThemeColor::Hex(s))
            } else {
                Err(serde::de::Error::custom(format!(
                    "{s} is not a valid hex color",
                )))
            }
        } else {
            Ok(ThemeColor::Named(s))
        }
    }
}

/// Palette-level color overrides applied on top of the selected built-in theme.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ThemePalette {
    /// General accent color used for key highlights.
    pub accent: Option<ThemeColor>,
    /// Success/positive color.
    pub success: Option<ThemeColor>,
    /// Error/negative color.
    pub error: Option<ThemeColor>,
    /// Brand color used for command labels and emphasis.
    pub brand: Option<ThemeColor>,
    /// Informational color.
    pub info: Option<ThemeColor>,
    /// Optional explicit background for message blocks.
    pub message_bg: Option<ThemeColor>,
    /// Optional explicit background for selected rows/areas.
    pub selection_bg: Option<ThemeColor>,
}

/// Style overrides for a specific text component.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ThemeStyle {
    /// Optional foreground color override.
    pub fg: Option<ThemeColor>,
    /// Optional background color override.
    pub bg: Option<ThemeColor>,
    /// Add bold text.
    #[serde(default)]
    pub bold: bool,
    /// Add dim text.
    #[serde(default)]
    pub dim: bool,
    /// Add italic text.
    #[serde(default)]
    pub italic: bool,
    /// Add underlined text.
    #[serde(default)]
    pub underlined: bool,
}

/// Component-style overrides for themeable UI surfaces.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ThemeStyleOverrides {
    /// Style override for inline/fenced markdown code text.
    pub markdown_code: Option<ThemeStyle>,
    /// Style override for markdown links.
    pub markdown_link: Option<ThemeStyle>,
    /// Style override for markdown blockquotes.
    pub markdown_blockquote: Option<ThemeStyle>,
    /// Style override for "added" diff lines.
    pub diff_added: Option<ThemeStyle>,
    /// Style override for "removed" diff lines.
    pub diff_removed: Option<ThemeStyle>,
    /// Blend amount used when no explicit `message_bg` is set.
    ///
    /// `0.0` disables blending; larger values increase contrast against the
    /// terminal background.
    pub message_bg_blend: Option<f32>,
}

/// Top-level TUI theme config loaded from `[tui.theme]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ThemeConfig {
    /// Optional built-in theme name.
    ///
    /// Supported names are defined by the TUI resolver in `tui/src/theme.rs`.
    pub name: Option<String>,
    /// Optional palette overrides applied after built-in selection.
    pub palette: Option<ThemePalette>,
    /// Optional component style overrides applied after palette resolution.
    pub styles: Option<ThemeStyleOverrides>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;
    use toml;

    #[derive(Deserialize)]
    struct TuiToml {
        theme: ThemeConfig,
    }

    #[derive(Deserialize)]
    struct RootToml {
        tui: TuiToml,
    }

    fn deser(s: &str) -> Result<ThemeColor, serde_json::Error> {
        serde_json::from_value(serde_json::Value::String(s.to_string()))
    }

    #[test]
    fn hex_color_6_digit() {
        assert_eq!(deser("#ff00aa").unwrap(), ThemeColor::Hex("#ff00aa".into()));
    }

    #[test]
    fn named_color() {
        assert_eq!(deser("red").unwrap(), ThemeColor::Named("red".into()));
    }

    #[test]
    fn named_color_multi_word() {
        assert_eq!(
            deser("dark-blue").unwrap(),
            ThemeColor::Named("dark-blue".into())
        );
    }

    #[test]
    fn invalid_hex_too_short() {
        let err = deser("#ab").unwrap_err();
        assert!(err.to_string().contains("is not a valid hex color"));
    }

    #[test]
    fn invalid_hex_too_long() {
        let err = deser("#ff00aabb").unwrap_err();
        assert!(err.to_string().contains("is not a valid hex color"));
    }

    #[test]
    fn invalid_hex_5_chars() {
        let err = deser("#abcde").unwrap_err();
        assert!(err.to_string().contains("is not a valid hex color"));
    }

    #[test]
    fn hex_hash_only() {
        let err = deser("#").unwrap_err();
        assert!(err.to_string().contains("is not a valid hex color"));
    }

    #[test]
    fn invalid_hex_non_hex_digit() {
        let err = deser("#zzzzzz").unwrap_err();
        assert!(err.to_string().contains("is not a valid hex color"));
    }

    #[test]
    fn empty_string_is_named() {
        assert_eq!(deser("").unwrap(), ThemeColor::Named("".into()));
    }

    #[test]
    fn theme_config_from_toml() {
        let parsed: RootToml = toml::from_str(
            r##"
[tui.theme]
name = "ocean"

[tui.theme.palette]
accent = "#7aa2f7"
success = "green"

[tui.theme.styles.markdown_code]
fg = "#ff00aa"
bold = true
"##,
        )
        .expect("tui theme should parse");
        assert_eq!(parsed.tui.theme.name, Some("ocean".to_string()));
        assert_eq!(
            parsed.tui.theme.palette.as_ref().unwrap().accent,
            Some(ThemeColor::Hex("#7aa2f7".to_string()))
        );
        assert_eq!(
            parsed.tui.theme.styles.as_ref().unwrap().markdown_code,
            Some(ThemeStyle {
                fg: Some(ThemeColor::Hex("#ff00aa".to_string())),
                bg: None,
                bold: true,
                dim: false,
                italic: false,
                underlined: false,
            })
        );
    }
}
