/// Returns `true` if the given RGB background would be perceived as light
/// (luma > 128 using BT.601 weights).
pub(crate) fn is_light(bg: (u8, u8, u8)) -> bool {
    let (r, g, b) = bg;
    let y = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
    y > 128.0
}

/// Alpha-blends `fg` over `bg` in sRGB space.
///
/// `alpha = 0.0` returns `bg`; `alpha = 1.0` returns `fg`.
pub(crate) fn blend(fg: (u8, u8, u8), bg: (u8, u8, u8), alpha: f32) -> (u8, u8, u8) {
    let r = (fg.0 as f32 * alpha + bg.0 as f32 * (1.0 - alpha)) as u8;
    let g = (fg.1 as f32 * alpha + bg.1 as f32 * (1.0 - alpha)) as u8;
    let b = (fg.2 as f32 * alpha + bg.2 as f32 * (1.0 - alpha)) as u8;
    (r, g, b)
}

/// Computes WCAG relative luminance for an sRGB color.
pub(crate) fn relative_luminance(rgb: (u8, u8, u8)) -> f32 {
    let (r, g, b) = rgb;
    let r = srgb_u8_to_linear(r);
    let g = srgb_u8_to_linear(g);
    let b = srgb_u8_to_linear(b);
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// Returns the WCAG 2.x contrast ratio between two sRGB colors (range 1..21).
pub(crate) fn contrast_ratio(fg: (u8, u8, u8), bg: (u8, u8, u8)) -> f32 {
    let fg_l = relative_luminance(fg);
    let bg_l = relative_luminance(bg);
    let (light, dark) = if fg_l >= bg_l {
        (fg_l, bg_l)
    } else {
        (bg_l, fg_l)
    };
    (light + 0.05) / (dark + 0.05)
}

/// Adapts an RGB foreground color for readability against `terminal_bg`.
///
/// Inverts perceived lightness in OKLCh space and then binary-searches
/// lightness until the WCAG contrast ratio reaches [`MIN_CONTRAST_RATIO`].
pub(crate) fn adapt_color_for_terminal(
    color: (u8, u8, u8),
    terminal_bg: (u8, u8, u8),
    target_is_dark: bool,
) -> (u8, u8, u8) {
    // Invert perceived lightness while preserving hue/chroma.
    let mut lch = rgb_to_oklch(color);
    lch.l = 1.0 - lch.l;
    lch.l = lch.l.clamp(LIGHTNESS_MIN, LIGHTNESS_MAX);
    lch.c *= if target_is_dark {
        CHROMA_DARK_SCALE
    } else {
        CHROMA_LIGHT_SCALE
    };

    // First pass conversion + contrast check.
    let mut candidate = oklch_to_rgb_with_gamut(lch);
    let mut best_ratio = contrast_ratio(candidate, terminal_bg);
    if best_ratio >= MIN_CONTRAST_RATIO {
        return candidate;
    }

    // Contrast guard: search lightness only to reach the target ratio.
    let (mut low, mut high) = if target_is_dark {
        (lch.l, LIGHTNESS_MAX)
    } else {
        (LIGHTNESS_MIN, lch.l)
    };
    let (low_candidate, low_ratio) = candidate_with_lightness(lch, low, terminal_bg);
    let (high_candidate, high_ratio) = candidate_with_lightness(lch, high, terminal_bg);

    if low_ratio > best_ratio {
        candidate = low_candidate;
        best_ratio = low_ratio;
    }
    if high_ratio > best_ratio {
        candidate = high_candidate;
        best_ratio = high_ratio;
    }

    if best_ratio < MIN_CONTRAST_RATIO {
        return high_contrast_fallback(terminal_bg);
    }

    for _ in 0..LIGHTNESS_SEARCH_STEPS {
        let mid = (low + high) * 0.5;
        let (mid_candidate, mid_ratio) = candidate_with_lightness(lch, mid, terminal_bg);
        if mid_ratio >= MIN_CONTRAST_RATIO {
            candidate = mid_candidate;
            if target_is_dark {
                high = mid;
            } else {
                low = mid;
            }
        } else if target_is_dark {
            low = mid;
        } else {
            high = mid;
        }
    }

    candidate
}

/// Returns the perceptual color distance between two RGB colors.
/// Uses the CIE76 formula (Euclidean distance in Lab space approximation).
pub(crate) fn perceptual_distance(a: (u8, u8, u8), b: (u8, u8, u8)) -> f32 {
    // Convert sRGB to linear RGB
    fn srgb_to_linear(c: u8) -> f32 {
        let c = c as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    // Convert RGB to XYZ
    fn rgb_to_xyz(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
        let r = srgb_to_linear(r);
        let g = srgb_to_linear(g);
        let b = srgb_to_linear(b);

        let x = r * 0.4124 + g * 0.3576 + b * 0.1805;
        let y = r * 0.2126 + g * 0.7152 + b * 0.0722;
        let z = r * 0.0193 + g * 0.1192 + b * 0.9505;
        (x, y, z)
    }

    // Convert XYZ to Lab
    fn xyz_to_lab(x: f32, y: f32, z: f32) -> (f32, f32, f32) {
        // D65 reference white
        let xr = x / 0.95047;
        let yr = y / 1.00000;
        let zr = z / 1.08883;

        fn f(t: f32) -> f32 {
            if t > 0.008856 {
                t.powf(1.0 / 3.0)
            } else {
                7.787 * t + 16.0 / 116.0
            }
        }

        let fx = f(xr);
        let fy = f(yr);
        let fz = f(zr);

        let l = 116.0 * fy - 16.0;
        let a = 500.0 * (fx - fy);
        let b = 200.0 * (fy - fz);
        (l, a, b)
    }

    let (x1, y1, z1) = rgb_to_xyz(a.0, a.1, a.2);
    let (x2, y2, z2) = rgb_to_xyz(b.0, b.1, b.2);

    let (l1, a1, b1) = xyz_to_lab(x1, y1, z1);
    let (l2, a2, b2) = xyz_to_lab(x2, y2, z2);

    let dl = l1 - l2;
    let da = a1 - a2;
    let db = b1 - b2;

    (dl * dl + da * da + db * db).sqrt()
}

const MIN_CONTRAST_RATIO: f32 = 4.5;
const LIGHTNESS_MIN: f32 = 0.15;
const LIGHTNESS_MAX: f32 = 0.95;
const CHROMA_DARK_SCALE: f32 = 0.85;
const CHROMA_LIGHT_SCALE: f32 = 1.05;
const LIGHTNESS_SEARCH_STEPS: usize = 20;
const GAMUT_ADJUST_STEPS: usize = 32;

#[derive(Clone, Copy, Debug)]
struct Oklab {
    l: f32,
    a: f32,
    b: f32,
}

#[derive(Clone, Copy, Debug)]
struct Oklch {
    l: f32,
    c: f32,
    h: f32,
}

fn candidate_with_lightness(
    mut lch: Oklch,
    lightness: f32,
    terminal_bg: (u8, u8, u8),
) -> ((u8, u8, u8), f32) {
    lch.l = lightness;
    let rgb = oklch_to_rgb_with_gamut(lch);
    let ratio = contrast_ratio(rgb, terminal_bg);
    (rgb, ratio)
}

fn high_contrast_fallback(terminal_bg: (u8, u8, u8)) -> (u8, u8, u8) {
    let black = (0, 0, 0);
    let white = (255, 255, 255);
    if contrast_ratio(black, terminal_bg) >= contrast_ratio(white, terminal_bg) {
        black
    } else {
        white
    }
}

fn rgb_to_oklch(rgb: (u8, u8, u8)) -> Oklch {
    let lab = rgb_to_oklab(rgb);
    let c = (lab.a * lab.a + lab.b * lab.b).sqrt();
    let h = lab.b.atan2(lab.a);
    Oklch { l: lab.l, c, h }
}

fn rgb_to_oklab(rgb: (u8, u8, u8)) -> Oklab {
    let (r, g, b) = rgb;
    let r = srgb_u8_to_linear(r);
    let g = srgb_u8_to_linear(g);
    let b = srgb_u8_to_linear(b);

    let l = 0.412_221_46 * r + 0.536_332_54 * g + 0.051_445_994 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;

    let l = l.cbrt();
    let m = m.cbrt();
    let s = s.cbrt();

    Oklab {
        l: 0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        a: 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        b: 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    }
}

fn oklch_to_rgb_with_gamut(lch: Oklch) -> (u8, u8, u8) {
    let mut chroma = lch.c;
    for _ in 0..GAMUT_ADJUST_STEPS {
        let linear = oklch_to_linear_rgb(Oklch { c: chroma, ..lch });
        if linear_in_gamut(linear) {
            return linear_rgb_to_srgb_u8(linear);
        }
        // Desaturate until we land in sRGB gamut.
        chroma *= 0.98;
        if chroma <= 0.0 {
            break;
        }
    }

    let linear = oklch_to_linear_rgb(Oklch { c: 0.0, ..lch });
    linear_rgb_to_srgb_u8(linear)
}

fn oklch_to_linear_rgb(lch: Oklch) -> (f32, f32, f32) {
    let a = lch.c * lch.h.cos();
    let b = lch.c * lch.h.sin();
    let lab = Oklab { l: lch.l, a, b };
    oklab_to_linear_rgb(lab)
}

fn oklab_to_linear_rgb(lab: Oklab) -> (f32, f32, f32) {
    let l = lab.l + 0.396_337_78 * lab.a + 0.215_803_76 * lab.b;
    let m = lab.l - 0.105_561_346 * lab.a - 0.063_854_17 * lab.b;
    let s = lab.l - 0.089_484_18 * lab.a - 1.291_485_5 * lab.b;

    let l = l * l * l;
    let m = m * m * m;
    let s = s * s * s;

    let r = 4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s;
    let g = -1.268_438 * l + 2.609_757_4 * m - 0.341_319_4 * s;
    let b = -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s;
    (r, g, b)
}

fn linear_in_gamut(rgb: (f32, f32, f32)) -> bool {
    let (r, g, b) = rgb;
    (0.0..=1.0).contains(&r) && (0.0..=1.0).contains(&g) && (0.0..=1.0).contains(&b)
}

fn linear_rgb_to_srgb_u8(rgb: (f32, f32, f32)) -> (u8, u8, u8) {
    let (r, g, b) = rgb;
    (
        srgb_f32_to_u8(linear_to_srgb_f32(r)),
        srgb_f32_to_u8(linear_to_srgb_f32(g)),
        srgb_f32_to_u8(linear_to_srgb_f32(b)),
    )
}

fn srgb_u8_to_linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb_f32(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn srgb_f32_to_u8(c: f32) -> u8 {
    let c = (c.clamp(0.0, 1.0) * 255.0).round();
    c as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn contrast_ratio_black_white() {
        let ratio = contrast_ratio((0, 0, 0), (255, 255, 255));
        assert!((ratio - 21.0).abs() < 0.2);
    }

    #[test]
    fn oklch_round_trip_is_close() {
        let samples = [
            (122, 162, 247),
            (220, 15, 57),
            (64, 160, 43),
            (32, 159, 181),
        ];
        for rgb in samples {
            let lch = rgb_to_oklch(rgb);
            let round_trip = oklch_to_rgb_with_gamut(lch);
            let delta = (
                (rgb.0 as i16 - round_trip.0 as i16).abs(),
                (rgb.1 as i16 - round_trip.1 as i16).abs(),
                (rgb.2 as i16 - round_trip.2 as i16).abs(),
            );
            assert!(delta.0 <= 2 && delta.1 <= 2 && delta.2 <= 2);
        }
    }

    #[test]
    fn adapt_color_increases_contrast_on_light_bg() {
        let fg = (122, 162, 247);
        let bg = (240, 240, 240);
        let before = contrast_ratio(fg, bg);
        let after = contrast_ratio(adapt_color_for_terminal(fg, bg, false), bg);
        assert!(after >= MIN_CONTRAST_RATIO);
        assert!(after >= before);
    }

    #[test]
    fn adapt_color_increases_contrast_on_dark_bg() {
        let fg = (30, 120, 200);
        let bg = (18, 18, 18);
        let before = contrast_ratio(fg, bg);
        let after = contrast_ratio(adapt_color_for_terminal(fg, bg, true), bg);
        assert!(after >= MIN_CONTRAST_RATIO);
        assert!(after >= before);
    }

    #[test]
    fn srgb_linear_round_trip() {
        let value = 0.4;
        let round_trip = linear_to_srgb_f32(srgb_u8_to_linear(srgb_f32_to_u8(value)));
        assert!((round_trip - value).abs() < 0.02);
        assert_eq!(srgb_f32_to_u8(0.0), 0);
    }
}
