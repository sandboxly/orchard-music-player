//! The sample on the Appearance page: a tiny player and playlist in the
//! current colors, frame style and font (the dialog covers much of the
//! window behind it, which previews the rest).

use oc_grid::{CellRect, Flags, Frame, Style};

use crate::ui::widgets::{BoxFrame, DrawCtx, bottom, fill, put, put_elided, right, width};

const TRACKS: [(&str, &str); 3] = [("Teardrop", "5:30"), ("Roads", "5:05"), ("Kerala", "4:01")];
const PLAYING: usize = 0;
const CURSOR: usize = 2;
/// Spectrum bar heights in eighths of the two bar rows.
const BARS: [u8; 11] = [14, 16, 12, 15, 10, 11, 8, 9, 6, 4, 3];
const KEYS: [(&str, &str); 3] = [("1", "Help"), ("2", "Open"), ("9", "Settings")];

/// The narrowest it is drawn.
pub const MIN_WIDTH: u16 = 22;
pub const MAX_WIDTH: u16 = 30;
/// Two boxes of 3 inner rows, the key bar.
pub const HEIGHT: u16 = 5 + 5 + 1;

/// Draw the sample into `r` (at most [`HEIGHT`] rows).
pub fn draw(f: &mut Frame, r: CellRect, ctx: &DrawCtx) {
    let c = &ctx.theme.colors;
    let r = CellRect::new(r.col, r.row, r.cols.min(MAX_WIDTH), r.rows.min(HEIGHT));
    if r.rows < HEIGHT || r.cols < MIN_WIDTH {
        return;
    }
    let player = CellRect::new(r.col, r.row, r.cols, 5);
    let inner = BoxFrame { title: "Now playing", subtitle: "", active: true }.draw(f, player, ctx);
    let (x, w) = (inner.col + 1, inner.cols.saturating_sub(2));
    put(f, x, inner.row, "▶", Style::new(c.success, c.surface), 1);
    put_elided(f, x + 2, inner.row, TRACKS[PLAYING].0, Style::new(c.foreground, c.surface).with(Flags::BOLD), w.saturating_sub(7));
    let time = "2:07";
    put(f, right(inner) - 1 - width(time), inner.row, time, Style::new(c.foreground, c.surface).with(Flags::BOLD), width(time));
    // Two rows of spectrum bars, green to red as they rise.
    for (i, h) in BARS.iter().enumerate() {
        let bx = x + i as u16 * 2;
        if bx + 1 > right(inner) - 1 {
            break;
        }
        for (row, level) in [(inner.row + 2, 0u8), (inner.row + 1, 8u8)] {
            let color = if level == 0 { c.success } else if *h >= 14 { c.error } else { c.warning };
            let glyph = match h.saturating_sub(level).min(8) {
                0 => " ",
                1..=2 => "▂",
                3..=4 => "▄",
                5..=6 => "▆",
                _ => "█",
            };
            put(f, bx, row, glyph, Style::new(color, c.surface), 1);
        }
    }

    let list = CellRect::new(r.col, bottom(player), r.cols, 5);
    let inner = BoxFrame { title: "Playlist", subtitle: "", active: false }.draw(f, list, ctx);
    for (i, (title, dur)) in TRACKS.iter().enumerate() {
        let row = CellRect::new(inner.col, inner.row + i as u16, inner.cols, 1);
        let (fg, bg) = match i {
            CURSOR => (c.cursor_fg, c.cursor_bg),
            PLAYING => (c.accent, c.surface),
            _ => (c.foreground, c.surface),
        };
        fill(f, row, bg);
        let mut style = Style::new(fg, bg);
        if i == PLAYING {
            style = style.with(Flags::BOLD);
            put(f, row.col + 1, row.row, "▶", Style::new(c.success, bg), 1);
        }
        let num = Style::new(if i == CURSOR { c.cursor_fg } else { c.muted }, bg);
        put(f, row.col + 3, row.row, &format!("{}.", i + 1), num, 2);
        put_elided(f, row.col + 6, row.row, title, style, row.cols.saturating_sub(13));
        put(f, right(row) - 1 - width(dur), row.row, dur, num, width(dur));
    }

    let bar = CellRect::new(r.col, bottom(list), r.cols, 1);
    fill(f, bar, c.footer_bg);
    let mut x = bar.col + 1;
    for (key, label) in KEYS {
        if x + width(key) + width(label) > right(bar) {
            break;
        }
        x = put(f, x, bar.row, key, Style::new(c.footer_key, c.footer_bg).with(Flags::BOLD), width(key));
        x = put(f, x, bar.row, label, Style::new(c.footer_fg, c.footer_bg), width(label)) + 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    #[test]
    fn sample_shows_tracks_cursor_and_keys() {
        let theme = Theme::default_dark();
        let ctx = DrawCtx { theme: &theme, cell_px: (8.0, 16.0) };
        let mut f = Frame::new(MAX_WIDTH, HEIGHT, theme.colors.background);
        draw(&mut f, CellRect::new(0, 0, MAX_WIDTH, HEIGHT), &ctx);
        assert!(f.row_text(0).contains("Now playing"));
        assert!(f.row_text(1).contains("Teardrop") && f.row_text(1).contains("2:07"));
        assert!(f.row_text(8).contains("3. Kerala"));
        assert_eq!(f.cell(8, 8).unwrap().bg, theme.colors.cursor_bg, "Kerala has the cursor");
        assert!(f.row_text(HEIGHT - 1).starts_with(" 1Help  2Open  9Settings"));
    }
}
