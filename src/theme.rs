//! The look: an Orchard color scheme (oc-config: the built-ins plus the
//! user schemes in Orchard Commander's `color-schemes/` folder, so schemes
//! made in its editor show up here), resolved to grid colors, plus the
//! frame style and font.
//!
//! `Colors` mirrors `oc_ui::theme::Colors`; oc-ui itself is the file
//! manager's whole UI and too heavy to depend on.

use std::path::PathBuf;

use oc_config::themes::{self, Appearance, ColorScheme};
use oc_config::ConfigPaths;
use oc_grid::{BoxStyle, FontConfig, Rgba, Style};

/// Every color role of a [`ColorScheme`], parsed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colors {
    pub background: Rgba,
    pub surface: Rgba,
    pub foreground: Rgba,
    pub muted: Rgba,
    pub border: Rgba,
    pub border_active: Rgba,
    pub accent: Rgba,
    pub cursor_bg: Rgba,
    pub cursor_fg: Rgba,
    pub cursor_inactive_bg: Rgba,
    pub dir: Rgba,
    pub link: Rgba,
    pub marked: Rgba,
    pub error: Rgba,
    pub warning: Rgba,
    pub success: Rgba,
    pub footer_bg: Rgba,
    pub footer_fg: Rgba,
    pub footer_key: Rgba,
}

impl Colors {
    pub fn from_scheme(s: &ColorScheme) -> Colors {
        let c = |v: &str, fallback: Rgba| Rgba::parse(v).unwrap_or(fallback);
        let fg = c(&s.foreground, Rgba::WHITE);
        let bg = c(&s.background, Rgba::BLACK);
        Colors {
            background: bg,
            surface: c(&s.surface, bg),
            foreground: fg,
            muted: c(&s.muted, fg),
            border: c(&s.border, fg),
            border_active: c(&s.border_active, fg),
            accent: c(&s.accent, fg),
            cursor_bg: c(&s.cursor_bg, fg),
            cursor_fg: c(&s.cursor_fg, bg),
            cursor_inactive_bg: c(&s.cursor_inactive_bg, bg),
            dir: c(&s.dir, fg),
            link: c(&s.link, fg),
            marked: c(&s.marked, fg),
            error: c(&s.error, fg),
            warning: c(&s.warning, fg),
            success: c(&s.success, fg),
            footer_bg: c(&s.footer_bg, bg),
            footer_fg: c(&s.footer_fg, fg),
            footer_key: c(&s.footer_key, fg),
        }
    }
}

/// Everything drawing needs to know about the look.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub scheme: ColorScheme,
    pub colors: Colors,
    pub frame: BoxStyle,
    pub font: FontConfig,
}

impl Theme {
    pub fn new(appearance: &Appearance, scheme: ColorScheme) -> Theme {
        let font = FontConfig {
            family: appearance.font_family.clone(),
            style: appearance.font_style.clone(),
            size: appearance.font_size as f32,
            line_spacing: appearance.line_spacing as f32,
            fallback: Vec::new(),
        }
        .sanitized();
        Theme {
            colors: Colors::from_scheme(&scheme),
            scheme,
            frame: BoxStyle::from_name(&appearance.frame_style).unwrap_or_default(),
            font,
        }
    }

    /// Built-in default (monokai, default font): tests and fallbacks.
    pub fn default_dark() -> Theme {
        let a = Appearance::default();
        Theme::new(&a, themes::builtin_scheme(&a.color_scheme).cloned().expect("default scheme is built in"))
    }

    pub fn text(&self) -> Style {
        Style::new(self.colors.foreground, self.colors.surface)
    }

    pub fn muted(&self) -> Style {
        Style::new(self.colors.muted, self.colors.surface)
    }

    pub fn accent(&self) -> Style {
        Style::new(self.colors.accent, self.colors.surface)
    }
}

/// The color-schemes folder shared with Orchard Commander.
pub fn schemes_dir() -> PathBuf {
    ConfigPaths::platform().color_schemes_dir()
}

/// Every scheme: built-ins, then the user's (blocking: reads the folder).
pub fn all_schemes() -> Vec<ColorScheme> {
    themes::all_schemes(&schemes_dir())
}

/// Mix two colors: `t` = 0 is `a`, 1 is `b`.
pub fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Rgba::new(l(a.r, b.r), l(a.g, b.g), l(a.b, b.b), l(a.a, b.a))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appearance_maps_to_grid_settings() {
        let a = Appearance { frame_style: "double".into(), font_size: 15, ..Appearance::default() };
        let t = Theme::new(&a, themes::builtin_scheme("gruvbox").cloned().unwrap());
        assert_eq!(t.frame, BoxStyle::Double);
        assert_eq!(t.font.size, 15.0);
        assert_eq!(t.colors.surface, Rgba::hex(0x282828));
    }

    #[test]
    fn mix_ends() {
        assert_eq!(mix(Rgba::BLACK, Rgba::WHITE, 0.0), Rgba::BLACK);
        assert_eq!(mix(Rgba::BLACK, Rgba::WHITE, 1.0), Rgba::WHITE);
    }
}
