//! Drawing: a pure function of the app state into an `orchard_grid::Frame`.
//! The layout is recomputed, never stored; mouse hit testing uses it too.

pub mod settings;

use orchard_grid::{CellRect, Flags, Frame, ImageId, ImagePlacement, Layer, PxRect, Rgba, Style, elide_right};
use orchard_kit::widgets::{self, BoxFrame, DrawCtx, Scrollbar, contains, draw_key_bar, fill, inset, put, width};
use orchard_theme::mix;

use crate::app::{App, Modal, format_time};
use crate::audio::State;
use crate::playlist::Repeat;
use crate::spectrum;

pub const FKEYS: &[(&str, &str)] = &[
    ("F1", "Help"),
    ("F2", "Open"),
    ("F3", "Visualizer"),
    ("F4", "Save"),
    ("F5", "Load"),
    ("F6", "Shuffle"),
    ("F7", "Repeat"),
    ("F8", "Remove"),
    ("F9", "Settings"),
    ("F10", "Quit"),
];

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Layout {
    pub player: CellRect,
    pub cover: Option<CellRect>,
    /// Title / time and artist / format lines.
    pub info: CellRect,
    pub vis: CellRect,
    pub seek: CellRect,
    pub controls: CellRect,
    pub volume: CellRect,
    pub playlist: CellRect,
    /// The playlist's rows.
    pub list: CellRect,
    /// The quick filter's line under the rows, while filtering.
    pub filter: Option<CellRect>,
    pub footer: CellRect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Prev,
    Play,
    Pause,
    Stop,
    Next,
    Shuffle,
    Repeat,
}

const TRANSPORT: &[(&str, Button)] = &[
    ("|◀", Button::Prev),
    ("▶", Button::Play),
    ("❚❚", Button::Pause),
    ("■", Button::Stop),
    ("▶|", Button::Next),
];

/// Width of the volume bar, in cells.
const VOLUME_CELLS: u16 = 14;

fn has_cover(app: &App) -> bool {
    app.now.as_ref().and_then(|n| n.details.as_ref()).is_some_and(|d| d.cover.is_some())
}

pub fn layout(app: &App) -> Layout {
    let (w, h) = (app.cols.max(20), app.rows.max(8));
    // The key bar wraps on narrow windows, but leaves the panels room.
    let footer_h = widgets::key_bar_rows(w, FKEYS).min(h / 4).max(1);
    let footer = CellRect::new(0, h - footer_h, w, footer_h);
    let avail = h - footer_h;
    let vis_rows = if avail < 18 { 2 } else { ((avail as f32 * 0.28) as u16).clamp(3, 12) };
    let player_h = (vis_rows + 6).min(avail.saturating_sub(4));
    let player = CellRect::new(0, 0, w, player_h);
    let inner = inset(player, 1, 1);
    let upper = 2 + vis_rows;
    let cover = has_cover(app).then(|| {
        let (cw, ch) = app.cell_px;
        let cols = ((upper as f32 * ch / cw).round() as u16).min(w / 3);
        CellRect::new(inner.col + 1, inner.row, cols, upper)
    });
    let left = inner.col + 1 + cover.map_or(0, |c| c.cols + 2);
    let right = inner.col + inner.cols.saturating_sub(1);
    let text_w = right.saturating_sub(left);
    let info = CellRect::new(left, inner.row, text_w, 2);
    let vis = CellRect::new(left, inner.row + 2, text_w, vis_rows);
    let seek = CellRect::new(inner.col + 1, inner.row + upper, inner.cols.saturating_sub(2), 1);
    let controls = CellRect::new(inner.col + 1, inner.row + upper + 1, inner.cols.saturating_sub(2), 1);
    let vol_w = VOLUME_CELLS.min(controls.cols / 3);
    let volume = CellRect::new(controls.col + controls.cols - vol_w, controls.row, vol_w, 1);
    let playlist = CellRect::new(0, player_h, w, avail - player_h);
    let mut list = inset(playlist, 1, 1);
    let filter = (app.filter.is_some() && list.rows > 2).then(|| {
        list.rows -= 1;
        CellRect::new(list.col, list.row + list.rows, list.cols, 1)
    });
    Layout { player, cover, info, vis, seek, controls, volume, playlist, list, filter, footer }
}

/// How many spectrum bars fit the visualizer.
pub fn bar_count(app: &App) -> usize {
    let l = layout(app);
    let (bar, gap) = bar_metrics(app.cell_px);
    let w = l.vis.cols as f32 * app.cell_px.0;
    ((w + gap) / (bar + gap)).floor().clamp(1.0, 128.0) as usize
}

fn bar_metrics((cw, _): (f32, f32)) -> (f32, f32) {
    let gap = (cw * 0.25).round().max(1.0);
    let bar = (cw * 1.25).round().max(3.0);
    (bar, gap)
}

/// The transport and toggle buttons on the controls row: (cells, button).
fn buttons(app: &App, l: &Layout) -> Vec<(CellRect, Button, String)> {
    let mut out = Vec::new();
    let mut x = l.controls.col;
    for (label, b) in TRANSPORT {
        let w = width(label) + 2;
        out.push((CellRect::new(x, l.controls.row, w, 1), *b, format!(" {label} ")));
        x += w + 1;
    }
    x += 2;
    let shuffle = format!("shuffle {}", if app.shuffle { "on" } else { "off" });
    let w = width(&shuffle);
    out.push((CellRect::new(x, l.controls.row, w, 1), Button::Shuffle, shuffle));
    x += w + 3;
    let repeat = format!("repeat {}", app.repeat.label());
    out.push((CellRect::new(x, l.controls.row, width(&repeat), 1), Button::Repeat, repeat));
    out.retain(|(r, ..)| r.col + r.cols < l.volume.col.saturating_sub(6));
    out
}

pub fn button_at(app: &App, l: &Layout, col: u16, row: u16) -> Option<Button> {
    buttons(app, l).into_iter().find(|(r, ..)| contains(*r, col, row)).map(|(_, b, _)| b)
}

/// The image id under which the window keeps the cover.
pub fn cover_image(cover_id: u64) -> ImageId {
    ImageId::external(cover_id)
}

pub fn draw(app: &App, f: &mut Frame) {
    let ctx = DrawCtx { theme: &app.theme, cell_px: app.cell_px };
    let c = &app.theme.colors;
    f.clear(c.background);
    f.cursor = None;
    let l = layout(app);
    draw_player(app, f, &l, &ctx);
    draw_playlist(app, f, &l, &ctx);
    draw_key_bar(f, l.footer, FKEYS, &ctx);
    if let Some(m) = &app.modal {
        draw_modal(app, m, f, &ctx);
    }
    draw_toast(app, f, &l, &ctx);
}

// ---------------------------------------------------------------------------
// The player panel
// ---------------------------------------------------------------------------

fn draw_player(app: &App, f: &mut Frame, l: &Layout, ctx: &DrawCtx) {
    let c = &app.theme.colors;
    let state = app.state();
    let frame = BoxFrame { title: "Orchard Music Player", subtitle: "", active: true };
    frame.draw(f, l.player, ctx);

    // Cover art, aspect kept, centred in its cells.
    if let (Some(r), Some(now)) = (l.cover, &app.now)
        && let Some(cover) = now.details.as_ref().and_then(|d| d.cover.as_ref()) {
            let area = widgets::px(r, ctx);
            let s = (area.w / cover.width as f32).min(area.h / cover.height as f32);
            let (w, h) = ((cover.width as f32 * s).floor(), (cover.height as f32 * s).floor());
            let dest = PxRect::new(area.x + ((area.w - w) / 2.0).floor(), area.y + ((area.h - h) / 2.0).floor(), w, h);
            f.add_image(ImagePlacement::new(cover_image(now.cover_id), dest));
        }

    // Title line: state, title, time.
    let info = l.info;
    let (glyph, glyph_color) = match state {
        State::Playing => ("▶", c.success),
        State::Paused => ("❚❚", c.warning),
        State::Stopped => ("■", c.muted),
    };
    let surface = c.surface;
    let pos = app.position();
    let time = match (app.now.is_some() && state != State::Stopped, app.duration()) {
        (true, Some(d)) => format!("{} / {}", format_time(pos), format_time(d)),
        (true, None) => format_time(pos),
        (false, Some(d)) => format!("– / {}", format_time(d)),
        (false, None) => String::new(),
    };
    let time_w = width(&time);
    put(f, info.col, info.row, glyph, Style::new(glyph_color, surface), 2);
    let x = info.col + 3;
    let title_room = info.cols.saturating_sub(time_w + 4);
    match &app.now {
        Some(now) => {
            let title = now.tags.title.clone().unwrap_or_else(|| crate::meta::file_stem(&now.path));
            put(f, x, info.row, &elide_right(&title, title_room as usize), Style::new(c.foreground, surface).bold(), title_room);
            let mut sub: Vec<String> = Vec::new();
            sub.extend(now.tags.artist.clone());
            sub.extend(now.tags.album.clone());
            sub.extend(now.tags.year.clone());
            let tech = tech_line(now, app.gain_mode);
            let tech_w = width(&tech);
            let sub_room = info.cols.saturating_sub(2 + if tech_w + 20 < info.cols { tech_w + 2 } else { 0 });
            put(f, x, info.row + 1, &elide_right(&sub.join(" · "), sub_room as usize), Style::new(c.accent, surface), sub_room);
            if tech_w + 20 < info.cols {
                put(f, info.col + info.cols - tech_w, info.row + 1, &tech, Style::new(c.muted, surface), tech_w);
            }
        }
        None => {
            put(f, x, info.row, "Nothing playing", Style::new(c.muted, surface), title_room);
        }
    }
    put(f, info.col + info.cols - time_w, info.row, &time, Style::new(c.foreground, surface).bold(), time_w);

    draw_vis(app, f, l.vis, ctx);
    draw_seek(app, f, l.seek, ctx);
    draw_controls(app, f, l, ctx);
}

/// "MP3 · 320 kbps · 44.1 kHz · stereo".
fn tech_line(now: &crate::app::NowPlaying, gain_mode: crate::meta::GainMode) -> String {
    let mut parts: Vec<String> = Vec::new();
    let ext = now.path.extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default();
    parts.push(if now.codec.is_empty() { ext } else { now.codec.clone() });
    let d = now.details.as_ref();
    if let Some(kbps) = d.and_then(|d| d.bitrate).or(now.tags.bitrate) {
        parts.push(format!("{kbps} kbps"));
    }
    if let Some(rate) = d.and_then(|d| d.sample_rate) {
        let khz = rate as f64 / 1000.0;
        parts.push(if khz.fract() == 0.0 { format!("{khz:.0} kHz") } else { format!("{khz:.1} kHz") });
    }
    if let Some(bits) = d.and_then(|d| d.bit_depth) {
        parts.push(format!("{bits} bit"));
    }
    if let Some(db) = now.gain.and_then(|g| g.db(gain_mode)) {
        parts.push(format!("RG {db:+.1} dB"));
    }
    match d.and_then(|d| d.channels) {
        Some(1) => parts.push("mono".into()),
        Some(2) => parts.push("stereo".into()),
        Some(n) => parts.push(format!("{n} ch")),
        None => {}
    }
    parts.retain(|p| !p.is_empty());
    parts.join(" · ")
}

fn draw_vis(app: &App, f: &mut Frame, r: CellRect, ctx: &DrawCtx) {
    let c = &app.theme.colors;
    if r.cols == 0 || r.rows == 0 {
        return;
    }
    let area = widgets::px(r, ctx);
    let idle = app.now.is_none() && !app.analyzer.is_animating();
    if idle || app.vis == spectrum::Mode::Off {
        let (line1, line2) = if idle {
            ("♫  Drop music here or press F2 to open files", "")
        } else {
            ("Visualizer off", "Press A or click to turn it on")
        };
        let mid = r.row + r.rows / 2;
        let row1 = if line2.is_empty() { mid } else { mid.saturating_sub(1).max(r.row) };
        let x = r.col + r.cols.saturating_sub(width(line1)) / 2;
        put(f, x, row1, line1, Style::new(c.muted, c.surface), r.cols);
        if !line2.is_empty() && row1 + 1 < r.row + r.rows {
            let x = r.col + r.cols.saturating_sub(width(line2)) / 2;
            put(f, x, row1 + 1, line2, Style::new(mix(c.muted, c.surface, 0.4), c.surface), r.cols);
        }
        return;
    }
    match app.vis {
        spectrum::Mode::Bars => draw_bars(app, f, area, ctx),
        spectrum::Mode::Scope => draw_scope(app, f, area),
        spectrum::Mode::Off => {}
    }
}

/// LED-segment bars, green to red as they rise, with falling peak caps.
fn draw_bars(app: &App, f: &mut Frame, area: PxRect, ctx: &DrawCtx) {
    let c = &app.theme.colors;
    let (bar, gap) = bar_metrics(ctx.cell_px);
    let n = app.analyzer.bars.len();
    if n == 0 {
        return;
    }
    let used = n as f32 * bar + (n - 1) as f32 * gap;
    let x0 = (area.x + (area.w - used).max(0.0) / 2.0).floor();
    let seg = (ctx.cell_px.1 / 4.0).round().max(3.0);
    let seg_gap = (seg / 3.0).round().max(1.0);
    let pitch = seg + seg_gap;
    let segs = ((area.h + seg_gap) / pitch).floor().max(1.0) as usize;
    let bottom = area.y + area.h;
    let track = mix(c.surface, c.border, 0.35);
    let low = c.success;
    let midc = c.warning;
    let high = c.error;
    for (i, (&v, &peak)) in app.analyzer.bars.iter().zip(&app.analyzer.peaks).enumerate() {
        let x = x0 + i as f32 * (bar + gap);
        let lit = (v * segs as f32).round() as usize;
        for s in 0..segs {
            let y = bottom - (s + 1) as f32 * pitch + seg_gap;
            let t = s as f32 / (segs.max(2) - 1) as f32;
            let color = if s < lit {
                if t < 0.6 { mix(low, midc, t / 0.6) } else { mix(midc, high, (t - 0.6) / 0.4) }
            } else {
                track
            };
            f.add_rect(PxRect::new(x, y, bar, seg), color, Layer::BelowText);
        }
        if peak > 0.02 {
            let s = ((peak * segs as f32).round() as usize).clamp(1, segs);
            let y = bottom - s as f32 * pitch + seg_gap;
            f.add_rect(PxRect::new(x, y, bar, (seg / 2.0).max(2.0)), c.foreground, Layer::BelowText);
        }
    }
}

/// The waveform of the last few milliseconds.
fn draw_scope(app: &App, f: &mut Frame, area: PxRect) {
    let c = &app.theme.colors;
    let mid = area.y + area.h / 2.0;
    f.add_rect(PxRect::new(area.x, mid.floor(), area.w, 1.0), mix(c.surface, c.border, 0.6), Layer::BelowText);
    if app.state() != State::Playing && app.fake_position.is_none() {
        return;
    }
    let step = 2.0f32;
    let n = (area.w / step) as usize;
    let samples = app.player.scope(n.max(2) * 2);
    let amp = area.h / 2.0 - 1.0;
    let mut prev = mid;
    for i in 0..n {
        let v = samples.get(i * 2).copied().unwrap_or(0.0).clamp(-1.0, 1.0);
        let y = mid - v * amp;
        let (top, bot) = (prev.min(y), prev.max(y));
        let t = (v.abs() * 1.6).min(1.0);
        let color = mix(c.success, c.accent, t);
        f.add_rect(PxRect::new(area.x + i as f32 * step, top, step, (bot - top).max(2.0)), color, Layer::BelowText);
        prev = y;
    }
}

/// A slim pixel bar: track, filled part, knob.
fn pixel_bar(f: &mut Frame, r: CellRect, frac: f32, fg: Rgba, ctx: &DrawCtx, knob: bool) {
    let c = &ctx.theme.colors;
    let area = widgets::px(r, ctx);
    let th = (ctx.cell_px.1 / 5.0).round().max(2.0);
    let y = (area.y + (area.h - th) / 2.0).floor();
    let filled = (area.w * frac.clamp(0.0, 1.0)).round();
    f.add_rect(PxRect::new(area.x, y, area.w, th), mix(c.surface, c.border, 0.8), Layer::BelowText);
    f.add_rect(PxRect::new(area.x, y, filled, th), fg, Layer::BelowText);
    if knob {
        let kw = (ctx.cell_px.0 * 0.6).round().max(3.0);
        let kh = (ctx.cell_px.1 * 0.6).round();
        let kx = (area.x + filled - kw / 2.0).clamp(area.x, area.x + area.w - kw);
        f.add_rect(PxRect::new(kx, (area.y + (area.h - kh) / 2.0).floor(), kw, kh), c.foreground, Layer::AboveText);
    }
}

fn draw_seek(app: &App, f: &mut Frame, r: CellRect, ctx: &DrawCtx) {
    let frac = match (app.state(), app.duration()) {
        (State::Stopped, _) if app.fake_position.is_none() => 0.0,
        (_, Some(d)) if d > 0.0 => (app.position() / d) as f32,
        _ => 0.0,
    };
    pixel_bar(f, r, frac, app.theme.colors.accent, ctx, app.now.is_some());
}

fn draw_controls(app: &App, f: &mut Frame, l: &Layout, ctx: &DrawCtx) {
    let c = &app.theme.colors;
    let state = app.state();
    for (r, b, label) in buttons(app, l) {
        let on = match b {
            Button::Play => state == State::Playing,
            Button::Pause => state == State::Paused,
            Button::Stop => state == State::Stopped,
            Button::Shuffle => app.shuffle,
            Button::Repeat => app.repeat != Repeat::Off,
            _ => false,
        };
        let style = match (b, on) {
            (Button::Shuffle | Button::Repeat, true) => Style::new(c.accent, c.surface),
            (Button::Shuffle | Button::Repeat, false) => Style::new(c.muted, c.surface),
            (_, true) => Style::new(c.cursor_fg, c.cursor_bg),
            (_, false) => Style::new(c.foreground, mix(c.surface, c.border, 0.5)),
        };
        put(f, r.col, r.row, &label, style, r.cols);
    }
    let vol = app.player.volume();
    let label = format!("vol {:>3}%", (vol * 100.0).round() as u32);
    let lw = width(&label);
    put(f, l.volume.col.saturating_sub(lw + 1), l.volume.row, &label, Style::new(c.muted, c.surface), lw);
    pixel_bar(f, l.volume, vol, c.success, ctx, true);
}

// ---------------------------------------------------------------------------
// The playlist
// ---------------------------------------------------------------------------

fn draw_playlist(app: &App, f: &mut Frame, l: &Layout, ctx: &DrawCtx) {
    let c = &app.theme.colors;
    let pl = &app.playlist;
    let title = match &pl.file {
        Some(p) => format!("Playlist · {}", crate::meta::file_stem(p)),
        None => "Playlist".to_string(),
    };
    let (total, all_known) = pl.total_duration();
    let subtitle = if pl.is_empty() {
        String::new()
    } else if app.filter.is_some() {
        format!("{} of {} tracks", app.view_len(), pl.len())
    } else {
        format!(
            "{} track{} · {}{}",
            pl.len(),
            if pl.len() == 1 { "" } else { "s" },
            format_time(total),
            if all_known { "" } else { "+" }
        )
    };
    BoxFrame { title: &title, subtitle: &subtitle, active: false }.draw(f, l.playlist, ctx);
    let list = l.list;
    if pl.is_empty() {
        let hints = ["The playlist is empty", "", "F2  open files or folders", "F5  load an M3U playlist", "or drop files onto the window"];
        let top = list.row + list.rows.saturating_sub(hints.len() as u16) / 2;
        for (i, h) in hints.iter().enumerate() {
            let x = list.col + list.cols.saturating_sub(width(h)) / 2;
            put(f, x, top + i as u16, h, Style::new(c.muted, c.surface), list.cols);
        }
        return;
    }
    if let (Some(fr), Some(flt)) = (l.filter, &app.filter) {
        draw_filter(app, flt, f, fr, ctx);
    }
    if app.view_len() == 0 {
        let msg = "No matching tracks — Esc clears the filter";
        let x = list.col + list.cols.saturating_sub(width(msg)) / 2;
        put(f, x, list.row + list.rows / 2, msg, Style::new(c.muted, c.surface), list.cols);
        return;
    }
    let num_w = pl.len().to_string().len() as u16;
    for i in 0..list.rows as usize {
        let Some(idx) = app.view_entry(app.list_top + i) else { break };
        let e = &pl.entries[idx];
        let row = list.row + i as u16;
        let is_cursor = idx == pl.cursor;
        let is_current = pl.current == Some(idx);
        let bg = if is_cursor { c.cursor_bg } else { c.surface };
        let fg = if is_cursor {
            c.cursor_fg
        } else if is_current {
            c.accent
        } else {
            c.foreground
        };
        let row_rect = CellRect::new(list.col, row, list.cols, 1);
        f.fill(row_rect, ' ', Style::new(fg, bg));
        let marker = if is_current {
            match app.state() {
                State::Playing => "▶",
                State::Paused => "‖",
                State::Stopped => "·",
            }
        } else {
            " "
        };
        let num = format!("{:>w$}.", idx + 1, w = num_w as usize);
        let mut x = list.col + 1;
        let marker_fg = if is_cursor { c.cursor_fg } else { c.success };
        x = put(f, x, row, marker, Style::new(marker_fg, bg), 1) + 1;
        let num_fg = if is_cursor { c.cursor_fg } else { c.muted };
        x = put(f, x, row, &num, Style::new(num_fg, bg), num_w + 1) + 1;
        let dur = e.duration().map(format_time).unwrap_or_default();
        let dur_w = width(&dur);
        let end = list.col + list.cols.saturating_sub(2);
        let room = end.saturating_sub(x + dur_w + 1);
        let mut style = Style::new(fg, bg);
        if is_current {
            style = style.with(Flags::BOLD);
        }
        put(f, x, row, &elide_right(&e.display(), room as usize), style, room);
        put(f, end - dur_w, row, &dur, Style::new(if is_cursor { c.cursor_fg } else { c.muted }, bg), dur_w);
    }
    Scrollbar { total: app.view_len(), visible: list.rows as usize, top: app.list_top }.draw(f, list, ctx);
}

/// "/ query▏" at the bottom of the playlist; the field takes keys while editing.
fn draw_filter(app: &App, flt: &crate::app::Filter, f: &mut Frame, r: CellRect, ctx: &DrawCtx) {
    let c = &app.theme.colors;
    let style = Style::new(c.accent, c.surface).bold();
    let x = put(f, r.col + 1, r.row, "/", style, 1) + 1;
    let field = CellRect::new(x, r.row, r.cols.saturating_sub(x - r.col + 1), 1);
    if flt.editing {
        flt.field.draw(f, field, ctx);
    } else {
        let hint = if flt.field.text.is_empty() { "(all)" } else { &flt.field.text };
        put(f, field.col, r.row, hint, Style::new(c.foreground, c.surface), field.cols);
        let tip = "/ edit · Esc clear";
        let w = width(tip);
        if field.cols > w + width(hint) + 4 {
            put(f, r.col + r.cols - w - 1, r.row, tip, Style::new(c.muted, c.surface), w);
        }
    }
}

// ---------------------------------------------------------------------------
// Dialogs
// ---------------------------------------------------------------------------

fn centered(app: &App, w: u16, h: u16) -> CellRect {
    let w = w.min(app.cols.saturating_sub(4)).max(10);
    let h = h.min(app.rows.saturating_sub(2)).max(5);
    CellRect::new((app.cols.saturating_sub(w)) / 2, (app.rows.saturating_sub(h)) / 2, w, h)
}

fn modal_rect(app: &App, m: &Modal) -> CellRect {
    match m {
        Modal::Prompt(_) => centered(app, 84, 7),
        Modal::Themes(t) => centered(app, 50, t.schemes.len() as u16 + 4),
        Modal::Help => centered(app, 76, HELP.len() as u16 + 4),
        Modal::Settings(_) | Modal::Picker(..) => CellRect::new(0, 0, 0, 0),
    }
}

/// The rows of the open dialog's list (for paging and clicks).
pub fn dialog_list_rect(app: &App) -> Option<CellRect> {
    let m = app.modal.as_ref()?;
    let inner = inset(modal_rect(app, m), 1, 1);
    match m {
        Modal::Themes(_) => Some(CellRect::new(inner.col + 1, inner.row + 1, inner.cols.saturating_sub(2), inner.rows.saturating_sub(2))),
        _ => None,
    }
}

pub fn dialog_list_height(app: &App) -> usize {
    dialog_list_rect(app).map_or(10, |r| r.rows as usize)
}

fn draw_modal(app: &App, m: &Modal, f: &mut Frame, ctx: &DrawCtx) {
    let c = &app.theme.colors;
    match m {
        Modal::Settings(d) => return d.draw(app, f, ctx),
        // The shared picker draws its own frame, cover and shadow.
        Modal::Picker(p, _) => return p.draw(f, app.screen(), ctx),
        _ => {}
    }
    let r = modal_rect(app, m);
    // Over what is under it, with a soft shadow along the right and bottom edges.
    settings::form::cover(f, r, ctx);
    match m {
        Modal::Prompt(p) => {
            let inner = BoxFrame { title: &p.title, subtitle: "", active: true }.draw(f, r, ctx);
            put(f, inner.col + 1, inner.row + 1, &p.label, Style::new(c.foreground, c.surface), inner.cols.saturating_sub(2));
            p.field.draw(f, CellRect::new(inner.col + 1, inner.row + 2, inner.cols.saturating_sub(2), 1), ctx);
            put(f, inner.col + 1, inner.row + 4, "Enter save · Esc cancel", Style::new(c.muted, c.surface), inner.cols.saturating_sub(2));
        }
        Modal::Themes(t) => {
            let inner = BoxFrame { title: "Color scheme", subtitle: "", active: true }.draw(f, r, ctx);
            let list = dialog_list_rect(app).unwrap_or(inner);
            for i in 0..list.rows as usize {
                let idx = t.top + i;
                let Some(s) = t.schemes.get(idx) else { break };
                let row = list.row + i as u16;
                let is_cursor = idx == t.cursor;
                let bg = if is_cursor { c.cursor_bg } else { c.surface };
                let fg = if is_cursor { c.cursor_fg } else { c.foreground };
                f.fill(CellRect::new(list.col, row, list.cols, 1), ' ', Style::new(fg, bg));
                let sw_w = 16u16;
                put(f, list.col + 1, row, &s.name, Style::new(fg, bg), list.cols.saturating_sub(sw_w + 3));
                // Swatches: the scheme's own colors.
                let sc = orchard_theme::Colors::from_scheme(s);
                let swatches = [sc.background, sc.surface, sc.foreground, sc.accent, sc.border_active, sc.success, sc.warning, sc.error];
                let x0 = list.col + list.cols.saturating_sub(sw_w + 1);
                for (k, col) in swatches.iter().enumerate() {
                    let x = x0 + k as u16 * 2;
                    if x + 1 < list.col + list.cols {
                        f.put_str(x, row, "██", Style::new(*col, bg));
                    }
                }
            }
            put(f, inner.col + 1, inner.row + inner.rows - 1, "↑↓ preview · Enter keep · Esc revert", Style::new(c.muted, c.surface), inner.cols.saturating_sub(2));
        }
        Modal::Settings(_) | Modal::Picker(..) => {}
        Modal::Help => {
            let inner = BoxFrame { title: "Keys", subtitle: "", active: true }.draw(f, r, ctx);
            for (i, (k, d)) in HELP.iter().enumerate() {
                let row = inner.row + 1 + i as u16;
                if row >= inner.row + inner.rows {
                    break;
                }
                if k.is_empty() {
                    continue;
                }
                put(f, inner.col + 2, row, k, Style::new(c.accent, c.surface).bold(), 22);
                put(f, inner.col + 26, row, d, Style::new(c.foreground, c.surface), inner.cols.saturating_sub(27));
            }
        }
    }
}

const HELP: &[(&str, &str)] = &[
    ("Space", "Play / pause"),
    ("Enter / double-click", "Play the selected track"),
    ("Z X C V B", "Previous · play · pause · stop · next"),
    ("← →  (⇧: 30 s)", "Seek 5 seconds"),
    ("+ −  / wheel on player", "Volume"),
    ("↑ ↓ PgUp PgDn Home End", "Move the selection"),
    ("⌥↑ ⌥↓", "Move the selected track"),
    ("Del  /  ⇧Del", "Remove track  /  clear playlist"),
    ("J", "Jump to the playing track"),
    ("/  or  ⌘F", "Filter the playlist (Esc clears)"),
    ("G", "ReplayGain track · album · off"),
    ("S  /  R", "Shuffle  /  repeat off · all · one"),
    ("A  / click visualizer", "Bars · oscilloscope · off"),
    ("T", "Color scheme, quick pick"),
    ("⌘,  /  F9", "Settings: colors, font, playback"),
    ("⌘O / ⌘L / ⌘S", "Open · load playlist · save playlist"),
    ("⌘+ ⌘− ⌘0", "Font size"),
    ("Media keys", "Play/pause · next · previous"),
    ("F1 … F10", "As on the key bar"),
];

fn draw_toast(app: &App, f: &mut Frame, l: &Layout, ctx: &DrawCtx) {
    let Some(t) = &app.toast else { return };
    let c = &ctx.theme.colors;
    let text = format!(" {} ", elide_right(&t.text, l.footer.cols.saturating_sub(6) as usize));
    let w = width(&text);
    let row = l.footer.row.saturating_sub(1);
    let x = l.footer.cols.saturating_sub(w + 2);
    let style = if t.error { Style::new(c.surface, c.error).bold() } else { Style::new(c.cursor_fg, c.cursor_bg) };
    fill(f, CellRect::new(x, row, w, 1), style.bg);
    put(f, x, row, &text, style, w);
}
