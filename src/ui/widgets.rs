//! Generic grid widgets in Orchard Commander's style: the box frame
//! (border through the middle of the edge cells, title chip), a pixel
//! scrollbar, the F-key bar and a one-line text field. Ported from
//! `oc_ui::widgets`; candidates for a shared Orchard UI crate.

use oc_grid::{CellRect, Flags, Frame, Layer, PxRect, Rgba, Style, elide_left, elide_right, str_width};

use crate::theme::Theme;

/// What drawing needs besides the state.
#[derive(Clone, Copy)]
pub struct DrawCtx<'a> {
    pub theme: &'a Theme,
    /// Cell size in physical pixels.
    pub cell_px: (f32, f32),
}

pub fn width(text: &str) -> u16 {
    str_width(text).min(u16::MAX as usize) as u16
}

/// Put `text` at (col, row), at most `max` cells; returns the column after it.
pub fn put(f: &mut Frame, col: u16, row: u16, text: &str, style: Style, max: u16) -> u16 {
    f.put_str_clipped(col, row, text, style, col.saturating_add(max))
}

pub fn fill(f: &mut Frame, r: CellRect, bg: Rgba) {
    f.fill(r, ' ', Style::new(bg, bg));
}

pub fn inset(r: CellRect, dx: u16, dy: u16) -> CellRect {
    CellRect::new(
        r.col + dx,
        r.row + dy,
        r.cols.saturating_sub(2 * dx),
        r.rows.saturating_sub(2 * dy),
    )
}

pub fn contains(r: CellRect, col: u16, row: u16) -> bool {
    col >= r.col && col < r.col + r.cols && row >= r.row && row < r.row + r.rows
}

pub fn px(r: CellRect, ctx: &DrawCtx) -> PxRect {
    let (cw, ch) = ctx.cell_px;
    PxRect::new(r.col as f32 * cw, r.row as f32 * ch, r.cols as f32 * cw, r.rows as f32 * ch)
}

/// Remove the pixel overlays (rects, images) already in `f` from `area`, so
/// a dialog drawn there isn't covered by what is under it: overlays sit
/// above cell backgrounds. Partly covered rects are cut into pieces.
pub fn punch(f: &mut Frame, area: CellRect, ctx: &DrawCtx) {
    let hole = px(area, ctx);
    let (cw, ch) = ctx.cell_px;
    let to_px = |a: oc_grid::Area| match a {
        oc_grid::Area::Px(r) => r,
        oc_grid::Area::Cells(c) => PxRect::new(c.col as f32 * cw, c.row as f32 * ch, c.cols as f32 * cw, c.rows as f32 * ch),
    };
    let mut out = Vec::with_capacity(f.rects.len());
    for r in f.rects.drain(..) {
        let p = to_px(r.area);
        let i = p.intersect(&hole);
        if i.is_empty() {
            out.push(r);
            continue;
        }
        let pieces = [
            PxRect::new(p.x, p.y, p.w, i.y - p.y),                   // above
            PxRect::new(p.x, i.bottom(), p.w, p.bottom() - i.bottom()), // below
            PxRect::new(p.x, i.y, i.x - p.x, i.h),                   // left
            PxRect::new(i.right(), i.y, p.right() - i.right(), i.h),  // right
        ];
        for piece in pieces.into_iter().filter(|q| q.w > 0.0 && q.h > 0.0) {
            out.push(oc_grid::FillRect { area: piece.into(), ..r });
        }
    }
    f.rects = out;
    f.images.retain(|img| to_px(img.dest).intersect(&hole).is_empty());
}

/// A box with an optional title (top edge, left) and subtitle (bottom edge, right).
#[derive(Clone, Copy, Debug, Default)]
pub struct BoxFrame<'a> {
    pub title: &'a str,
    pub subtitle: &'a str,
    /// Focused: border and title in the active color.
    pub active: bool,
}

impl<'a> BoxFrame<'a> {
    pub fn new(title: &'a str) -> Self {
        BoxFrame { title, ..Default::default() }
    }

    fn edge_text(text: &str, r: CellRect) -> Option<String> {
        let room = r.cols.saturating_sub(6);
        if text.is_empty() || room < 2 {
            return None;
        }
        Some(format!(" {} ", elide_left(text, room.saturating_sub(2) as usize)))
    }

    /// Paint the box into `r` and return the inside (one cell in from each edge).
    ///
    /// The border runs through the middle of the edge cells: their outer
    /// halves keep the window background, the inner halves get the surface
    /// color (pixel strips under the text layer).
    pub fn draw(&self, f: &mut Frame, r: CellRect, ctx: &DrawCtx) -> CellRect {
        if r.cols < 2 || r.rows < 2 {
            return CellRect::new(r.col, r.row, 0, 0);
        }
        let c = &ctx.theme.colors;
        let inner = inset(r, 1, 1);
        fill(f, r, c.background);
        fill(f, inner, c.surface);
        let line = if self.active { c.border_active } else { c.border };
        let border = if self.active {
            Style::new(line, c.background).with(Flags::STRONG)
        } else {
            Style::new(line, c.background)
        };
        f.draw_box(r, ctx.theme.frame, border);

        let right = r.col + r.cols;
        let title = Self::edge_text(self.title, r);
        let title_span = title.as_ref().map(|t| (r.col + 2, r.col + 2 + width(t)));
        let subtitle = Self::edge_text(self.subtitle, r);
        let sub_span = subtitle.as_ref().map(|t| {
            let w = width(t);
            (right.saturating_sub(2 + w).max(r.col + 1), right.saturating_sub(2))
        });
        surface_strips(f, r, ctx, title_span, sub_span);

        if let (Some(text), Some((x, _))) = (&title, title_span) {
            let style = if self.active {
                Style::new(c.surface, line).with(Flags::BOLD)
            } else {
                Style::new(c.muted, c.surface)
            };
            put(f, x, r.row, text, style, r.cols.saturating_sub(4));
        }
        if let (Some(text), Some((x, end))) = (&subtitle, sub_span) {
            put(f, x, r.row + r.rows - 1, text, Style::new(c.muted, c.surface), end - x);
        }
        inner
    }
}

/// Inner half-cell strips along the border, skipping `[a, b)` column spans on
/// the top and bottom rows (where the title and subtitle are).
fn surface_strips(f: &mut Frame, r: CellRect, ctx: &DrawCtx, top_gap: Option<(u16, u16)>, bottom_gap: Option<(u16, u16)>) {
    let (cw, ch) = ctx.cell_px;
    let color = ctx.theme.colors.surface;
    let (x0, y0) = (r.col as f32 * cw, r.row as f32 * ch);
    let (x1, y1) = ((r.col + r.cols) as f32 * cw, (r.row + r.rows) as f32 * ch);
    let (hx, hy) = ((cw / 2.0).floor(), (ch / 2.0).floor());
    let mut hstrip = |y: f32, gap: Option<(u16, u16)>| {
        let (start, end) = (x0 + hx, x1 - hx);
        let mut segs = vec![(start, end)];
        if let Some((a, b)) = gap {
            let (ga, gb) = (a as f32 * cw, b as f32 * cw);
            segs = vec![(start, ga.max(start)), (gb.min(end), end)];
        }
        for (a, b) in segs {
            if b > a {
                f.add_rect(PxRect::new(a, y, b - a, ch - hy), color, Layer::BelowText);
            }
        }
    };
    hstrip(y0 + hy, top_gap);
    hstrip(y1 - ch, bottom_gap);
    let (sy, sh) = (y0 + ch, (y1 - ch) - (y0 + ch));
    if sh > 0.0 {
        f.add_rect(PxRect::new(x0 + hx, sy, cw - hx, sh), color, Layer::BelowText);
        f.add_rect(PxRect::new(x1 - cw, sy, cw - hx, sh), color, Layer::BelowText);
    }
}

/// A thin vertical scrollbar drawn as a pixel-exact thumb over the right
/// half of the last column of a view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scrollbar {
    pub total: usize,
    pub visible: usize,
    pub top: usize,
}

impl Scrollbar {
    pub fn needed(&self) -> bool {
        self.total > self.visible && self.visible > 0
    }

    pub fn draw(&self, f: &mut Frame, r: CellRect, ctx: &DrawCtx) {
        if !self.needed() || r.rows == 0 {
            return;
        }
        let (cw, ch) = ctx.cell_px;
        let w = (cw / 2.0).round().max(2.0);
        let x = (r.col + r.cols) as f32 * cw - w;
        let (y0, h) = (r.row as f32 * ch, r.rows as f32 * ch);
        let len = (self.visible as f32 / self.total as f32).max(0.05);
        let max_top = (self.total - self.visible) as f32;
        let travel = (self.top as f32 / max_top).min(1.0);
        let thumb_h = (len * h).max(ch);
        let y = y0 + travel * (h - thumb_h);
        f.add_rect(PxRect::new(x, y0, w, h), ctx.theme.colors.surface, Layer::AboveText);
        f.add_rect(PxRect::new(x, y, w, thumb_h), ctx.theme.colors.border, Layer::AboveText);
    }
}

/// The F-key bar: "1Help 2Open …", keys bold in the key color. Items that
/// don't fit wrap to the next row of `area`. Returns each item's cells and
/// its key number, for clicks.
pub fn key_bar_items(area: CellRect, keys: &[(&str, &str)]) -> Vec<(CellRect, usize)> {
    let mut out = Vec::new();
    let (left, right) = (area.col + 1, area.col + area.cols);
    let (mut x, mut row) = (left, area.row);
    for (i, (key, label)) in keys.iter().enumerate() {
        let w = (width(key) + width(label)).min(right.saturating_sub(left));
        if x + w > right && x > left {
            x = left;
            row += 1;
        }
        if row >= area.row + area.rows.max(1) {
            break;
        }
        out.push((CellRect::new(x, row, w, 1), i));
        x += w + 2;
    }
    out
}

/// Rows the key bar needs at `cols` wide.
pub fn key_bar_rows(cols: u16, keys: &[(&str, &str)]) -> u16 {
    let all = CellRect::new(0, 0, cols, u16::MAX);
    key_bar_items(all, keys).last().map_or(1, |(r, _)| r.row + 1)
}

pub fn draw_key_bar(f: &mut Frame, row: CellRect, keys: &[(&str, &str)], ctx: &DrawCtx) {
    let c = &ctx.theme.colors;
    fill(f, row, c.footer_bg);
    for (r, i) in key_bar_items(row, keys) {
        let (key, label) = keys[i];
        let x = put(f, r.col, r.row, key, Style::new(c.footer_key, c.footer_bg).with(Flags::BOLD), r.cols);
        let room = r.cols.saturating_sub(x - r.col);
        put(f, x, r.row, &elide_right(label, room as usize), Style::new(c.footer_fg, c.footer_bg), room);
    }
}

/// A one-line text field: text and a cursor (char index).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextField {
    pub text: String,
    pub cursor: usize,
}

impl TextField {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let cursor = text.chars().count();
        TextField { text, cursor }
    }

    fn byte(&self, i: usize) -> usize {
        self.text.char_indices().nth(i).map_or(self.text.len(), |(b, _)| b)
    }

    /// Edit keys; returns whether the key was taken.
    pub fn handle(&mut self, ev: &crate::input::KeyEvent) -> bool {
        use crate::input::{Key, Named};
        let n = self.text.chars().count();
        match &ev.key {
            Key::Named(Named::Left) => self.cursor = self.cursor.saturating_sub(1),
            Key::Named(Named::Right) => self.cursor = (self.cursor + 1).min(n),
            Key::Named(Named::Home) => self.cursor = 0,
            Key::Named(Named::End) => self.cursor = n,
            Key::Named(Named::Backspace) if self.cursor > 0 => {
                let (a, b) = (self.byte(self.cursor - 1), self.byte(self.cursor));
                self.text.replace_range(a..b, "");
                self.cursor -= 1;
            }
            Key::Named(Named::Delete) if self.cursor < n => {
                let (a, b) = (self.byte(self.cursor), self.byte(self.cursor + 1));
                self.text.replace_range(a..b, "");
            }
            Key::Named(Named::Backspace | Named::Delete) => {}
            _ => match ev.typed() {
                Some(t) => {
                    let at = self.byte(self.cursor);
                    self.text.insert_str(at, t);
                    self.cursor += t.chars().count();
                }
                None => return false,
            },
        }
        true
    }

    /// Draw into one row; the text scrolls to keep the cursor in view.
    pub fn draw(&self, f: &mut Frame, r: CellRect, ctx: &DrawCtx) {
        let c = &ctx.theme.colors;
        let style = Style::new(c.foreground, c.cursor_inactive_bg);
        f.fill(r, ' ', style);
        let chars: Vec<char> = self.text.chars().collect();
        let room = r.cols.saturating_sub(1) as usize;
        let start = self.cursor.saturating_sub(room);
        let shown: String = chars[start..].iter().collect();
        put(f, r.col, r.row, &shown, style, r.cols);
        let cur_col = r.col + width(&chars[start..self.cursor].iter().collect::<String>());
        f.cursor = Some(oc_grid::Cursor {
            col: cur_col.min(r.col + r.cols.saturating_sub(1)),
            row: r.row,
            shape: oc_grid::CursorShape::Beam,
            color: c.accent,
            text: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{KeyEvent, Named};

    #[test]
    fn key_bar_wraps() {
        let keys = [("1", "Help"), ("2", "Open"), ("10", "Quit")];
        // " 1Help  2Open  10Quit" needs 21 columns.
        assert_eq!(key_bar_rows(21, &keys), 1);
        assert_eq!(key_bar_rows(14, &keys), 2);
        let items = key_bar_items(CellRect::new(0, 5, 14, 2), &keys);
        let rows: Vec<u16> = items.iter().map(|(r, _)| r.row).collect();
        assert_eq!(rows, [5, 5, 6]);
        assert_eq!(items[2].0.col, 1);
    }

    #[test]
    fn text_field_edits() {
        let mut t = TextField::new("abc");
        t.handle(&KeyEvent::named(Named::Left));
        t.handle(&KeyEvent::char('X'));
        assert_eq!((t.text.as_str(), t.cursor), ("abXc", 3));
        t.handle(&KeyEvent::named(Named::Backspace));
        t.handle(&KeyEvent::named(Named::Delete));
        assert_eq!(t.text, "ab");
    }
}
