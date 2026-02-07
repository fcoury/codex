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
use tracing::warn;

use crate::terminal_palette::best_color;
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

impl BuiltinTheme {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "default" => Some(Self::Default),
            "ocean" => Some(Self::Ocean),
            "rose-pine" => Some(Self::RosePine),
            "solarized" => Some(Self::Solarized),
            "catppuccin-light" => Some(Self::CatppuccinLight),
            _ => None,
        }
    }
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

    fn builtin_theme(name: &str) -> Theme {
        BuiltinTheme::from_name(name)
            .map(Self::from_builtin)
            .unwrap_or_else(Self::default_theme)
    }

    fn from_builtin(builtin: BuiltinTheme) -> Theme {
        match builtin {
            BuiltinTheme::Default => Self::default_theme(),
            BuiltinTheme::Ocean => build_theme(
                hex_color("#7aa2f7"),
                hex_color("#9ece6a"),
                hex_color("#f7768e"),
                hex_color("#bb9af7"),
                hex_color("#7dcfff"),
                Some(hex_color("#1a1b26")),
                Some(hex_color("#283457")),
            ),
            BuiltinTheme::RosePine => build_theme(
                hex_color("#9ccfd8"),
                hex_color("#31748f"),
                hex_color("#eb6f92"),
                hex_color("#c4a7e7"),
                hex_color("#f6c177"),
                Some(hex_color("#191724")),
                Some(hex_color("#26233a")),
            ),
            BuiltinTheme::Solarized => build_theme(
                hex_color("#268bd2"),
                hex_color("#859900"),
                hex_color("#dc322f"),
                hex_color("#d33682"),
                hex_color("#2aa198"),
                Some(hex_color("#002b36")),
                Some(hex_color("#073642")),
            ),
            BuiltinTheme::CatppuccinLight => build_theme(
                hex_color("#1e66f5"),
                hex_color("#40a02b"),
                hex_color("#d20f39"),
                hex_color("#8839ef"),
                hex_color("#209fb5"),
                Some(hex_color("#eff1f5")),
                Some(hex_color("#ccd0da")),
            ),
        }
    }

    pub fn accent_style(&self) -> Style {
        Style::default().fg(self.accent)
    }

    pub fn success_style(&self) -> Style {
        Style::default().fg(self.success)
    }

    pub fn error_style(&self) -> Style {
        Style::default().fg(self.error)
    }

    pub fn brand_style(&self) -> Style {
        Style::default().fg(self.brand)
    }

    pub fn info_style(&self) -> Style {
        Style::default().fg(self.info)
    }

    pub fn success_bold(&self) -> Style {
        self.success_style().add_modifier(Modifier::BOLD)
    }

    pub fn error_bold(&self) -> Style {
        self.error_style().add_modifier(Modifier::BOLD)
    }

    pub fn brand_bold(&self) -> Style {
        self.brand_style().add_modifier(Modifier::BOLD)
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

fn hex_color(hex: &str) -> Color {
    let hex = hex
        .strip_prefix('#')
        .expect("hex color should start with #");
    let r = u8::from_str_radix(&hex[0..2], 16).expect("valid hex color");
    let g = u8::from_str_radix(&hex[2..4], 16).expect("valid hex color");
    let b = u8::from_str_radix(&hex[4..6], 16).expect("valid hex color");
    best_color((r, g, b))
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
    let mut theme = match config.name.as_deref() {
        Some(name) => {
            if BuiltinTheme::from_name(name).is_none() {
                warn!("Unknown theme name: {name}");
            }
            Theme::builtin_theme(name)
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

pub fn accent() -> Style {
    current().accent_style()
}

pub fn success() -> Style {
    current().success_style()
}

pub fn error() -> Style {
    current().error_style()
}

pub fn brand() -> Style {
    current().brand_style()
}

pub fn info() -> Style {
    current().info_style()
}
