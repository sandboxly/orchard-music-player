//! Settings (⌘, / Ctrl+, or F9), modelled on Orchard Commander's: pages on
//! the left, their controls on the right. Changes apply and are saved as
//! they are made (no OK button: the player behind is the preview). The
//! color scheme editor is in [`scheme_editor`], its color picker in
//! [`color_picker`].
//!
//! Keys: Tab / Shift+Tab and ↑/↓ move between controls, ←/→ change a list
//! or number, Enter/Space open a list, toggle or press; Esc closes. A number
//! can also be typed or turned with the mouse wheel.
//!
//! The dialog keeps only UI state (page, focus, open list, the editor);
//! every value is read from `app.settings` whenever a page is laid out, so
//! it always shows what is saved.
//!
//! # Adding a setting
//!
//! 1. Add the field to [`Settings`](crate::settings::Settings) (with its
//!    default) and make the player follow it (in `App::apply_settings` if it
//!    is derived state, or wherever it is read).
//! 2. Describe its control in the page's function ([`playback_page`],
//!    [`appearance_page`]): a [`toggle`], [`choice`] or [`number`] with an
//!    id, a label, a hint, the current value read from the settings and a
//!    setter. For example, a gapless switch on the Playback page:
//!
//!    ```ignore
//!    Row::Line(vec![toggle("gapless", "Gapless playback", "Starts the next track without a pause",
//!        s.gapless, |app, on| app.update_settings(|s| s.gapless = on))]),
//!    ```
//!
//! That's all: focus order, keys, mouse, drawing, saving and "Restore
//! defaults" (which lays the page out for `Settings::default()` and hands
//! each control's value to its setter) come with the control. A new page is
//! an entry in [`PAGES`].

pub mod color_picker;
pub mod form;
pub mod sample;
pub mod scheme_editor;

use std::collections::{HashMap, HashSet};

use oc_config::ColorScheme;
use oc_config::themes::{self, FONT_SIZE_RANGE, FRAME_STYLES, LINE_SPACING_RANGE};
use oc_grid::{CellRect, Frame, Style};

use self::form::{Choice, Popup, PopupOutcome};
use self::scheme_editor::{EditorOutcome, SchemeEditor};
use crate::app::{App, Modal};
use crate::input::{Key, KeyEvent, MouseEvent, MouseKind, Named};
use crate::meta::GainMode;
use crate::settings::Settings;
use crate::ui::widgets::{DrawCtx, bottom, clip, contains, fill, is_empty, put_elided, right, width};

pub const TITLE: &str = "Settings";

/// A page: its name in the list on the left, and what is on it.
pub struct Page {
    pub name: &'static str,
    pub build: fn(&Settings, &Env) -> Vec<Row>,
    /// Shows the sample next to the form, if there is room.
    pub sample: bool,
}

pub const PAGES: &[Page] = &[
    Page { name: "Appearance", build: appearance_page, sample: true },
    Page { name: "Playback", build: playback_page, sample: false },
];

const WIDTH: u16 = 108;
const SECTIONS_W: u16 = 14;
const PAGE_ROWS: u16 = 15;
const LABEL_W: u16 = 14;
const FIELD_W: u16 = 26;
const SPIN_W: u16 = 12;
const RESTORE: &str = "Restore defaults";

/// "⌘" on macOS, "Ctrl+" elsewhere, for hints.
fn primary() -> &'static str {
    if cfg!(target_os = "macos") { "⌘" } else { "Ctrl+" }
}

/// Opens the settings dialog.
pub fn open(app: &mut App) {
    load_fonts(app);
    // Re-read the folder: Orchard Commander may have saved a scheme meanwhile.
    app.refresh_schemes();
    app.modal = Some(Modal::Settings(Box::default()));
}

// ---------------------------------------------------------------------------
// Fonts (looked up on a worker)
// ---------------------------------------------------------------------------

/// Installed monospaced font families and their styles, filled in the
/// background (scanning the fonts takes a moment).
#[derive(Debug, Default)]
pub struct FontInfo {
    pub mono: Option<Vec<String>>,
    /// The family "Automatic" stands for.
    pub automatic: Option<String>,
    /// Style names by family.
    pub styles: HashMap<String, Vec<String>>,
    requested: HashSet<String>,
    started: bool,
}

fn load_fonts(app: &mut App) {
    if std::mem::replace(&mut app.fonts.started, true) {
        request_styles(app);
        return;
    }
    app.tasks.spawn(
        || {
            let lib = oc_grid::FontLibrary::system();
            (lib.families(true), lib.resolve_family(""))
        },
        |app, (mono, automatic)| {
            app.fonts.mono = Some(mono);
            app.fonts.automatic = automatic;
            request_styles(app);
        },
    );
}

/// The same lookups, done right away (screenshots).
pub fn load_fonts_now(app: &mut App) {
    let lib = oc_grid::FontLibrary::system();
    app.fonts.mono = Some(lib.families(true));
    app.fonts.automatic = lib.resolve_family("");
    app.fonts.started = true;
    if let Some(family) = styled_family(&app.settings, &app.fonts) {
        app.fonts.requested.insert(family.clone());
        app.fonts.styles.insert(family.clone(), lib.styles(&family));
    }
}

/// The family the Style list is for: the chosen one, else the automatic one.
fn styled_family(s: &Settings, fonts: &FontInfo) -> Option<String> {
    if !s.font_family.is_empty() {
        return Some(s.font_family.clone());
    }
    fonts.automatic.clone()
}

/// Look up the styles of the current family if not known yet.
fn request_styles(app: &mut App) {
    if !app.fonts.started {
        return;
    }
    let Some(family) = styled_family(&app.settings, &app.fonts) else { return };
    if !app.fonts.requested.insert(family.clone()) {
        return;
    }
    app.tasks.spawn(
        {
            let family = family.clone();
            move || oc_grid::FontLibrary::system().styles(&family)
        },
        move |app, styles| {
            app.fonts.styles.insert(family, styles);
        },
    );
}

// ---------------------------------------------------------------------------
// Controls
// ---------------------------------------------------------------------------

/// What a page's controls can know besides the settings.
pub struct Env<'a> {
    pub schemes: &'a [ColorScheme],
    pub fonts: &'a FontInfo,
}

type Setter<T> = Box<dyn Fn(&mut App, T)>;
type Action = Box<dyn Fn(&mut App, &mut SettingsDialog)>;

/// One control: its focus id, its label (left of it; a checkbox's or
/// button's own text), the hint under the page while it has the focus.
pub struct Ctl {
    pub id: &'static str,
    pub label: &'static str,
    pub hint: String,
    pub kind: Kind,
}

pub enum Kind {
    Toggle {
        on: bool,
        set: Setter<bool>,
    },
    /// A list: labels (None: separator), the setting value of each, the
    /// shown one, and the setting as stored (for "Restore defaults").
    Choice {
        items: Vec<Choice>,
        values: Vec<String>,
        current: usize,
        value: String,
        set: Setter<String>,
    },
    Number {
        value: f64,
        range: (f64, f64),
        step: f64,
        decimals: usize,
        unit: &'static str,
        set: Setter<f64>,
    },
    Button {
        press: Action,
    },
}

/// A page's content, top to bottom.
pub enum Row {
    Heading(&'static str),
    /// Controls side by side; the first one gets the label column.
    Line(Vec<Ctl>),
}

/// A checkbox.
pub fn toggle(id: &'static str, label: &'static str, hint: &str, on: bool, set: impl Fn(&mut App, bool) + 'static) -> Ctl {
    Ctl { id, label, hint: hint.into(), kind: Kind::Toggle { on, set: Box::new(set) } }
}

/// A list of (label, setting value); `value` is the setting as stored.
pub fn choice(
    id: &'static str,
    label: &'static str,
    hint: &str,
    options: &[(&str, &str)],
    value: &str,
    set: impl Fn(&mut App, String) + 'static,
) -> Ctl {
    let items = options.iter().map(|(l, _)| Some(l.to_string())).collect();
    let values = options.iter().map(|(_, v)| v.to_string()).collect();
    choice_of(id, label, hint, items, values, value, set)
}

/// A list with separators (`None` items) and computed labels.
pub fn choice_of(
    id: &'static str,
    label: &'static str,
    hint: &str,
    items: Vec<Choice>,
    values: Vec<String>,
    value: &str,
    set: impl Fn(&mut App, String) + 'static,
) -> Ctl {
    let current = values.iter().position(|v| v == value).unwrap_or(0);
    Ctl { id, label, hint: hint.into(), kind: Kind::Choice { items, values, current, value: value.into(), set: Box::new(set) } }
}

/// A number from `range`, changed in `step`s; shown with `decimals` and `unit`.
#[allow(clippy::too_many_arguments)]
pub fn number(
    id: &'static str,
    label: &'static str,
    hint: &str,
    value: f64,
    range: (f64, f64),
    step: f64,
    decimals: usize,
    unit: &'static str,
    set: impl Fn(&mut App, f64) + 'static,
) -> Ctl {
    Ctl { id, label, hint: hint.into(), kind: Kind::Number { value, range, step, decimals, unit, set: Box::new(set) } }
}

/// A push button.
pub fn button(id: &'static str, label: &'static str, hint: &str, press: impl Fn(&mut App, &mut SettingsDialog) + 'static) -> Ctl {
    Ctl { id, label, hint: hint.into(), kind: Kind::Button { press: Box::new(press) } }
}

/// Which color scheme setting a row is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    /// `color_scheme`
    Color,
    /// `light_scheme`
    Light,
    /// `dark_scheme`
    Dark,
}

impl Slot {
    pub fn label(self) -> &'static str {
        match self {
            Slot::Color => "Scheme",
            Slot::Light => "Light mode",
            Slot::Dark => "Dark mode",
        }
    }

    fn ids(self) -> [&'static str; 3] {
        match self {
            Slot::Color => ["scheme", "scheme.edit", "scheme.create"],
            Slot::Light => ["scheme.light", "scheme.light.edit", "scheme.light.create"],
            Slot::Dark => ["scheme.dark", "scheme.dark.edit", "scheme.dark.create"],
        }
    }

    pub fn value(self, s: &Settings) -> &str {
        match self {
            Slot::Color => &s.color_scheme,
            Slot::Light => &s.light_scheme,
            Slot::Dark => &s.dark_scheme,
        }
    }

    pub fn set(self, s: &mut Settings, name: &str) {
        let field = match self {
            Slot::Color => &mut s.color_scheme,
            Slot::Light => &mut s.light_scheme,
            Slot::Dark => &mut s.dark_scheme,
        };
        *field = name.to_owned();
    }

    pub const ALL: [Slot; 3] = [Slot::Color, Slot::Light, Slot::Dark];
}

/// The Appearance page.
pub fn appearance_page(s: &Settings, env: &Env) -> Vec<Row> {
    let mut rows = vec![Row::Heading("Color scheme")];
    rows.push(Row::Line(vec![toggle(
        "follow_system",
        "Follow the system's light / dark mode",
        "Light and dark mode each get a color scheme of their own",
        s.follow_system_appearance,
        |app, on| app.update_settings(|s| s.follow_system_appearance = on),
    )]));
    let builtin: Vec<&str> = env.schemes.iter().map(|s| s.name.as_str()).filter(|n| themes::is_builtin(n)).collect();
    let custom: Vec<&str> = env.schemes.iter().map(|s| s.name.as_str()).filter(|n| !themes::is_builtin(n)).collect();
    let mut items: Vec<Choice> = builtin.iter().map(|n| Some(n.to_string())).collect();
    if !custom.is_empty() {
        items.push(None);
        items.extend(custom.iter().map(|n| Some(n.to_string())));
    }
    let values: Vec<String> = items.iter().map(|i| i.clone().unwrap_or_default()).collect();
    let slots: &[Slot] = if s.follow_system_appearance { &[Slot::Light, Slot::Dark] } else { &[Slot::Color] };
    for &slot in slots {
        let [id, edit, create] = slot.ids();
        rows.push(Row::Line(vec![
            choice_of(id, slot.label(), "Also T: a quick list that previews each scheme", items.clone(), values.clone(), slot.value(s), move |app, v| {
                app.update_settings(|s| slot.set(s, &v))
            }),
            button(edit, "Edit…", "Change this scheme's colors; a built-in scheme is saved as a copy", move |app, d| {
                d.editor = Some(SchemeEditor::open(app, slot, false))
            }),
            button(create, "Create…", "A new color scheme, starting from this one", move |app, d| {
                d.editor = Some(SchemeEditor::open(app, slot, true))
            }),
        ]));
    }

    rows.push(Row::Heading("Font"));
    let auto_label = match &env.fonts.automatic {
        Some(f) => format!("Automatic ({f})"),
        None => "Automatic".to_owned(),
    };
    let mut items: Vec<Choice> = vec![Some(auto_label)];
    let mut values = vec![String::new()];
    let mut families = env.fonts.mono.clone().unwrap_or_default();
    if !s.font_family.is_empty() && !families.contains(&s.font_family) {
        families.push(s.font_family.clone()); // chosen by hand, or the list isn't there yet
    }
    for f in families {
        items.push(Some(f.clone()));
        values.push(f);
    }
    let hint = if env.fonts.mono.is_some() { "Monospaced fonts installed on this computer" } else { "Looking up the installed fonts…" };
    rows.push(Row::Line(vec![choice_of("font_family", "Typeface", hint, items, values, &s.font_family, |app, v| {
        app.update_settings(|s| {
            s.font_family = v;
            s.font_style = String::new();
        });
        request_styles(app);
    })]));
    let styles = styled_family(s, env.fonts)
        .and_then(|f| env.fonts.styles.get(&f).cloned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| vec!["Regular".to_owned()]);
    let shown = if styles.contains(&s.font_style) { s.font_style.clone() } else { "Regular".to_owned() };
    let mut style = choice_of("font_style", "Style", "", styles.iter().cloned().map(Some).collect(), styles, &shown, |app, v| {
        app.update_settings(|s| s.font_style = v)
    });
    if let Kind::Choice { value, .. } = &mut style.kind {
        *value = s.font_style.clone();
    }
    rows.push(Row::Line(vec![style]));
    let zoom = format!("Also {p}+ / {p}− / {p}0", p = primary());
    let (lo, hi) = FONT_SIZE_RANGE;
    rows.push(Row::Line(vec![number("font_size", "Size", &zoom, s.font_size as f64, (lo as f64, hi as f64), 1.0, 0, "pt", |app, v| {
        app.update_settings(|s| s.font_size = v.round() as i64)
    })]));
    rows.push(Row::Line(vec![number(
        "line_spacing",
        "Line spacing",
        "Row height, as a multiple of the font's",
        s.line_spacing,
        LINE_SPACING_RANGE,
        0.05,
        2,
        "×",
        |app, v| app.update_settings(|s| s.line_spacing = (v * 100.0).round() / 100.0),
    )]));

    rows.push(Row::Heading("Window"));
    let frames: Vec<(&str, &str)> = FRAME_STYLES.iter().map(|(value, label)| (*label, *value)).collect();
    rows.push(Row::Line(vec![choice("frame_style", "Frames", "The lines around panels and dialogs", &frames, &s.frame_style, |app, v| {
        app.update_settings(|s| s.frame_style = v)
    })]));
    rows
}

/// The Playback page.
pub fn playback_page(s: &Settings, _env: &Env) -> Vec<Row> {
    vec![
        Row::Heading("Sound"),
        Row::Line(vec![choice(
            "replay_gain",
            "ReplayGain",
            "Evens out loudness using your files' ReplayGain tags. Album keeps the loudness steps within an album. Also the G key",
            &[("Off", "off"), ("Track", "track"), ("Album", "album")],
            &s.replay_gain,
            |app, v| app.set_gain_mode(GainMode::from_label(&v)),
        )]),
    ]
}

/// Every control on `page` back to its default: the page laid out for the
/// default settings hands each control's value to its setter.
fn restore_defaults(app: &mut App, page: usize) {
    let defaults = Settings::default();
    let ctls: Vec<Ctl> = {
        let env = Env { schemes: &app.schemes, fonts: &app.fonts };
        (PAGES[page].build)(&defaults, &env)
            .into_iter()
            .flat_map(|r| match r {
                Row::Line(c) => c,
                Row::Heading(_) => Vec::new(),
            })
            .collect()
    };
    for c in ctls {
        match c.kind {
            Kind::Toggle { on, set } => set(app, on),
            Kind::Choice { value, set, .. } => set(app, value),
            Kind::Number { value, set, .. } => set(app, value),
            Kind::Button { .. } => {}
        }
    }
    if PAGES[page].name == "Appearance" {
        // Hidden while not following the system.
        app.update_settings(|s| {
            s.light_scheme = defaults.light_scheme.clone();
            s.dark_scheme = defaults.dark_scheme.clone();
        });
    }
}

// ---------------------------------------------------------------------------
// The dialog
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Sections,
    Ctl(&'static str),
    Close,
}

pub struct SettingsDialog {
    pub page: usize,
    pub focus: Focus,
    /// An open list and the control it belongs to.
    popup: Option<(&'static str, Popup)>,
    /// A number being typed, as typed.
    typed: Option<(&'static str, String)>,
    /// The color scheme editor, over the dialog.
    pub editor: Option<SchemeEditor>,
}

struct Rects {
    sections: CellRect,
    page: CellRect,
    hint: CellRect,
    buttons: CellRect,
    outer: CellRect,
}

/// A page's controls and texts, placed.
struct PageLayout {
    ctls: Vec<(CellRect, Ctl)>,
    /// (where, text, heading?)
    texts: Vec<(CellRect, String, bool)>,
    sample: Option<CellRect>,
}

impl PageLayout {
    fn ctl(&self, id: &str) -> Option<&(CellRect, Ctl)> {
        self.ctls.iter().find(|(_, c)| c.id == id)
    }
}

fn screen(app: &App) -> CellRect {
    CellRect::new(0, 0, app.cols, app.rows)
}

fn rects(screen: CellRect) -> Rects {
    let (outer, content) = form::dialog_rects(screen, WIDTH, PAGE_ROWS + 5);
    let buttons = CellRect::new(content.col, bottom(content).saturating_sub(1), content.cols, 1);
    // A short window loses the hint's second line, then the hint.
    let hint_h = match content.rows {
        h if h >= PAGE_ROWS + 5 => 2,
        h if h >= 10 => 1,
        _ => 0,
    };
    let page_h = content.rows.saturating_sub(2 + if hint_h > 0 { hint_h + 1 } else { 0 }).min(PAGE_ROWS);
    let page_x = content.col + SECTIONS_W + 2;
    let page = CellRect::new(page_x, content.row, right(content).saturating_sub(page_x), page_h);
    Rects {
        outer,
        sections: CellRect::new(content.col, content.row, SECTIONS_W.min(content.cols), page_h),
        hint: CellRect::new(page.col, bottom(page) + 1, page.cols, hint_h),
        page,
        buttons,
    }
}

/// Lay `rows` out in `page`, the sample on the right if wanted and there is room.
fn place(rows: Vec<Row>, page: CellRect, with_sample: bool, page_index: usize) -> PageLayout {
    let (x, fx) = (page.col, page.col + LABEL_W);
    // Lists get narrower in a narrow window, so the buttons beside them still fit.
    let trailing = rows
        .iter()
        .filter_map(|r| match r {
            Row::Line(line) => Some(line.iter().skip(1).map(|c| form::button_w(c.label) + 1).sum::<u16>()),
            Row::Heading(_) => None,
        })
        .max()
        .unwrap_or(0);
    let field_w = FIELD_W.min(page.cols.saturating_sub(LABEL_W + trailing)).max(14);
    let mut y = page.row;
    let mut ctls = Vec::new();
    let mut texts = Vec::new();
    let mut form_right = x;
    for row in rows {
        match row {
            Row::Heading(text) => {
                if y > page.row {
                    y += 1;
                }
                texts.push((CellRect::new(x, y, width(text), 1), text.to_string(), true));
                y += 1;
            }
            Row::Line(line) => {
                let mut cx = x;
                for (k, ctl) in line.into_iter().enumerate() {
                    let natural = match &ctl.kind {
                        Kind::Toggle { .. } => 4 + width(ctl.label),
                        Kind::Choice { .. } => field_w,
                        Kind::Number { .. } => SPIN_W,
                        Kind::Button { .. } => form::button_w(ctl.label),
                    };
                    let col = match (&ctl.kind, k) {
                        (Kind::Toggle { .. }, 0) => x,
                        (_, 0) => {
                            texts.push((CellRect::new(x, y, LABEL_W - 1, 1), ctl.label.to_string(), false));
                            fx
                        }
                        _ => cx + 1,
                    };
                    let r = CellRect::new(col, y, natural, 1);
                    cx = right(r);
                    ctls.push((r, ctl));
                }
                form_right = form_right.max(cx);
                y += 1;
            }
        }
    }
    let sample_w = right(page).saturating_sub(form_right + 2).min(sample::MAX_WIDTH);
    let sample = (with_sample && sample_w >= sample::MIN_WIDTH && page.rows >= sample::HEIGHT + 2).then(|| {
        let sx = right(page) - sample_w;
        texts.push((CellRect::new(sx, page.row, sample_w, 1), "Sample".to_string(), true));
        CellRect::new(sx, page.row + 1, sample_w, sample::HEIGHT)
    });
    let rw = form::button_w(RESTORE);
    let restore = CellRect::new(right(page).saturating_sub(rw), bottom(page).saturating_sub(1), rw, 1);
    ctls.push((
        restore,
        button("restore", RESTORE, "Sets everything on this page back to how it came", move |app, _| restore_defaults(app, page_index)),
    ));
    PageLayout { ctls, texts, sample }
}

fn number_text(value: f64, decimals: usize, unit: &str) -> String {
    format!("{value:.decimals$} {unit}")
}

impl Default for SettingsDialog {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsDialog {
    pub fn new() -> Self {
        SettingsDialog { page: 0, focus: Focus::Sections, popup: None, typed: None, editor: None }
    }

    fn layout(&self, app: &App, page: CellRect) -> PageLayout {
        let env = Env { schemes: &app.schemes, fonts: &app.fonts };
        let p = &PAGES[self.page];
        let mut l = place((p.build)(&app.settings, &env), page, p.sample, self.page);
        // In a small window nothing may reach out of the page.
        for (r, _) in &mut l.ctls {
            *r = clip(*r, page);
        }
        for (r, _, _) in &mut l.texts {
            *r = clip(*r, page);
        }
        l
    }

    /// Sections, the page's controls, Close.
    fn order(&self, l: &PageLayout) -> Vec<Focus> {
        let mut v = vec![Focus::Sections];
        v.extend(l.ctls.iter().map(|(_, c)| Focus::Ctl(c.id)));
        v.push(Focus::Close);
        v
    }

    fn set_focus(&mut self, focus: Focus) {
        if focus != self.focus {
            // Leaving a number: it shows the value in effect again.
            self.typed = None;
        }
        self.focus = focus;
    }

    fn move_focus(&mut self, l: &PageLayout, delta: i32, wrap: bool) {
        let order = self.order(l);
        let i = order.iter().position(|f| *f == self.focus).unwrap_or(order.len() - 1) as i32;
        let n = order.len() as i32;
        let next = if wrap { (i + delta).rem_euclid(n) } else { (i + delta).clamp(0, n - 1) };
        self.set_focus(order[next as usize]);
    }

    fn set_page(&mut self, page: usize) {
        self.set_focus(Focus::Sections);
        self.page = page.min(PAGES.len() - 1);
        self.popup = None;
    }

    /// Enter / Space / a click on a control.
    fn activate(&mut self, app: &mut App, r: CellRect, ctl: &Ctl) {
        match &ctl.kind {
            Kind::Toggle { on, set } => set(app, !on),
            Kind::Choice { items, current, .. } => self.popup = Some((ctl.id, Popup::open(r, items, *current))),
            Kind::Button { press } => press(app, self),
            Kind::Number { .. } => {}
        }
    }

    /// ←/→ (or the wheel, a click) on a number.
    fn spin(&mut self, app: &mut App, ctl: &Ctl, steps: i32) {
        self.typed = None;
        if let Kind::Number { value, range, step, decimals, set, .. } = &ctl.kind {
            let scale = 10f64.powi(*decimals as i32);
            let v = ((value + steps as f64 * step).clamp(range.0, range.1) * scale).round() / scale;
            if (v - value).abs() > 1e-9 {
                set(app, v);
            }
        }
    }

    /// A key typed into a focused number: digits (a decimal point if it has
    /// decimals), Backspace. False if it isn't one of those.
    fn type_number(&mut self, app: &mut App, ctl: &Ctl, k: &KeyEvent) -> bool {
        let Kind::Number { value, range, decimals, set, .. } = &ctl.kind else { return false };
        let current = match &self.typed {
            Some((t, text)) if *t == ctl.id => Some(text.clone()),
            _ => None,
        };
        let text = match (&k.key, k.typed()) {
            (Key::Named(Named::Backspace), _) if k.mods.none() => {
                // Without typing first, Backspace edits the number shown.
                let mut text = current.unwrap_or_else(|| format!("{value:.decimals$}"));
                text.pop();
                text
            }
            // The first digit replaces the number.
            (_, Some(t)) if t.chars().all(|c| c.is_ascii_digit() || (*decimals > 0 && matches!(c, '.' | ','))) => {
                current.unwrap_or_default() + t
            }
            _ => return false,
        };
        if let Some((text, v)) = typed_value(&text, *range, *decimals) {
            self.typed = Some((ctl.id, text));
            if let Some(v) = v.filter(|v| (v - value).abs() > 1e-9) {
                set(app, v);
            }
        }
        true
    }

    fn popup_key(&mut self, app: &mut App, l: &PageLayout, ev: Result<&KeyEvent, &MouseEvent>, screen: CellRect) {
        let Some((id, popup)) = self.popup.as_mut() else { return };
        let id = *id;
        let Some((_, Ctl { kind: Kind::Choice { items, values, set, .. }, .. })) = l.ctl(id) else {
            self.popup = None;
            return;
        };
        let outcome = match ev {
            Ok(k) => popup.handle_key(k, items),
            Err(m) => popup.handle_mouse(m, items, screen),
        };
        match outcome {
            PopupOutcome::Chosen(i) => {
                self.popup = None;
                // The list may have changed while open (fonts arriving in the background).
                if let Some(v) = values.get(i) {
                    set(app, v.clone());
                }
            }
            PopupOutcome::Closed => self.popup = None,
            PopupOutcome::Consumed => {}
        }
    }

    /// A key; returns false when the dialog closes.
    pub fn handle_key(&mut self, app: &mut App, k: &KeyEvent) -> bool {
        let screen = screen(app);
        if let Some(editor) = &mut self.editor {
            if editor.handle_key(app, k, screen) == EditorOutcome::Close {
                self.editor = None;
            }
            return true;
        }
        let r = rects(screen);
        let l = self.layout(app, r.page);
        if self.popup.is_some() {
            self.popup_key(app, &l, Ok(k), screen);
            return true;
        }
        let plain = k.mods.none();
        if k.mods.primary() {
            match &k.key {
                Key::Char(',') => return false,
                Key::Char('=' | '+') => app.zoom(1),
                Key::Char('-') => app.zoom(-1),
                Key::Char('0') => app.zoom(0),
                _ => {}
            }
            return true;
        }
        match &k.key {
            Key::Named(Named::Escape | Named::F(9)) => return false,
            Key::Named(Named::Tab) => {
                self.move_focus(&l, if k.mods.shift { -1 } else { 1 }, true);
                return true;
            }
            _ => {}
        }
        match self.focus {
            Focus::Sections => match &k.key {
                Key::Named(Named::Up) => self.set_page(self.page.saturating_sub(1)),
                Key::Named(Named::Down) => self.set_page(self.page + 1),
                Key::Named(Named::Home) => self.set_page(0),
                Key::Named(Named::End) => self.set_page(PAGES.len() - 1),
                Key::Named(Named::Right | Named::Enter) => self.move_focus(&l, 1, false),
                _ => {}
            },
            Focus::Close => match &k.key {
                Key::Named(Named::Enter) | Key::Char(' ') if plain => return false,
                Key::Named(Named::Up | Named::Left) => self.move_focus(&l, -1, false),
                _ => {}
            },
            Focus::Ctl(id) => {
                let Some((cr, ctl)) = l.ctl(id) else {
                    self.focus = Focus::Sections;
                    return true;
                };
                let is_number = matches!(ctl.kind, Kind::Number { .. });
                match (&ctl.kind, &k.key) {
                    (_, Key::Named(Named::Up)) => self.move_focus(&l, -1, false),
                    (_, Key::Named(Named::Down)) => self.move_focus(&l, 1, false),
                    (Kind::Choice { items, values, current, set, .. }, Key::Named(Named::Left | Named::Right)) => {
                        let i = form::step(items, *current, if k.is(Named::Left) { -1 } else { 1 });
                        if i != *current {
                            set(app, values[i].clone());
                        }
                    }
                    _ if is_number && self.type_number(app, ctl, k) => {}
                    (Kind::Number { .. }, Key::Named(Named::Enter)) => self.typed = None,
                    (Kind::Number { .. }, Key::Named(Named::Left | Named::Right)) => {
                        self.spin(app, ctl, if k.is(Named::Left) { -1 } else { 1 })
                    }
                    (Kind::Number { .. }, Key::Named(Named::PageUp | Named::PageDown)) => {
                        self.spin(app, ctl, if k.is(Named::PageDown) { -5 } else { 5 })
                    }
                    (Kind::Number { .. }, _) if matches!(k.typed(), Some("-" | "+" | "=")) => {
                        self.spin(app, ctl, if k.typed() == Some("-") { -1 } else { 1 })
                    }
                    (_, Key::Named(Named::Left)) => self.move_focus(&l, -1, false),
                    (_, Key::Named(Named::Right)) => self.move_focus(&l, 1, false),
                    (_, Key::Named(Named::Enter) | Key::Char(' ')) if plain || k.mods.shift => self.activate(app, *cr, ctl),
                    _ => {}
                }
            }
        }
        true
    }

    /// A click or the wheel; returns false when the dialog closes.
    pub fn handle_mouse(&mut self, app: &mut App, m: &MouseEvent) -> bool {
        let screen = screen(app);
        if let Some(editor) = &mut self.editor {
            if editor.handle_mouse(app, m, screen) == EditorOutcome::Close {
                self.editor = None;
            }
            return true;
        }
        let r = rects(screen);
        let l = self.layout(app, r.page);
        if self.popup.is_some() {
            self.popup_key(app, &l, Err(m), screen);
            return true;
        }
        let (col, row) = (m.col, m.row);
        match m.kind {
            MouseKind::Down => {
                if contains(r.sections, col, row) {
                    let i = (row - r.sections.row) as usize;
                    if i < PAGES.len() {
                        self.set_page(i);
                    }
                } else if let Some((cr, ctl)) = l.ctls.iter().find(|(cr, _)| contains(*cr, col, row)) {
                    self.set_focus(Focus::Ctl(ctl.id));
                    match &ctl.kind {
                        Kind::Number { .. } => self.spin(app, ctl, form::spin_hit(*cr, col)),
                        _ => self.activate(app, *cr, ctl),
                    }
                } else if close_row().hit(r.buttons, col, row).is_some() {
                    return false;
                }
            }
            // The wheel over a number turns it.
            MouseKind::Scroll(lines) if lines != 0.0 => {
                if let Some((_, ctl)) = l.ctls.iter().find(|(cr, c)| contains(*cr, col, row) && matches!(c.kind, Kind::Number { .. })) {
                    self.set_focus(Focus::Ctl(ctl.id));
                    let steps = if lines.abs() < 1.0 { lines.signum() } else { lines.round() };
                    self.spin(app, ctl, steps.clamp(-10.0, 10.0) as i32);
                }
            }
            _ => {}
        }
        true
    }

    pub fn draw(&self, app: &App, f: &mut Frame, ctx: &DrawCtx) {
        let c = &ctx.theme.colors;
        let screen = screen(app);
        let r = rects(screen);
        form::dialog_box(f, r.outer, TITLE, ctx);

        for (i, page) in PAGES.iter().enumerate() {
            let row = CellRect::new(r.sections.col, r.sections.row + i as u16, r.sections.cols, 1);
            if row.row >= bottom(r.sections) {
                break;
            }
            let style = match (i == self.page, self.focus == Focus::Sections && self.editor.is_none()) {
                (true, true) => Style::new(c.cursor_fg, c.cursor_bg),
                (true, false) => Style::new(c.foreground, c.cursor_inactive_bg),
                _ => Style::new(c.foreground, c.surface),
            };
            fill(f, row, style.bg);
            put_elided(f, row.col + 1, row.row, page.name, style, row.cols.saturating_sub(2));
        }

        let l = self.layout(app, r.page);
        for (tr, text, heading) in l.texts.iter().filter(|(r, _, _)| !is_empty(*r)) {
            if *heading { form::heading(f, *tr, text, ctx) } else { form::label(f, *tr, text, ctx) }
        }
        if let Some(sr) = l.sample {
            sample::draw(f, sr, ctx);
        }
        for (cr, ctl) in l.ctls.iter().filter(|(r, _)| !is_empty(*r)) {
            let focused = self.focus == Focus::Ctl(ctl.id) && self.editor.is_none();
            match &ctl.kind {
                Kind::Toggle { on, .. } => form::draw_check(f, *cr, ctl.label, *on, focused, ctx),
                Kind::Choice { items, current, .. } => {
                    let text = items.get(*current).cloned().flatten().unwrap_or_default();
                    form::draw_combo(f, *cr, &text, focused, ctx);
                }
                Kind::Number { value, decimals, unit, .. } => {
                    let text = match &self.typed {
                        Some((id, t)) if *id == ctl.id => format!("{t} {unit}"),
                        _ => number_text(*value, *decimals, unit),
                    };
                    form::draw_spin(f, *cr, &text, focused, ctx);
                }
                Kind::Button { .. } => form::draw_button(f, *cr, ctl.label, focused, ctx),
            }
        }

        // The focused control's hint.
        let hint = match self.focus {
            Focus::Ctl(id) => l.ctl(id).map(|(_, c)| c.hint.as_str()).unwrap_or(""),
            Focus::Sections => "↑↓ page · Tab next control · changes apply and are saved at once",
            Focus::Close => "",
        };
        for (i, line) in form::wrap(hint, r.hint.cols as usize).iter().take(r.hint.rows as usize).enumerate() {
            form::muted(f, CellRect::new(r.hint.col, r.hint.row + i as u16, r.hint.cols, 1), line, ctx);
        }
        close_row().draw(f, r.buttons, ctx, self.focus == Focus::Close && self.editor.is_none());

        if let Some((id, popup)) = &self.popup
            && let Some((_, Ctl { kind: Kind::Choice { items, .. }, .. })) = l.ctl(id)
        {
            popup.draw(f, items, screen, ctx);
        }
        if let Some(e) = &self.editor {
            e.draw(app, f, screen, ctx);
        }
    }
}

fn close_row() -> form::ButtonRow {
    form::ButtonRow::new(&["Close"])
}

/// A number as typed: the text as it may stand (digits, with `decimals` > 0
/// one decimal point and at most that many decimals, never above the
/// maximum), and the value once it is in range.
fn typed_value(text: &str, (lo, hi): (f64, f64), decimals: usize) -> Option<(String, Option<f64>)> {
    let text = text.replace(',', ".");
    if text.is_empty() {
        return Some((text, None));
    }
    let digits = |s: &str| s.chars().all(|c| c.is_ascii_digit());
    let ok = match text.split_once('.') {
        None => digits(&text),
        Some((int, frac)) => decimals > 0 && digits(int) && digits(frac) && frac.len() <= decimals,
    };
    if !ok {
        return None;
    }
    let value: f64 = format!("0{text}").parse().ok()?;
    if value > hi {
        return None;
    }
    Some((text, (value >= lo).then_some(value)))
}

#[cfg(test)]
mod tests;
