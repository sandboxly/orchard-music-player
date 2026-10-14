//! The color scheme editor (port of Orchard Commander's): every color of a
//! scheme as a swatch and a hex field (a click on a swatch, or Space on a
//! focused one, opens the [`ColorPicker`]), its name and dark flag,
//! previewed live on the whole player. Built-in schemes are read-only:
//! changes to them are saved as a new scheme. Schemes are saved with
//! oc-config into the color-schemes folder Orchard Commander reads too;
//! what the player doesn't use (the syntax highlighting style) is kept as
//! it was.

use oc_config::ColorScheme;
use oc_config::themes::{self, COLOR_ROLES};
use oc_grid::{CellRect, Frame, Rgba, Style};

use super::Slot;
use super::color_picker::{self, ColorPicker, PickerOutcome};
use super::form::{self, ButtonRow, RowOutcome};
use crate::app::App;
use crate::input::{Key, KeyEvent, MouseEvent, MouseKind, Named};
use crate::settings::Settings;
use crate::ui::widgets::{DrawCtx, TextField, bottom, contains, fill, put, right};

const SAVE: &str = "Save color scheme";
const DELETE: &str = "Delete color scheme";
const CANCEL: &str = "Cancel";
const WIDTH: u16 = 96;
/// Rows of the taller color column: Files (1 + 5), a gap, Messages (1 + 3),
/// a gap, Key bar (1 + 3).
const COLUMN_ROWS: u16 = 16;
const LABEL_W: u16 = 14;
const SWATCH_W: u16 = 5;
const HEX_W: u16 = 9;
const CONFIRM_W: u16 = 62;
const COLUMNS: [&[&str]; 2] = [&["General", "Cursor"], &["Files", "Messages", "Key bar"]];
pub const SHARED_NOTE: &str = "Shared with Orchard Commander: saved schemes show up there too";

/// The player's name for a color role where it differs from Commander's
/// (the file browser shows folders, playlists and marked files).
fn role_label(role: &str, commander: &'static str) -> &'static str {
    match role {
        "dir" => "Folders",
        "link" => "Links and playlists",
        _ => commander,
    }
}

/// Roles only Orchard Commander shows.
fn commander_only(role: &str) -> bool {
    matches!(role, "exec" | "hidden")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Name,
    Dark,
    /// A color's swatch (before its hex field in the Tab order).
    Swatch(usize),
    Hex(usize),
    Buttons,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorOutcome {
    Stay,
    Close,
}

pub struct SchemeEditor {
    slot: Slot,
    /// The user scheme being edited (it may be renamed); None: saved as a new one.
    pub original: Option<String>,
    /// The scheme it started from.
    based_on: String,
    create: bool,
    pub scheme: ColorScheme,
    pub name: TextField,
    hex: Vec<TextField>,
    buttons: ButtonRow,
    pub focus: Focus,
    /// The color picker for a role, while open.
    pub picker: Option<(usize, ColorPicker)>,
    /// "Delete …?" while asked.
    pub(crate) confirm: Option<ButtonRow>,
    pub error: Option<String>,
}

struct Layout {
    outer: CellRect,
    note: CellRect,
    name_label: CellRect,
    name: CellRect,
    dark: CellRect,
    headings: Vec<(CellRect, &'static str)>,
    /// (role index, label, swatch, hex field), in role order.
    rows: Vec<(usize, CellRect, CellRect, CellRect)>,
    status: CellRect,
    shared: CellRect,
    buttons: CellRect,
}

impl SchemeEditor {
    /// The editor for the scheme in `slot`'s setting; `create`: a new scheme
    /// starting from it. The player shows it at once.
    pub fn open(app: &mut App, slot: Slot, create: bool) -> SchemeEditor {
        let from = app.scheme(slot.value(&app.settings));
        let builtin = themes::is_builtin(&from.name);
        let (name, original) = if create {
            (unique_name(app, &format!("{} (copy)", from.name)), None)
        } else if builtin {
            (unique_name(app, &format!("{} (custom)", from.name)), None)
        } else {
            (from.name.clone(), Some(from.name.clone()))
        };
        let editor = SchemeEditor::new(slot, from, name, original, create);
        editor.preview(app);
        editor
    }

    fn new(slot: Slot, scheme: ColorScheme, name: String, original: Option<String>, create: bool) -> Self {
        let hex = COLOR_ROLES.iter().map(|(role, _, _)| TextField::new(scheme.color(role).unwrap_or_default())).collect();
        let labels: &[&str] = if original.is_some() { &[SAVE, DELETE, CANCEL] } else { &[SAVE, CANCEL] };
        SchemeEditor {
            slot,
            based_on: scheme.name.clone(),
            original,
            create,
            scheme,
            name: TextField::new(name),
            hex,
            buttons: ButtonRow::new(labels),
            focus: Focus::Name,
            picker: None,
            confirm: None,
            error: None,
        }
    }

    pub fn title(&self) -> &'static str {
        if self.create { "Create color scheme" } else { "Edit color scheme" }
    }

    /// The line at the top: what saving does.
    fn note(&self) -> String {
        if self.create {
            format!("A new color scheme, starting from '{}'", self.based_on)
        } else if self.original.is_none() {
            format!("'{}' is built in, so your changes are saved as a new color scheme", self.based_on)
        } else {
            "Changes show on the whole window as you make them · Esc discards them".to_owned()
        }
    }

    /// Tab stops (`swatches`: each color's swatch too, as Tab goes; ↑/↓ go
    /// from color to color).
    fn order(&self, swatches: bool) -> Vec<Focus> {
        let mut v = vec![Focus::Name, Focus::Dark];
        for i in 0..COLOR_ROLES.len() {
            if swatches {
                v.push(Focus::Swatch(i));
            }
            v.push(Focus::Hex(i));
        }
        v.push(Focus::Buttons);
        v
    }

    fn move_focus(&mut self, delta: i32, swatches: bool) {
        let order = self.order(swatches);
        let at = match self.focus {
            Focus::Swatch(i) if !swatches => Focus::Hex(i),
            f => f,
        };
        let i = order.iter().position(|f| *f == at).unwrap_or(0) as i32;
        self.focus = order[(i + delta).rem_euclid(order.len() as i32) as usize];
    }

    /// The picker for role `i`, titled with the role's name.
    pub fn open_picker(&mut self, i: usize) {
        let (role, label, _) = COLOR_ROLES[i];
        let color = self.scheme.color(role).and_then(Rgba::parse).unwrap_or(Rgba::BLACK);
        self.picker = Some((i, ColorPicker::new(role_label(role, label), color)));
    }

    fn set_color(&mut self, app: &mut App, i: usize, hex: String) {
        if let Some(c) = self.scheme.color_mut(COLOR_ROLES[i].0) {
            *c = hex;
        }
        self.preview(app);
    }

    /// Show the edited colors on the whole player.
    fn preview(&self, app: &mut App) {
        app.preview_scheme(self.scheme.clone());
    }

    // -- layout ------------------------------------------------------------

    fn rects(&self, screen: CellRect) -> Layout {
        let (outer, content) = form::dialog_rects(screen, WIDTH, 4 + COLUMN_ROWS + 4);
        let x = content.col;
        let note = CellRect::new(x, content.row, content.cols, 1);
        let y = content.row + 1;
        let name_label = CellRect::new(x, y, LABEL_W, 1);
        let name = CellRect::new(x + LABEL_W, y, content.cols.saturating_sub(LABEL_W).min(40), 1);
        let dark = CellRect::new(x + LABEL_W, y + 1, 20, 1);
        let buttons = CellRect::new(x, bottom(content).saturating_sub(1), content.cols, 1);
        let shared = CellRect::new(x, buttons.row.saturating_sub(2).max(y + 2), content.cols, 1);
        let status = CellRect::new(x, shared.row.saturating_sub(1).max(y + 2), content.cols, 1);
        let top = y + 3;
        let col_w = content.cols.saturating_sub(4) / 2;
        let mut rows = Vec::new();
        let mut headings = Vec::new();
        for (col, groups) in COLUMNS.iter().enumerate() {
            let cx = x + col as u16 * (col_w + 4);
            let mut cy = top;
            for (gi, group) in groups.iter().enumerate() {
                if gi > 0 {
                    cy += 1;
                }
                // In a short window the rows that don't fit are left out
                // (Tab still reaches them).
                if cy < status.row {
                    headings.push((CellRect::new(cx, cy, col_w, 1), *group));
                }
                cy += 1;
                for (i, _) in COLOR_ROLES.iter().enumerate().filter(|(_, (_, _, g))| g == group) {
                    if cy < status.row {
                        let hex = CellRect::new((cx + col_w).saturating_sub(HEX_W), cy, HEX_W, 1);
                        let swatch = CellRect::new(hex.col.saturating_sub(1 + SWATCH_W), cy, SWATCH_W, 1);
                        rows.push((i, CellRect::new(cx, cy, swatch.col.saturating_sub(cx + 1), 1), swatch, hex));
                    }
                    cy += 1;
                }
            }
        }
        rows.sort_by_key(|r| r.0);
        Layout { outer, note, name_label, name, dark, headings, rows, status, shared, buttons }
    }

    fn confirm_rects(&self, screen: CellRect) -> (CellRect, CellRect, Vec<String>, CellRect) {
        let lines = form::wrap(&self.delete_question(), CONFIRM_W as usize - 4);
        let (outer, content) = form::dialog_rects(screen, CONFIRM_W, lines.len() as u16 + 2);
        let buttons = CellRect::new(content.col, bottom(content).saturating_sub(1), content.cols, 1);
        (outer, content, lines, buttons)
    }

    fn delete_question(&self) -> String {
        format!(
            "Delete '{}'? Its file is deleted and you can't undo this. Orchard Commander uses the same color schemes, so it's gone there too. Where it's in use, the default color scheme takes over.",
            self.original.as_deref().unwrap_or_default()
        )
    }

    // -- results -------------------------------------------------------------

    fn cancel(&self, app: &mut App) -> EditorOutcome {
        app.apply_settings();
        EditorOutcome::Close
    }

    /// Save into the shared folder; the slot then uses the saved scheme.
    pub fn save(&mut self, app: &mut App) -> EditorOutcome {
        let mut scheme = self.scheme.clone();
        scheme.name = self.name.text.trim().to_owned();
        match themes::save_user_scheme(&app.schemes_dir, &scheme, self.original.as_deref()) {
            Ok(_) => {
                app.refresh_schemes();
                let (slot, name, old) = (self.slot, scheme.name, self.original.clone());
                app.update_settings(|s| {
                    // A renamed scheme stays chosen wherever it was.
                    for other in Slot::ALL {
                        if old.as_deref().is_some_and(|o| o == other.value(s)) {
                            other.set(s, &name);
                        }
                    }
                    slot.set(s, &name);
                });
                EditorOutcome::Close
            }
            Err(oc_config::ConfigError::Invalid(msg)) => {
                self.error = Some(msg);
                EditorOutcome::Stay
            }
            Err(oc_config::ConfigError::Io(e)) => {
                self.error = Some(format!(
                    "Can't save the color scheme: {} — check that you can write to {}",
                    crate::meta::io_reason(&e),
                    app.schemes_dir.display()
                ));
                EditorOutcome::Stay
            }
        }
    }

    /// Delete the user scheme being edited (after the question).
    pub fn delete(&mut self, app: &mut App) -> EditorOutcome {
        let Some(name) = self.original.clone() else { return EditorOutcome::Stay };
        match themes::delete_user_scheme(&app.schemes_dir, &name) {
            Ok(()) => {
                app.refresh_schemes();
                let defaults = Settings::default();
                // Whatever used it goes back to the default.
                app.update_settings(|s| {
                    for slot in Slot::ALL {
                        if slot.value(s) == name {
                            slot.set(s, slot.value(&defaults));
                        }
                    }
                });
                EditorOutcome::Close
            }
            Err(e) => {
                self.error = Some(format!(
                    "Can't delete the color scheme: {} — check that you can change files in {}",
                    crate::meta::io_reason(&e),
                    app.schemes_dir.display()
                ));
                EditorOutcome::Stay
            }
        }
    }

    fn press(&mut self, app: &mut App, i: usize) -> EditorOutcome {
        match self.buttons.labels[i].as_str() {
            SAVE => self.save(app),
            DELETE => {
                let mut row = ButtonRow::new(&[DELETE, CANCEL]);
                row.focus = 1;
                self.confirm = Some(row);
                EditorOutcome::Stay
            }
            _ => self.cancel(app),
        }
    }

    /// A hex field was edited: take the color once it is one.
    fn typed(&mut self, app: &mut App, i: usize) {
        self.error = None;
        if let Some(hex) = color_picker::clamp_hex(&mut self.hex[i]) {
            self.set_color(app, i, hex);
        }
    }

    // -- events --------------------------------------------------------------

    pub fn handle_key(&mut self, app: &mut App, k: &KeyEvent, screen: CellRect) -> EditorOutcome {
        if let Some((i, picker)) = &mut self.picker {
            let i = *i;
            match picker.handle_key(k, screen) {
                PickerOutcome::Consumed => {}
                PickerOutcome::Cancelled => self.picker = None,
                PickerOutcome::Chosen(c) => self.chosen(app, i, c),
            }
            return EditorOutcome::Stay;
        }
        if let Some(row) = &mut self.confirm {
            match (&k.key, row.handle_key(k)) {
                (Key::Named(Named::Escape), _) | (_, RowOutcome::Pressed(1)) => self.confirm = None,
                (Key::Named(Named::Tab), _) => row.focus = 1 - row.focus,
                (_, RowOutcome::Pressed(_)) => {
                    self.confirm = None;
                    return self.delete(app);
                }
                _ => {}
            }
            return EditorOutcome::Stay;
        }
        let plain = k.mods.none();
        match &k.key {
            Key::Named(Named::Escape) => return self.cancel(app),
            Key::Named(Named::Tab) => {
                self.move_focus(if k.mods.shift { -1 } else { 1 }, true);
                return EditorOutcome::Stay;
            }
            Key::Named(Named::Up | Named::Down) => {
                self.move_focus(if k.is(Named::Up) { -1 } else { 1 }, false);
                return EditorOutcome::Stay;
            }
            _ => {}
        }
        match self.focus {
            Focus::Buttons => {
                if let RowOutcome::Pressed(i) = self.buttons.handle_key(k) {
                    return self.press(app, i);
                }
            }
            Focus::Dark => match &k.key {
                Key::Named(Named::Enter) => return self.save(app),
                Key::Char(' ') if plain => {
                    self.scheme.dark = !self.scheme.dark;
                    self.preview(app);
                }
                _ => {}
            },
            Focus::Name => {
                if k.is(Named::Enter) {
                    return self.save(app);
                }
                if self.name.handle(k) {
                    self.error = None;
                }
            }
            Focus::Swatch(i) => match &k.key {
                Key::Char(' ') if plain => self.open_picker(i),
                Key::Named(Named::Right) => self.focus = Focus::Hex(i),
                Key::Named(Named::Enter) => return self.save(app),
                _ => {}
            },
            Focus::Hex(i) => {
                if k.is(Named::Enter) {
                    return self.save(app);
                }
                if k.is(Named::Left) && self.hex[i].cursor == 0 {
                    self.focus = Focus::Swatch(i);
                } else if self.hex[i].handle(k) {
                    self.typed(app, i);
                }
            }
        }
        EditorOutcome::Stay
    }

    fn chosen(&mut self, app: &mut App, i: usize, c: Rgba) {
        self.picker = None;
        let hex = color_picker::to_hex(c);
        self.hex[i].set_text(&hex);
        self.set_color(app, i, hex);
    }

    pub fn handle_mouse(&mut self, app: &mut App, m: &MouseEvent, screen: CellRect) -> EditorOutcome {
        if let Some((i, picker)) = &mut self.picker {
            let i = *i;
            match picker.handle_mouse(m, screen) {
                PickerOutcome::Consumed => {}
                PickerOutcome::Cancelled => self.picker = None,
                PickerOutcome::Chosen(c) => self.chosen(app, i, c),
            }
            return EditorOutcome::Stay;
        }
        if m.kind != MouseKind::Down {
            return EditorOutcome::Stay;
        }
        let (col, row) = (m.col, m.row);
        if let Some(confirm) = &self.confirm {
            let (_, _, _, buttons) = self.confirm_rects(screen);
            match confirm.hit(buttons, col, row) {
                Some(0) => {
                    self.confirm = None;
                    return self.delete(app);
                }
                Some(_) => self.confirm = None,
                None => {}
            }
            return EditorOutcome::Stay;
        }
        let l = self.rects(screen);
        if contains(l.name, col, row) {
            self.focus = Focus::Name;
            self.name.click(l.name, col);
        } else if contains(l.dark, col, row) {
            self.focus = Focus::Dark;
            self.scheme.dark = !self.scheme.dark;
            self.preview(app);
        } else if let Some((i, _, _, _)) = l.rows.iter().find(|(_, _, swatch, _)| contains(*swatch, col, row)) {
            self.focus = Focus::Swatch(*i);
            self.open_picker(*i);
        } else if let Some((i, _, _, hex)) = l.rows.iter().find(|(_, _, _, hex)| contains(*hex, col, row)) {
            self.focus = Focus::Hex(*i);
            self.hex[*i].click(*hex, col);
        } else if let Some(i) = self.buttons.hit(l.buttons, col, row) {
            self.focus = Focus::Buttons;
            self.buttons.focus = i;
            return self.press(app, i);
        }
        EditorOutcome::Stay
    }

    // -- drawing -------------------------------------------------------------

    /// What the status line says while nothing went wrong.
    fn hint(&self) -> String {
        match self.focus {
            Focus::Swatch(i) | Focus::Hex(i) if commander_only(COLOR_ROLES[i].0) => "Used by Orchard Commander only".into(),
            Focus::Swatch(_) => "Space or a click opens the color picker".into(),
            Focus::Hex(_) => "A color as #rrggbb · the swatch opens the color picker".into(),
            Focus::Dark => "Dark schemes fill in missing colors from a dark built-in scheme".into(),
            Focus::Name | Focus::Buttons => String::new(),
        }
    }

    pub fn draw(&self, _app: &App, f: &mut Frame, screen: CellRect, ctx: &DrawCtx) {
        let c = &ctx.theme.colors;
        let l = self.rects(screen);
        let active = self.picker.is_none() && self.confirm.is_none();
        form::dialog_box(f, l.outer, self.title(), ctx);
        form::muted(f, l.note, &self.note(), ctx);
        form::label(f, l.name_label, "Name", ctx);
        self.name.draw_field(f, l.name, ctx, active && self.focus == Focus::Name);
        form::draw_check(f, l.dark, "Dark scheme", self.scheme.dark, active && self.focus == Focus::Dark, ctx);
        for (r, text) in &l.headings {
            form::heading(f, *r, text, ctx);
        }
        for (i, label, swatch, hex) in &l.rows {
            let (role, text, _) = COLOR_ROLES[*i];
            let text = role_label(role, text);
            if commander_only(role) { form::muted(f, *label, text, ctx) } else { form::label(f, *label, text, ctx) }
            let color = self.scheme.color(role).and_then(Rgba::parse).unwrap_or(c.background);
            fill(f, *swatch, color);
            // A thin outline, so a swatch in the surface color still shows.
            if self.focus == Focus::Swatch(*i) && active {
                put(f, swatch.col, swatch.row, "▕", Style::new(c.border_active, color), 1);
                put(f, right(*swatch) - 1, swatch.row, "▏", Style::new(c.border_active, color), 1);
            } else {
                put(f, swatch.col, swatch.row, "▏", Style::new(c.muted, color), 1);
            }
            self.hex[*i].draw_field(f, *hex, ctx, active && self.focus == Focus::Hex(*i));
        }
        if let Some(e) = &self.error {
            put(f, l.status.col, l.status.row, e, Style::new(c.error, c.surface), l.status.cols);
        } else {
            form::muted(f, l.status, &self.hint(), ctx);
        }
        form::muted(f, l.shared, SHARED_NOTE, ctx);
        self.buttons.draw(f, l.buttons, ctx, active && self.focus == Focus::Buttons);
        if let Some((_, p)) = &self.picker {
            p.draw(f, screen, ctx);
        }
        if let Some(row) = &self.confirm {
            let (outer, content, lines, buttons) = self.confirm_rects(screen);
            form::dialog_box(f, outer, "Delete color scheme", ctx);
            for (k, line) in lines.iter().enumerate() {
                form::label(f, CellRect::new(content.col, content.row + k as u16, content.cols, 1), line, ctx);
            }
            row.draw(f, buttons, ctx, true);
        }
    }
}

/// `base`, or "base 2", "base 3"… if a scheme has that name.
fn unique_name(app: &App, base: &str) -> String {
    let taken = |n: &str| app.schemes.iter().any(|s| s.name == n);
    if !taken(base) {
        return base.to_owned();
    }
    (2..).map(|k| format!("{base} {k}")).find(|n| !taken(n)).unwrap_or_default()
}
