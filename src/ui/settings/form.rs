//! Form controls for the settings dialog, the scheme editor and the color
//! picker, in Orchard Commander's look (ported from `oc_ui`'s settings form
//! and widgets): a combo box (a field plus a popup list), a spin box, a
//! checkbox, push buttons, headings, and the dialog box they sit in. Their
//! values always come from the caller, so the state here is only what is
//! open and where its cursor is.

use orchard_grid::{CellRect, Flags, Frame, Layer, PxRect, Style};
use orchard_kit::input::{Key, KeyEvent, MouseEvent, MouseKind, Named};
use orchard_kit::widgets::{self, BoxFrame, DrawCtx, Scrollbar, bottom, contains, fill, inset, put, put_elided, right, width};

/// One entry of a combo box; `None` is a separator (between the built-in
/// and the user's color schemes).
pub type Choice = Option<String>;

/// At most this many items show at once.
pub const MAX_VISIBLE: u16 = 16;

/// A dialog of `width` × `content_rows` (plus its frame) centered in
/// `screen`, never larger: (outer, content). The content is two cells in
/// from the sides and one from the top and bottom.
pub fn dialog_rects(screen: CellRect, width: u16, content_rows: u16) -> (CellRect, CellRect) {
    let w = width.min(screen.cols.saturating_sub(4)).max(10.min(screen.cols));
    let h = (content_rows + 2).min(screen.rows.saturating_sub(2)).max(4.min(screen.rows));
    let outer = CellRect::new(screen.col + (screen.cols - w) / 2, screen.row + (screen.rows - h) / 2, w, h);
    (outer, CellRect::new(outer.col + 2, outer.row + 1, outer.cols.saturating_sub(4), outer.rows.saturating_sub(2)))
}

/// Draw a dialog's box (over whatever is under it, with a soft shadow) and
/// return its content area.
pub fn dialog_box(f: &mut Frame, outer: CellRect, title: &str, ctx: &DrawCtx) -> CellRect {
    cover(f, outer, ctx);
    BoxFrame { title, subtitle: "", active: true }.draw(f, outer, ctx);
    CellRect::new(outer.col + 2, outer.row + 1, outer.cols.saturating_sub(4), outer.rows.saturating_sub(2))
}

/// Clear the pixel overlays under `r` and shade its right and bottom edges.
pub fn cover(f: &mut Frame, r: CellRect, ctx: &DrawCtx) {
    widgets::punch(f, r, ctx);
    let (cw, ch) = ctx.cell_px;
    let b = widgets::px(r, ctx);
    let shade = ctx.theme.colors.background.with_alpha(170);
    f.add_rect(PxRect::new(b.right(), b.y + ch / 2.0, cw / 2.0, b.h), shade, Layer::AboveText);
    f.add_rect(PxRect::new(b.x + cw / 2.0, b.bottom(), b.w - cw / 2.0, ch / 2.0), shade, Layer::AboveText);
}

/// A heading: accent, bold.
pub fn heading(f: &mut Frame, r: CellRect, text: &str, ctx: &DrawCtx) {
    let c = &ctx.theme.colors;
    put_elided(f, r.col, r.row, text, Style::new(c.accent, c.surface).with(Flags::BOLD), r.cols);
}

/// A label in the form's text color.
pub fn label(f: &mut Frame, r: CellRect, text: &str, ctx: &DrawCtx) {
    let c = &ctx.theme.colors;
    put_elided(f, r.col, r.row, text, Style::new(c.foreground, c.surface), r.cols);
}

/// Muted text (hints, notes).
pub fn muted(f: &mut Frame, r: CellRect, text: &str, ctx: &DrawCtx) {
    let c = &ctx.theme.colors;
    put_elided(f, r.col, r.row, text, Style::new(c.muted, c.surface), r.cols);
}

/// The look of a field: on the window background like text fields; the
/// focused one in the active color, like a focused button.
fn field_style(focused: bool, ctx: &DrawCtx) -> Style {
    let c = &ctx.theme.colors;
    if focused { Style::new(c.surface, c.border_active).with(Flags::BOLD) } else { Style::new(c.foreground, c.background) }
}

/// A closed combo box: " value      ▾".
pub fn draw_combo(f: &mut Frame, r: CellRect, text: &str, focused: bool, ctx: &DrawCtx) {
    if r.cols < 4 || r.rows == 0 {
        return;
    }
    let style = field_style(focused, ctx);
    fill(f, r, style.bg);
    put_elided(f, r.col + 1, r.row, text, style, r.cols.saturating_sub(4));
    put(f, right(r) - 2, r.row, "▾", style, 1);
}

/// A spin box: "‹ 14 pt ›" (←/→, the wheel or a click on a side changes it).
pub fn draw_spin(f: &mut Frame, r: CellRect, text: &str, focused: bool, ctx: &DrawCtx) {
    if r.cols < 6 || r.rows == 0 {
        return;
    }
    let style = field_style(focused, ctx);
    fill(f, r, style.bg);
    put(f, r.col + 1, r.row, "‹", style, 1);
    let w = width(text).min(r.cols - 4);
    put(f, r.col + (r.cols - w) / 2, r.row, text, style, w);
    put(f, right(r) - 2, r.row, "›", style, 1);
}

/// Which way a click on a spin box at `col` turns it.
pub fn spin_hit(r: CellRect, col: u16) -> i32 {
    if col < r.col + r.cols / 2 { -1 } else { 1 }
}

/// A checkbox: "[x] Label"; the focused one in the active color.
pub fn draw_check(f: &mut Frame, r: CellRect, text: &str, on: bool, focused: bool, ctx: &DrawCtx) {
    let c = &ctx.theme.colors;
    let style = Style::new(if focused { c.border_active } else { c.foreground }, c.surface);
    let style = if focused { style.with(Flags::BOLD) } else { style };
    let x = put(f, r.col, r.row, if on { "[x] " } else { "[ ] " }, style, r.cols);
    put_elided(f, x, r.row, text, style, right(r).saturating_sub(x));
}

/// Cells a push button with this label takes.
pub fn button_w(text: &str) -> u16 {
    (width(text) + 4).max(8)
}

/// A push button: the label centered on the border color; the focused one
/// in the active color.
pub fn draw_button(f: &mut Frame, r: CellRect, text: &str, focused: bool, ctx: &DrawCtx) {
    if r.cols == 0 || r.rows == 0 {
        return;
    }
    let c = &ctx.theme.colors;
    let style = if focused { Style::new(c.surface, c.border_active).with(Flags::BOLD) } else { Style::new(c.foreground, c.border) };
    fill(f, r, style.bg);
    let w = width(text).min(r.cols);
    put_elided(f, r.col + (r.cols - w) / 2, r.row, text, style, r.cols);
}

/// The next selectable item from `current` in direction `delta`; stops at
/// the ends.
pub fn step(items: &[Choice], current: usize, delta: i32) -> usize {
    let mut i = current as i64;
    loop {
        i += delta as i64;
        if i < 0 || i >= items.len() as i64 {
            return current;
        }
        if items[i as usize].is_some() {
            return i as usize;
        }
    }
}

/// Lines of at most `w` cells, broken at spaces.
pub fn wrap(text: &str, w: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && orchard_grid::str_width(&line) + 1 + orchard_grid::str_width(word) > w {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// A row of push buttons, one of which has the focus.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ButtonRow {
    pub labels: Vec<String>,
    pub focus: usize,
}

/// What a key did to a button row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowOutcome {
    Ignored,
    Consumed,
    /// Button `i` was pressed.
    Pressed(usize),
}

impl ButtonRow {
    pub fn new(labels: &[&str]) -> Self {
        ButtonRow { labels: labels.iter().map(|l| l.to_string()).collect(), focus: 0 }
    }

    fn cells(&self) -> u16 {
        self.labels.iter().map(|l| button_w(l)).sum::<u16>() + 2 * (self.labels.len() as u16).saturating_sub(1)
    }

    /// Each button's cells, centered in `row`.
    pub fn rects(&self, row: CellRect) -> Vec<CellRect> {
        let mut x = row.col + row.cols.saturating_sub(self.cells()) / 2;
        self.labels
            .iter()
            .map(|l| {
                let r = widgets::clip(CellRect::new(x, row.row, button_w(l), 1), row);
                x += button_w(l) + 2;
                r
            })
            .collect()
    }

    pub fn hit(&self, row: CellRect, col: u16, line: u16) -> Option<usize> {
        self.rects(row).iter().position(|r| contains(*r, col, line))
    }

    /// `focused`: the row has the dialog's focus (shows the focused button).
    pub fn draw(&self, f: &mut Frame, row: CellRect, ctx: &DrawCtx, focused: bool) {
        for (i, (l, r)) in self.labels.iter().zip(self.rects(row)).enumerate() {
            draw_button(f, r, l, focused && i == self.focus, ctx);
        }
    }

    /// ←/→ move the focus; Enter or Space press the focused button.
    pub fn handle_key(&mut self, ev: &KeyEvent) -> RowOutcome {
        let n = self.labels.len();
        if n == 0 || !ev.mods.none() {
            return RowOutcome::Ignored;
        }
        match &ev.key {
            Key::Named(Named::Left) => self.focus = (self.focus + n - 1) % n,
            Key::Named(Named::Right) => self.focus = (self.focus + 1) % n,
            Key::Named(Named::Enter) | Key::Char(' ') => return RowOutcome::Pressed(self.focus),
            _ => return RowOutcome::Ignored,
        }
        RowOutcome::Consumed
    }
}

/// An open combo box's list, over everything else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Popup {
    /// The field it belongs to.
    pub field: CellRect,
    pub cursor: usize,
    pub top: usize,
}

/// What a key or click did to an open popup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupOutcome {
    Consumed,
    /// Item `i` was chosen.
    Chosen(usize),
    /// Closed without a choice.
    Closed,
}

impl Popup {
    pub fn open(field: CellRect, items: &[Choice], current: usize) -> Popup {
        let mut p = Popup { field, cursor: current.min(items.len().saturating_sub(1)), top: 0 };
        p.scroll_into_view(items.len());
        p
    }

    fn visible(n: usize) -> usize {
        n.clamp(1, MAX_VISIBLE as usize)
    }

    fn scroll_into_view(&mut self, n: usize) {
        let v = Self::visible(n);
        if self.cursor < self.top {
            self.top = self.cursor;
        } else if self.cursor >= self.top + v {
            self.top = self.cursor + 1 - v;
        }
        self.top = self.top.min(n.saturating_sub(v));
    }

    /// The box: below the field if it fits, else above; at least as wide as the field.
    pub fn rect(&self, items: &[Choice], screen: CellRect) -> CellRect {
        let longest = items.iter().flatten().map(|s| width(s)).max().unwrap_or(0);
        let w = (longest + 4).max(self.field.cols).min(screen.cols);
        let h = (Self::visible(items.len()) as u16 + 2).min(screen.rows);
        let x = self.field.col.min(right(screen).saturating_sub(w));
        let below = bottom(self.field);
        let y = if below + h <= bottom(screen) {
            below
        } else if self.field.row >= h + screen.row {
            self.field.row - h
        } else {
            bottom(screen).saturating_sub(h)
        };
        CellRect::new(x, y, w, h)
    }

    pub fn draw(&self, f: &mut Frame, items: &[Choice], screen: CellRect, ctx: &DrawCtx) {
        let c = &ctx.theme.colors;
        let r = self.rect(items, screen);
        cover(f, r, ctx);
        let inner = BoxFrame { title: "", subtitle: "", active: true }.draw(f, r, ctx);
        let mut p = *self;
        p.cursor = p.cursor.min(items.len().saturating_sub(1));
        p.scroll_into_view(items.len());
        for k in 0..inner.rows as usize {
            let i = p.top + k;
            let Some(item) = items.get(i) else { break };
            let row = CellRect::new(inner.col, inner.row + k as u16, inner.cols, 1);
            match item {
                Some(text) => {
                    let style = if i == p.cursor { Style::new(c.cursor_fg, c.cursor_bg) } else { Style::new(c.foreground, c.surface) };
                    fill(f, row, style.bg);
                    put_elided(f, row.col + 1, row.row, text, style, row.cols.saturating_sub(2));
                }
                None => {
                    let line = "─".repeat(row.cols.saturating_sub(2) as usize);
                    put(f, row.col + 1, row.row, &line, Style::new(c.border, c.surface), row.cols.saturating_sub(2));
                }
            }
        }
        Scrollbar { total: items.len(), visible: inner.rows as usize, top: p.top }.draw(f, inner, ctx);
    }

    pub fn handle_key(&mut self, k: &KeyEvent, items: &[Choice]) -> PopupOutcome {
        let n = items.len();
        let page = Self::visible(n) as i32 - 1;
        let mv = |p: &mut Popup, to: usize| {
            p.cursor = to.min(n.saturating_sub(1));
            if items.get(p.cursor).is_some_and(Option::is_none) {
                p.cursor = step(items, p.cursor, 1);
            }
        };
        match &k.key {
            Key::Named(Named::Escape | Named::Tab) => return PopupOutcome::Closed,
            Key::Named(Named::Enter) | Key::Char(' ') => {
                return match items.get(self.cursor) {
                    Some(Some(_)) => PopupOutcome::Chosen(self.cursor),
                    _ => PopupOutcome::Consumed,
                };
            }
            Key::Named(Named::Up) => self.cursor = step(items, self.cursor, -1),
            Key::Named(Named::Down) => self.cursor = step(items, self.cursor, 1),
            Key::Named(Named::PageUp) => mv(self, self.cursor.saturating_sub(page as usize)),
            Key::Named(Named::PageDown) => mv(self, self.cursor + page as usize),
            Key::Named(Named::Home) => mv(self, 0),
            Key::Named(Named::End) => mv(self, n.saturating_sub(1)),
            _ => {
                // Type to jump to the next item starting with the letter.
                if let Some(text) = k.typed() {
                    let needle = text.to_lowercase();
                    for d in 1..=n {
                        let i = (self.cursor + d) % n;
                        if items[i].as_ref().is_some_and(|s| s.to_lowercase().starts_with(&needle)) {
                            self.cursor = i;
                            break;
                        }
                    }
                }
            }
        }
        self.scroll_into_view(n);
        PopupOutcome::Consumed
    }

    pub fn handle_mouse(&mut self, m: &MouseEvent, items: &[Choice], screen: CellRect) -> PopupOutcome {
        let r = self.rect(items, screen);
        let inner = inset(r, 1, 1);
        let n = items.len();
        match m.kind {
            MouseKind::Scroll(lines) => {
                let max = n.saturating_sub(Self::visible(n));
                self.top = (self.top as f32 - lines * 3.0).round().clamp(0.0, max as f32) as usize;
                PopupOutcome::Consumed
            }
            MouseKind::Down if !contains(r, m.col, m.row) => PopupOutcome::Closed,
            MouseKind::Down if contains(inner, m.col, m.row) => {
                let i = self.top + (m.row - inner.row) as usize;
                match items.get(i) {
                    Some(Some(_)) => PopupOutcome::Chosen(i),
                    _ => PopupOutcome::Consumed,
                }
            }
            _ => PopupOutcome::Consumed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<Choice> {
        vec![Some("monokai".into()), Some("nord".into()), None, Some("Mine".into())]
    }

    #[test]
    fn steps_skip_separators_and_stop_at_the_ends() {
        let it = items();
        assert_eq!(step(&it, 1, 1), 3);
        assert_eq!(step(&it, 3, -1), 1);
        assert_eq!(step(&it, 3, 1), 3);
        assert_eq!(step(&it, 0, -1), 0);
    }

    #[test]
    fn popup_keys_and_placement() {
        let it = items();
        let screen = CellRect::new(0, 0, 80, 30);
        let mut p = Popup::open(CellRect::new(10, 5, 20, 1), &it, 1);
        assert_eq!(p.rect(&it, screen), CellRect::new(10, 6, 20, 6), "below the field, as wide");
        assert_eq!(p.handle_key(&KeyEvent::named(Named::Down), &it), PopupOutcome::Consumed);
        assert_eq!(p.cursor, 3, "over the separator");
        assert_eq!(p.handle_key(&KeyEvent::named(Named::Enter), &it), PopupOutcome::Chosen(3));
        p.handle_key(&KeyEvent::char('n'), &it);
        assert_eq!(p.cursor, 1, "type to jump");
        assert_eq!(p.handle_key(&KeyEvent::named(Named::Escape), &it), PopupOutcome::Closed);
        // Near the bottom it opens above the field.
        let low = Popup::open(CellRect::new(10, 27, 20, 1), &it, 0);
        assert_eq!(bottom(low.rect(&it, screen)), 27);
    }

    #[test]
    fn wraps_at_spaces() {
        assert_eq!(wrap("Delete 'Mine'? You can't undo this.", 16), ["Delete 'Mine'?", "You can't undo", "this."]);
    }
}
