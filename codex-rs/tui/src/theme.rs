//! Runtime TUI theme resolution and global theme access.
//!
//! Resolution order:
//! 1. Select built-in from `ThemeConfig.name` (or `default` if unset/unknown).
//! 2. Apply `ThemeConfig.palette` color overrides.
//! 3. Recompute dependent component styles from the palette.
//! 4. Apply `ThemeConfig.styles` component-level overrides.

use std::sync::Mutex;
use std::sync::OnceLock;

use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use strum::IntoEnumIterator;
use strum_macros::Display;
use strum_macros::EnumIter;
use strum_macros::EnumString;
use strum_macros::IntoStaticStr;
use tracing::warn;

use crate::color::adapt_color_for_terminal;
use crate::color::blend;
use crate::color::is_light;
use crate::terminal_palette::best_color;
use crate::terminal_palette::default_bg;
use codex_core::config::theme::ThemeColor;
use codex_core::config::theme::ThemeConfig;
use codex_core::config::theme::ThemePalette;
use codex_core::config::theme::ThemeStyle;
use codex_core::config::theme::ThemeStyleOverrides;

/// Concrete theme values used by renderers at runtime.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    /// Accent color/style base.
    pub accent: Color,
    /// Success color/style base.
    pub success: Color,
    /// Error color/style base.
    pub error: Color,
    /// Brand color/style base.
    pub brand: Color,
    /// Informational color/style base.
    pub info: Color,
    /// Optional explicit message background.
    pub message_bg: Option<Color>,
    /// Optional explicit selection background.
    pub selection_bg: Option<Color>,
    /// Markdown inline/fenced code style.
    pub markdown_code: Style,
    /// Markdown link style.
    pub markdown_link: Style,
    /// Markdown blockquote style.
    pub markdown_blockquote: Style,
    /// Diff "added" style.
    pub diff_added: Style,
    /// Diff "removed" style.
    pub diff_removed: Style,
    /// Fallback message background blend amount.
    pub message_bg_blend: f32,
}

/// Built-in themes supported by `ThemeConfig.name`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumString, EnumIter, Display, IntoStaticStr)]
#[strum(serialize_all = "kebab-case")]
enum BuiltinTheme {
    /// Legacy/default dark palette.
    Default,
    /// Tokyo-night-inspired dark palette.
    Ocean,
    /// Rose Pine dark palette.
    RosePine,
    /// Solarized dark palette.
    Solarized,
    /// Catppuccin Latte-inspired light palette.
    CatppuccinLight,
    /// Everforest Light-inspired palette.
    EverforestLight,
    /// Ayu Light-inspired palette.
    AyuLight,
}

impl BuiltinTheme {
    fn from_name(name: &str) -> Option<Self> {
        name.parse::<BuiltinTheme>().ok()
    }

    fn is_dark(self) -> Option<bool> {
        match self {
            BuiltinTheme::Default => None,
            BuiltinTheme::Ocean | BuiltinTheme::RosePine | BuiltinTheme::Solarized => Some(true),
            BuiltinTheme::CatppuccinLight
            | BuiltinTheme::EverforestLight
            | BuiltinTheme::AyuLight => Some(false),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct BuiltinPalette {
    accent: (u8, u8, u8),
    success: (u8, u8, u8),
    error: (u8, u8, u8),
    brand: (u8, u8, u8),
    info: (u8, u8, u8),
    message_bg: Option<(u8, u8, u8)>,
    selection_bg: Option<(u8, u8, u8)>,
}

impl Theme {
    fn default_theme() -> Theme {
        build_theme(
            Color::Cyan,
            Color::Green,
            Color::Red,
            Color::Magenta,
            Color::LightBlue,
            None,
            None,
        )
    }

    fn from_builtin(builtin: BuiltinTheme) -> Theme {
        match builtin {
            BuiltinTheme::Default => Self::default_theme(),
            BuiltinTheme::Ocean
            | BuiltinTheme::RosePine
            | BuiltinTheme::Solarized
            | BuiltinTheme::CatppuccinLight
            | BuiltinTheme::EverforestLight
            | BuiltinTheme::AyuLight => build_theme_from_palette(
                builtin_palette(builtin).unwrap_or_else(BuiltinPalette::fallback),
            ),
        }
    }

    fn from_builtin_with_terminal(builtin: BuiltinTheme, terminal_bg: (u8, u8, u8)) -> Theme {
        let Some(palette) = builtin_palette(builtin) else {
            return Self::default_theme();
        };
        let Some(builtin_is_dark) = builtin.is_dark() else {
            return Self::default_theme();
        };
        let terminal_is_dark = !is_light(terminal_bg);
        if builtin_is_dark == terminal_is_dark {
            return build_theme_from_palette(palette);
        }
        // Polarity mismatch: adapt foregrounds and let backgrounds derive from terminal.
        let adapted_accent =
            adapt_color_for_terminal(palette.accent, terminal_bg, terminal_is_dark);
        let adapted_brand = adapt_color_for_terminal(palette.brand, terminal_bg, terminal_is_dark);
        let (message_alpha, selection_alpha) = if terminal_is_dark {
            (0.12, 0.22)
        } else {
            (0.08, 0.16)
        };
        let adapted = BuiltinPalette {
            accent: adapted_accent,
            success: adapt_color_for_terminal(palette.success, terminal_bg, terminal_is_dark),
            error: adapt_color_for_terminal(palette.error, terminal_bg, terminal_is_dark),
            brand: adapted_brand,
            info: adapt_color_for_terminal(palette.info, terminal_bg, terminal_is_dark),
            // Polarity mismatch: tint backgrounds from the adapted brand.
            message_bg: Some(blend(adapted_brand, terminal_bg, message_alpha)),
            selection_bg: Some(blend(adapted_brand, terminal_bg, selection_alpha)),
        };
        build_theme_from_palette(adapted)
    }

    /// Returns a [`Style`] with the accent foreground color.
    pub fn accent_style(&self) -> Style {
        Style::default().fg(self.accent)
    }

    /// Returns a [`Style`] with the success foreground color.
    pub fn success_style(&self) -> Style {
        Style::default().fg(self.success)
    }

    /// Returns a [`Style`] with the error foreground color.
    pub fn error_style(&self) -> Style {
        Style::default().fg(self.error)
    }

    /// Returns a [`Style`] with the brand foreground color.
    pub fn brand_style(&self) -> Style {
        Style::default().fg(self.brand)
    }

    /// Returns a [`Style`] with the info foreground color.
    pub fn info_style(&self) -> Style {
        Style::default().fg(self.info)
    }

    /// Returns a bold [`Style`] with the success foreground color.
    pub fn success_bold(&self) -> Style {
        self.success_style().add_modifier(Modifier::BOLD)
    }

    /// Returns a bold [`Style`] with the error foreground color.
    pub fn error_bold(&self) -> Style {
        self.error_style().add_modifier(Modifier::BOLD)
    }

    /// Returns a bold [`Style`] with the brand foreground color.
    pub fn brand_bold(&self) -> Style {
        self.brand_style().add_modifier(Modifier::BOLD)
    }
}

impl BuiltinPalette {
    fn fallback() -> Self {
        Self {
            accent: (0, 255, 255),
            success: (0, 255, 0),
            error: (255, 0, 0),
            brand: (255, 0, 255),
            info: (135, 206, 250),
            message_bg: None,
            selection_bg: None,
        }
    }
}

fn build_theme(
    accent: Color,
    success: Color,
    error: Color,
    brand: Color,
    info: Color,
    message_bg: Option<Color>,
    selection_bg: Option<Color>,
) -> Theme {
    Theme {
        accent,
        success,
        error,
        brand,
        info,
        message_bg,
        selection_bg,
        markdown_code: Style::default().fg(accent),
        markdown_link: Style::default()
            .fg(accent)
            .add_modifier(Modifier::UNDERLINED),
        markdown_blockquote: Style::default().fg(success),
        diff_added: Style::default().fg(success),
        diff_removed: Style::default().fg(error),
        message_bg_blend: 0.12,
    }
}

fn build_theme_from_palette(palette: BuiltinPalette) -> Theme {
    build_theme(
        best_color(palette.accent),
        best_color(palette.success),
        best_color(palette.error),
        best_color(palette.brand),
        best_color(palette.info),
        palette.message_bg.map(best_color),
        palette.selection_bg.map(best_color),
    )
}

/// Converts a `#RRGGBB` literal into RGB components.
///
/// Only used for hardcoded builtin palette values, so an invalid literal is a
/// programmer error surfaced at startup rather than a user-facing failure.
fn hex_rgb(hex: &str) -> (u8, u8, u8) {
    parse_hex_triplet(hex).unwrap_or_else(|| {
        tracing::error!("invalid built-in hex color literal: {hex}");
        (0, 0, 0)
    })
}

fn builtin_palette(builtin: BuiltinTheme) -> Option<BuiltinPalette> {
    match builtin {
        BuiltinTheme::Default => None,
        BuiltinTheme::Ocean => Some(BuiltinPalette {
            accent: hex_rgb("#7aa2f7"),
            success: hex_rgb("#9ece6a"),
            error: hex_rgb("#f7768e"),
            brand: hex_rgb("#bb9af7"),
            info: hex_rgb("#7dcfff"),
            message_bg: Some(hex_rgb("#1a1b26")),
            selection_bg: Some(hex_rgb("#283457")),
        }),
        BuiltinTheme::RosePine => Some(BuiltinPalette {
            accent: hex_rgb("#9ccfd8"),
            success: hex_rgb("#31748f"),
            error: hex_rgb("#eb6f92"),
            brand: hex_rgb("#c4a7e7"),
            info: hex_rgb("#f6c177"),
            message_bg: Some(hex_rgb("#191724")),
            selection_bg: Some(hex_rgb("#26233a")),
        }),
        BuiltinTheme::Solarized => Some(BuiltinPalette {
            accent: hex_rgb("#268bd2"),
            success: hex_rgb("#859900"),
            error: hex_rgb("#dc322f"),
            brand: hex_rgb("#d33682"),
            info: hex_rgb("#2aa198"),
            message_bg: Some(hex_rgb("#002b36")),
            selection_bg: Some(hex_rgb("#073642")),
        }),
        BuiltinTheme::CatppuccinLight => Some(BuiltinPalette {
            accent: hex_rgb("#1e66f5"),
            success: hex_rgb("#40a02b"),
            error: hex_rgb("#d20f39"),
            brand: hex_rgb("#8839ef"),
            info: hex_rgb("#209fb5"),
            message_bg: Some(hex_rgb("#eff1f5")),
            selection_bg: Some(hex_rgb("#ccd0da")),
        }),
        BuiltinTheme::EverforestLight => Some(BuiltinPalette {
            accent: hex_rgb("#3a94c5"),
            success: hex_rgb("#8da101"),
            error: hex_rgb("#f85552"),
            brand: hex_rgb("#df69ba"),
            info: hex_rgb("#35a77c"),
            message_bg: Some(hex_rgb("#f8f5e4")),
            selection_bg: Some(hex_rgb("#f2efdf")),
        }),
        BuiltinTheme::AyuLight => Some(BuiltinPalette {
            accent: hex_rgb("#36a3d9"),
            success: hex_rgb("#86b300"),
            error: hex_rgb("#ff3333"),
            brand: hex_rgb("#a37acc"),
            info: hex_rgb("#4cbf99"),
            message_bg: Some(hex_rgb("#fafafa")),
            selection_bg: Some(hex_rgb("#f0eee4")),
        }),
    }
}

/// Resolves a named color keyword used in `ThemeColor::Named`.
///
/// Returns `None` for unsupported names.
fn resolve_named_color(name: &str) -> Option<Color> {
    match name {
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "gray" => Some(Color::Gray),
        "grey" => Some(Color::Gray),
        "dark_gray" => Some(Color::DarkGray),
        "light_red" => Some(Color::LightRed),
        "light_green" => Some(Color::LightGreen),
        "light_yellow" => Some(Color::LightYellow),
        "light_blue" => Some(Color::LightBlue),
        "light_magenta" => Some(Color::LightMagenta),
        "light_cyan" => Some(Color::LightCyan),
        "white" => Some(Color::White),
        "default" => Some(Color::Reset),
        _ => None,
    }
}

/// Resolves a configured `ThemeColor` into a terminal `Color`.
///
/// Invalid hex values are logged and treated as missing.
fn resolve_theme_color(color: &ThemeColor) -> Option<Color> {
    match color {
        ThemeColor::Named(name) => resolve_named_color(name),
        ThemeColor::Hex(hex) => match parse_hex_triplet(hex) {
            Some((r, g, b)) => Some(best_color((r, g, b))),
            None => {
                warn!("Invalid hex color in theme: {hex}");
                None
            }
        },
    }
}

/// Parses a `#RRGGBB` value into RGB components.
fn parse_hex_triplet(hex: &str) -> Option<(u8, u8, u8)> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some((r, g, b))
}

/// Resolves an effective runtime theme from configuration.
///
/// Unknown built-in names fall back to `default` and emit a warning.
pub fn resolve_theme(config: &ThemeConfig) -> Theme {
    resolve_theme_with_terminal_bg(config, default_bg())
}

fn resolve_theme_with_terminal_bg(
    config: &ThemeConfig,
    terminal_bg: Option<(u8, u8, u8)>,
) -> Theme {
    let builtin = config.name.as_deref().and_then(BuiltinTheme::from_name);
    if config.name.is_some() && builtin.is_none() {
        warn!(
            "Unknown theme name: {}",
            config.name.as_deref().unwrap_or_default()
        );
    }
    // Only auto-adapt pure built-ins without user overrides.
    let should_adapt = builtin.is_some() && config.palette.is_none() && config.styles.is_none();
    let mut theme = match builtin {
        Some(builtin) => {
            if should_adapt {
                if let Some(bg) = terminal_bg {
                    // Apply polarity adaptation against the detected terminal background.
                    Theme::from_builtin_with_terminal(builtin, bg)
                } else {
                    Theme::from_builtin(builtin)
                }
            } else {
                Theme::from_builtin(builtin)
            }
        }
        None => Theme::default_theme(),
    };

    if let Some(palette) = &config.palette {
        apply_palette_overrides(&mut theme, palette);
        apply_component_styles(&mut theme);
    }

    if let Some(styles) = &config.styles {
        apply_style_overrides(&mut theme, styles);
    }

    theme
}

fn apply_palette_overrides(theme: &mut Theme, palette: &ThemePalette) {
    if let Some(color) = palette.accent.as_ref().and_then(resolve_theme_color) {
        theme.accent = color;
    }
    if let Some(color) = palette.success.as_ref().and_then(resolve_theme_color) {
        theme.success = color;
    }
    if let Some(color) = palette.error.as_ref().and_then(resolve_theme_color) {
        theme.error = color;
    }
    if let Some(color) = palette.brand.as_ref().and_then(resolve_theme_color) {
        theme.brand = color;
    }
    if let Some(color) = palette.info.as_ref().and_then(resolve_theme_color) {
        theme.info = color;
    }
    if let Some(color) = palette.message_bg.as_ref().and_then(resolve_theme_color) {
        theme.message_bg = Some(color);
    }
    if let Some(color) = palette.selection_bg.as_ref().and_then(resolve_theme_color) {
        theme.selection_bg = Some(color);
    }
}

fn apply_component_styles(theme: &mut Theme) {
    theme.markdown_code = Style::default().fg(theme.accent);
    theme.markdown_link = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::UNDERLINED);
    theme.markdown_blockquote = Style::default().fg(theme.success);
    theme.diff_added = Style::default().fg(theme.success);
    theme.diff_removed = Style::default().fg(theme.error);
}

fn apply_style_overrides(theme: &mut Theme, overrides: &ThemeStyleOverrides) {
    if let Some(style) = &overrides.markdown_code {
        theme.markdown_code = apply_style_override(theme.markdown_code, style);
    }
    if let Some(style) = &overrides.markdown_link {
        theme.markdown_link = apply_style_override(theme.markdown_link, style);
    }
    if let Some(style) = &overrides.markdown_blockquote {
        theme.markdown_blockquote = apply_style_override(theme.markdown_blockquote, style);
    }
    if let Some(style) = &overrides.diff_added {
        theme.diff_added = apply_style_override(theme.diff_added, style);
    }
    if let Some(style) = &overrides.diff_removed {
        theme.diff_removed = apply_style_override(theme.diff_removed, style);
    }
    if let Some(blend) = overrides.message_bg_blend {
        theme.message_bg_blend = blend;
    }
}

fn apply_style_override(base: Style, override_style: &ThemeStyle) -> Style {
    let mut style = base;
    if let Some(color) = override_style.fg.as_ref().and_then(resolve_theme_color) {
        style = style.fg(color);
    }
    if let Some(color) = override_style.bg.as_ref().and_then(resolve_theme_color) {
        style = style.bg(color);
    }
    if override_style.bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    if override_style.dim {
        style = style.add_modifier(Modifier::DIM);
    }
    if override_style.italic {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if override_style.underlined {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    style
}

static ACTIVE_THEME: OnceLock<Mutex<Theme>> = OnceLock::new();

fn theme_lock() -> &'static Mutex<Theme> {
    ACTIVE_THEME.get_or_init(|| Mutex::new(Theme::default_theme()))
}

/// Returns the current active theme snapshot.
pub fn current() -> Theme {
    theme_lock()
        .lock()
        .map(|t| t.clone())
        .unwrap_or_else(|_| Theme::default_theme())
}

/// Replaces the global active theme used by renderers.
pub fn set_theme(theme: Theme) {
    if let Ok(mut guard) = theme_lock().lock() {
        *guard = theme;
    }
}

/// Shorthand: accent [`Style`] from the current global theme.
pub fn accent() -> Style {
    current().accent_style()
}

/// Shorthand: success [`Style`] from the current global theme.
pub fn success() -> Style {
    current().success_style()
}

/// Shorthand: error [`Style`] from the current global theme.
pub fn error() -> Style {
    current().error_style()
}

/// Shorthand: brand [`Style`] from the current global theme.
pub fn brand() -> Style {
    current().brand_style()
}

/// Shorthand: info [`Style`] from the current global theme.
pub fn info() -> Style {
    current().info_style()
}

/// Returns the kebab-case names of every built-in theme.
pub fn builtin_theme_names() -> Vec<&'static str> {
    BuiltinTheme::iter().map(Into::into).collect()
}

/// Looks up a built-in theme by name, adapting it for the detected terminal
/// background when available.
pub fn builtin_theme_for_terminal(name: &str) -> Option<Theme> {
    let builtin = BuiltinTheme::from_name(name)?;
    let theme = if let Some(bg) = default_bg() {
        Theme::from_builtin_with_terminal(builtin, bg)
    } else {
        Theme::from_builtin(builtin)
    };
    Some(theme)
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_core::config::theme::ThemeColor;
    use codex_core::config::theme::ThemePalette;
    #[test]
    fn adapts_dark_theme_on_light_terminal() {
        let config = ThemeConfig {
            name: Some("ocean".to_string()),
            palette: None,
            styles: None,
        };
        let theme = resolve_theme_with_terminal_bg(&config, Some((240, 240, 240)));
        assert!(theme.message_bg.is_some());
        assert!(theme.selection_bg.is_some());
    }

    #[test]
    fn adapts_light_theme_on_dark_terminal() {
        let config = ThemeConfig {
            name: Some("catppuccin-light".to_string()),
            palette: None,
            styles: None,
        };
        let theme = resolve_theme_with_terminal_bg(&config, Some((16, 16, 16)));
        assert!(theme.message_bg.is_some());
        assert!(theme.selection_bg.is_some());
    }

    #[test]
    fn skips_adaptation_when_palette_overrides() {
        let config = ThemeConfig {
            name: Some("ocean".to_string()),
            palette: Some(ThemePalette {
                accent: Some(ThemeColor::Named("red".to_string())),
                ..ThemePalette::default()
            }),
            styles: None,
        };
        let theme = resolve_theme_with_terminal_bg(&config, Some((240, 240, 240)));
        assert!(theme.message_bg.is_some());
        assert!(theme.selection_bg.is_some());
    }
}
