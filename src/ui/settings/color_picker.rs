//! A color picker for the scheme editor's swatches (port of Orchard
//! Commander's): 48 basic colors, the hue/saturation field with the value
//! bar beside it, the numbers, a hex field, "Use color" and Cancel. The
//! scheme editor owns it while it is open and takes the color on "Use color".

use orchard_grid::{CellRect, Frame, Rgba, Style};
use orchard_kit::input::{Key, KeyEvent, MouseEvent, MouseKind, Named};
use orchard_kit::widgets::{DrawCtx, TextField, bottom, contains, fill, is_empty, put, right};

use super::form::{self, ButtonRow, RowOutcome};

const WIDTH: u16 = 74;
const ROWS: u16 = 13;
/// Basic colors: 6 rows × 8 columns of swatches this wide, a column apart.
const WELL_ROWS: u16 = 6;
const WELL_COLS: u16 = 8;
const WELL_W: u16 = 4;
/// The hue/saturation field (cells; each holds two rows of colors) and the value bar.
const FIELD_X: u16 = 42;
const FIELD_W: u16 = 24;
const BAR_X: u16 = FIELD_X + FIELD_W + 1;
const BAR_W: u16 = 2;
/// The field is drawn at this value.
const FIELD_VALUE: i32 = 200;
/// "#rrggbb".
pub const HEX_MAX: usize = 7;

/// Qt's standard colors (`qcolordialog.cpp`, `initRgb`), shown column by column.
fn basic_color(i: usize) -> Rgba {
    let (g, r, b) = (i / 12, i / 3 % 4, i % 3);
    Rgba::rgb((r * 255 / 3) as u8, (g * 255 / 3) as u8, (b * 255 / 2) as u8)
}

/// HSV (hue 0–359, saturation and value 0–255) → RGB.
pub fn hsv_to_rgb(h: i32, s: i32, v: i32) -> Rgba {
    let (h, s, v) = (h.rem_euclid(360) as f64 / 60.0, s.clamp(0, 255) as f64 / 255.0, v.clamp(0, 255) as f64 / 255.0);
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let byte = |f: f64| ((f + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Rgba::rgb(byte(r), byte(g), byte(b))
}

/// RGB → (hue or None for grays, saturation, value).
pub fn rgb_to_hsv(c: Rgba) -> (Option<i32>, i32, i32) {
    let (r, g, b) = (c.r as f64, c.g as f64, c.b as f64);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let s = if max == 0.0 { 0.0 } else { d / max * 255.0 };
    if d == 0.0 {
        return (None, s.round() as i32, max as i32);
    }
    let h = if max == r {
        60.0 * ((g - b) / d)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (Some((h.round() as i32).rem_euclid(360)), s.round() as i32, max as i32)
}

pub fn to_hex(c: Rgba) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
}

/// Black or white, whichever shows on `bg`.
pub fn contrast(bg: Rgba) -> Rgba {
    let luma = bg.r as u32 * 299 + bg.g as u32 * 587 + bg.b as u32 * 114;
    if luma > 128_000 { Rgba::BLACK } else { Rgba::WHITE }
}

/// Cut a hex field back to [`HEX_MAX`] characters (what went past it, at
/// the cursor) and return its color once it is one.
pub fn clamp_hex(field: &mut TextField) -> Option<String> {
    while field.text.chars().count() > HEX_MAX && field.cursor > 0 {
        let at = field.text.char_indices().nth(field.cursor - 1).map_or(0, |(b, _)| b);
        field.text.remove(at);
        field.cursor -= 1;
    }
    let text = field.text.trim().to_lowercase();
    orchard_theme::schemes::is_color(&text).then_some(text)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    Basic,
    Field,
    Value,
    Hex,
    Buttons,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drag {
    Field,
    Value,
}

/// What an event did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerOutcome {
    Consumed,
    /// "Use color": this one.
    Chosen(Rgba),
    Cancelled,
}

pub struct ColorPicker {
    title: String,
    initial: Rgba,
    color: Rgba,
    hue: i32,
    sat: i32,
    val: i32,
    /// The basic color under the keyboard cursor.
    well: usize,
    hex: TextField,
    buttons: ButtonRow,
    focus: Focus,
    drag: Option<Drag>,
}

struct Layout {
    outer: CellRect,
    heading: CellRect,
    wells: CellRect,
    field: CellRect,
    bar: CellRect,
    swatch: CellRect,
    swatch_label: CellRect,
    numbers: CellRect,
    hex_label: CellRect,
    hex: CellRect,
    buttons: CellRect,
}

impl ColorPicker {
    /// A picker titled `title` (the color's role), starting at `color`.
    pub fn new(title: &str, color: Rgba) -> Self {
        let mut p = ColorPicker {
            title: title.to_owned(),
            initial: color,
            color,
            hue: 0,
            sat: 0,
            val: 0,
            well: (0..48).find(|&i| basic_color(i) == color).unwrap_or(0),
            hex: TextField::default().flush(),
            buttons: ButtonRow::new(&["Use color", "Cancel"]),
            focus: Focus::Field,
            drag: None,
        };
        p.set_rgb(color);
        p
    }

    /// The color as picked so far.
    pub fn color(&self) -> Rgba {
        self.color
    }

    fn set_rgb(&mut self, c: Rgba) {
        let (h, s, v) = rgb_to_hsv(c);
        if let Some(h) = h {
            self.hue = h; // grays keep the hue they had
        }
        (self.sat, self.val) = (s, v);
        self.color = c.with_alpha(255);
        self.hex.set_text(&to_hex(self.color));
    }

    fn set_hsv(&mut self, h: i32, s: i32, v: i32) {
        (self.hue, self.sat, self.val) = (h.clamp(0, 359), s.clamp(0, 255), v.clamp(0, 255));
        self.color = hsv_to_rgb(self.hue, self.sat, self.val);
        self.hex.set_text(&to_hex(self.color));
    }

    fn rects(&self, screen: CellRect) -> Layout {
        let (outer, c) = form::dialog_rects(screen, WIDTH, ROWS);
        let at = |x: u16, y: u16, w: u16, h: u16| {
            let x = (c.col + x).min(right(c));
            let y = (c.row + y).min(bottom(c));
            CellRect::new(x, y, w.min(right(c).saturating_sub(x)), h.min(bottom(c).saturating_sub(y)))
        };
        Layout {
            outer,
            heading: at(0, 0, FIELD_X - 2, 1),
            wells: at(0, 1, WELL_COLS * (WELL_W + 1) - 1, WELL_ROWS),
            field: at(FIELD_X, 1, FIELD_W, WELL_ROWS),
            bar: at(BAR_X, 1, BAR_W, WELL_ROWS),
            swatch: at(0, 8, 8, 2),
            swatch_label: at(9, 8, 3, 2),
            numbers: at(14, 8, 44, 2),
            hex_label: at(14, 10, 5, 1),
            hex: at(20, 10, 10, 1),
            buttons: at(0, ROWS - 1, c.cols, 1),
        }
    }

    /// The basic color well at a cell.
    fn well_at(l: &Layout, col: u16, row: u16) -> Option<usize> {
        if !contains(l.wells, col, row) {
            return None;
        }
        let (dx, dy) = (col - l.wells.col, row - l.wells.row);
        (dx % (WELL_W + 1) < WELL_W).then(|| (dx / (WELL_W + 1) * WELL_ROWS + dy) as usize)
    }

    /// Half-rows of the field and the bar (each cell shows two).
    fn halves(r: CellRect) -> i32 {
        (r.rows as i32 * 2).max(2)
    }

    fn field_hue(r: CellRect, x: i32) -> i32 {
        360 - x.clamp(0, r.cols as i32 - 1) * 360 / (r.cols as i32 - 1).max(1)
    }

    fn half_level(r: CellRect, y: i32) -> i32 {
        255 - y.clamp(0, Self::halves(r) - 1) * 255 / (Self::halves(r) - 1)
    }

    /// The field's cell (column, half-row) for the current hue and saturation.
    fn field_pos(&self, r: CellRect) -> (i32, i32) {
        ((360 - self.hue) * (r.cols as i32 - 1).max(1) / 360, (255 - self.sat) * (Self::halves(r) - 1) / 255)
    }

    fn value_pos(&self, r: CellRect) -> i32 {
        (255 - self.val) * (Self::halves(r) - 1) / 255
    }

    fn pick_field(&mut self, r: CellRect, x: i32, half: i32) {
        self.set_hsv(Self::field_hue(r, x).min(359), Self::half_level(r, half), self.val);
    }

    fn pick_value(&mut self, r: CellRect, half: i32) {
        self.set_hsv(self.hue, self.sat, Self::half_level(r, half));
    }

    pub fn draw(&self, f: &mut Frame, screen: CellRect, ctx: &DrawCtx) {
        let c = &ctx.theme.colors;
        let l = self.rects(screen);
        form::dialog_box(f, l.outer, &self.title, ctx);
        form::heading(f, l.heading, "Basic colors", ctx);
        for i in 0..(WELL_ROWS * WELL_COLS) as usize {
            let (col, row) = (i as u16 / WELL_ROWS, i as u16 % WELL_ROWS);
            let r = CellRect::new(l.wells.col + col * (WELL_W + 1), l.wells.row + row, WELL_W, 1);
            if right(r) > right(l.wells) || r.row >= bottom(l.wells) {
                continue;
            }
            let color = basic_color(i);
            fill(f, r, color);
            if i == self.well && (self.focus == Focus::Basic || color == self.color) {
                let mark = if self.focus == Focus::Basic { c.border_active } else { contrast(color) };
                put(f, r.col, r.row, "▕", Style::new(mark, color), 1);
                put(f, right(r) - 1, r.row, "▏", Style::new(mark, color), 1);
            }
        }
        // Hue across, saturation down (two levels per cell: ▀ in the upper color on the lower).
        for x in 0..l.field.cols {
            let hue = Self::field_hue(l.field, x as i32).min(359);
            for y in 0..l.field.rows {
                let top = hsv_to_rgb(hue, Self::half_level(l.field, y as i32 * 2), FIELD_VALUE);
                let low = hsv_to_rgb(hue, Self::half_level(l.field, y as i32 * 2 + 1), FIELD_VALUE);
                put(f, l.field.col + x, l.field.row + y, "▀", Style::new(top, low), 1);
            }
        }
        if !is_empty(l.field) {
            let (fx, fy) = self.field_pos(l.field);
            let (x, y) = (l.field.col + fx as u16, l.field.row + (fy / 2) as u16);
            let under = hsv_to_rgb(self.hue, self.sat, FIELD_VALUE);
            let mark = if self.focus == Focus::Field { c.border_active } else { contrast(under) };
            put(f, x, y, "┼", Style::new(mark, under), 1);
        }
        for y in 0..l.bar.rows {
            let top = hsv_to_rgb(self.hue, self.sat, Self::half_level(l.bar, y as i32 * 2));
            let low = hsv_to_rgb(self.hue, self.sat, Self::half_level(l.bar, y as i32 * 2 + 1));
            for x in 0..l.bar.cols {
                put(f, l.bar.col + x, l.bar.row + y, "▀", Style::new(top, low), 1);
            }
        }
        if !is_empty(l.bar) {
            let y = l.bar.row + (self.value_pos(l.bar) / 2) as u16;
            let mark = if self.focus == Focus::Value { c.border_active } else { c.foreground };
            put(f, right(l.bar), y, "◀", Style::new(mark, c.surface), 1);
        }
        // The new color over the one it started from.
        fill(f, CellRect::new(l.swatch.col, l.swatch.row, l.swatch.cols, l.swatch.rows.min(1)), self.color);
        if l.swatch.rows > 1 {
            fill(f, CellRect::new(l.swatch.col, l.swatch.row + 1, l.swatch.cols, 1), self.initial);
        }
        for (i, text) in ["New", "Old"].iter().enumerate().take(l.swatch_label.rows as usize) {
            form::muted(f, CellRect::new(l.swatch_label.col, l.swatch_label.row + i as u16, l.swatch_label.cols, 1), text, ctx);
        }
        let numbers = [
            format!("Hue  {:>3}   Saturation {:>3}   Value {:>3}", self.hue, self.sat, self.val),
            format!("Red  {:>3}   Green      {:>3}   Blue  {:>3}", self.color.r, self.color.g, self.color.b),
        ];
        for (i, line) in numbers.iter().enumerate().take(l.numbers.rows as usize) {
            form::label(f, CellRect::new(l.numbers.col, l.numbers.row + i as u16, l.numbers.cols, 1), line, ctx);
        }
        form::label(f, l.hex_label, "Hex", ctx);
        self.hex.draw_field(f, l.hex, ctx, self.focus == Focus::Hex);
        self.buttons.draw(f, l.buttons, ctx, self.focus == Focus::Buttons);
    }

    fn move_focus(&mut self, delta: i32) {
        const ORDER: [Focus; 5] = [Focus::Field, Focus::Value, Focus::Basic, Focus::Hex, Focus::Buttons];
        let i = ORDER.iter().position(|f| *f == self.focus).unwrap_or(0) as i32;
        self.focus = ORDER[(i + delta).rem_euclid(ORDER.len() as i32) as usize];
    }

    pub fn handle_key(&mut self, k: &KeyEvent, screen: CellRect) -> PickerOutcome {
        let l = self.rects(screen);
        let plain = k.mods.none();
        match &k.key {
            Key::Named(Named::Escape) => return PickerOutcome::Cancelled,
            Key::Named(Named::Tab) => {
                self.move_focus(if k.mods.shift { -1 } else { 1 });
                return PickerOutcome::Consumed;
            }
            Key::Named(Named::Enter) if self.focus != Focus::Buttons => return PickerOutcome::Chosen(self.color),
            _ => {}
        }
        let step = |key: &Key| match key {
            Key::Named(Named::Left) => (-1, 0),
            Key::Named(Named::Right) => (1, 0),
            Key::Named(Named::Up) => (0, -1),
            Key::Named(Named::Down) => (0, 1),
            _ => (0, 0),
        };
        match self.focus {
            Focus::Basic if plain || k.mods.shift => {
                let (dx, dy) = step(&k.key);
                let (col, row) = ((self.well as i32) / WELL_ROWS as i32 + dx, (self.well as i32) % WELL_ROWS as i32 + dy);
                if (dx, dy) != (0, 0) && (0..WELL_COLS as i32).contains(&col) && (0..WELL_ROWS as i32).contains(&row) {
                    self.well = (col * WELL_ROWS as i32 + row) as usize;
                }
                if (dx, dy) != (0, 0) || k.key == Key::Char(' ') {
                    self.set_rgb(basic_color(self.well));
                }
            }
            Focus::Field => {
                let (dx, dy) = step(&k.key);
                if (dx, dy) != (0, 0) {
                    let n = if k.mods.shift { 4 } else { 1 };
                    let (x, y) = self.field_pos(l.field);
                    self.pick_field(l.field, x + dx * n, y + dy * n);
                }
            }
            Focus::Value => {
                let dy = match &k.key {
                    Key::Named(Named::PageUp) => -4,
                    Key::Named(Named::PageDown) => 4,
                    Key::Named(Named::Left) => 1,
                    Key::Named(Named::Right) => -1,
                    other => step(other).1,
                };
                if dy != 0 {
                    let y = self.value_pos(l.bar);
                    self.pick_value(l.bar, y + dy);
                }
            }
            Focus::Hex => {
                if self.hex.handle(k) {
                    self.typed();
                }
            }
            Focus::Buttons => {
                if let RowOutcome::Pressed(i) = self.buttons.handle_key(k) {
                    return self.press(i);
                }
            }
            _ => {}
        }
        PickerOutcome::Consumed
    }

    pub fn handle_mouse(&mut self, m: &MouseEvent, screen: CellRect) -> PickerOutcome {
        let l = self.rects(screen);
        let (col, row) = (m.col, m.row);
        match m.kind {
            MouseKind::Down => {
                if let Some(i) = Self::well_at(&l, col, row) {
                    self.focus = Focus::Basic;
                    self.well = i;
                    self.set_rgb(basic_color(i));
                    if m.double {
                        return PickerOutcome::Chosen(self.color);
                    }
                } else if contains(l.field, col, row) {
                    self.focus = Focus::Field;
                    self.drag = Some(Drag::Field);
                    self.pick_field(l.field, (col - l.field.col) as i32, (row - l.field.row) as i32 * 2);
                } else if contains(l.bar, col, row) {
                    self.focus = Focus::Value;
                    self.drag = Some(Drag::Value);
                    self.pick_value(l.bar, (row - l.bar.row) as i32 * 2);
                } else if contains(l.hex, col, row) {
                    self.focus = Focus::Hex;
                    self.hex.click(l.hex, col);
                } else if let Some(i) = self.buttons.hit(l.buttons, col, row) {
                    self.focus = Focus::Buttons;
                    self.buttons.focus = i;
                    return self.press(i);
                }
            }
            MouseKind::Drag => match self.drag {
                Some(Drag::Field) => {
                    let (x, y) = (col as i32 - l.field.col as i32, row as i32 - l.field.row as i32);
                    self.pick_field(l.field, x, y * 2);
                }
                Some(Drag::Value) => self.pick_value(l.bar, (row as i32 - l.bar.row as i32) * 2),
                None => {}
            },
            MouseKind::Up => self.drag = None,
            MouseKind::Scroll(lines) if contains(l.bar, col, row) => {
                let y = self.value_pos(l.bar);
                self.pick_value(l.bar, y - lines.signum() as i32);
            }
            _ => {}
        }
        PickerOutcome::Consumed
    }

    fn press(&self, i: usize) -> PickerOutcome {
        if i == 0 { PickerOutcome::Chosen(self.color) } else { PickerOutcome::Cancelled }
    }

    /// The hex field changed: the color once it is one.
    fn typed(&mut self) {
        if let Some(c) = clamp_hex(&mut self.hex).and_then(|t| Rgba::parse(&t)) {
            let (h, s, v) = rgb_to_hsv(c);
            if let Some(h) = h {
                self.hue = h;
            }
            (self.sat, self.val, self.color) = (s, v, c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_colors_are_qts() {
        assert_eq!(basic_color(0), Rgba::rgb(0, 0, 0));
        assert_eq!(basic_color(2), Rgba::rgb(0, 0, 255));
        assert_eq!(basic_color(3), Rgba::rgb(85, 0, 0));
        assert_eq!(basic_color(47), Rgba::rgb(255, 255, 255));
    }

    #[test]
    fn hsv_round_trips() {
        for c in [Rgba::rgb(255, 0, 0), Rgba::rgb(18, 52, 86), Rgba::rgb(171, 205, 239), Rgba::rgb(0, 255, 128)] {
            let (h, s, v) = rgb_to_hsv(c);
            let back = hsv_to_rgb(h.unwrap(), s, v);
            assert!((back.r as i32 - c.r as i32).abs() <= 2, "{c:?} {back:?}");
            assert!((back.g as i32 - c.g as i32).abs() <= 2, "{c:?} {back:?}");
            assert!((back.b as i32 - c.b as i32).abs() <= 2, "{c:?} {back:?}");
        }
        assert_eq!(rgb_to_hsv(Rgba::rgb(128, 128, 128)), (None, 0, 128));
    }

    #[test]
    fn keys_pick_and_hex_field_takes_colors() {
        let screen = CellRect::new(0, 0, 112, 36);
        let mut p = ColorPicker::new("Text", Rgba::rgb(255, 0, 0));
        // Value bar: Down darkens.
        p.handle_key(&KeyEvent::named(Named::Tab), screen);
        p.handle_key(&KeyEvent::named(Named::Down), screen);
        assert!(p.color().r < 255);
        // Hex field: typing a full color takes it; extra characters are cut.
        p.handle_key(&KeyEvent::named(Named::Tab), screen);
        p.handle_key(&KeyEvent::named(Named::Tab), screen);
        for _ in 0..7 {
            p.handle_key(&KeyEvent::named(Named::Backspace), screen);
        }
        for ch in "#12345678".chars() {
            p.handle_key(&KeyEvent::char(ch), screen);
        }
        assert_eq!(p.hex.text, "#123456");
        assert_eq!(p.color(), Rgba::rgb(0x12, 0x34, 0x56));
        assert_eq!(p.handle_key(&KeyEvent::named(Named::Enter), screen), PickerOutcome::Chosen(Rgba::rgb(0x12, 0x34, 0x56)));
        assert_eq!(p.handle_key(&KeyEvent::named(Named::Escape), screen), PickerOutcome::Cancelled);
    }
}
