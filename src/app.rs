//! All of the player's state and behavior. The binary owns only the window
//! and the GPU; it feeds events in and draws `ui::draw(&app, …)`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use orchard_kit::input::{Key, KeyEvent, MouseEvent, MouseKind, Named};
use orchard_kit::tasks::{Tasks, Waker};
use orchard_kit::widgets::TextField;
use orchard_theme::Theme;
use orchard_theme::schemes::{self, ColorScheme};

use crate::audio::{Notice, Player, State, TrackInfo};
use crate::browser::{self, Browser};
use crate::meta::{self, Details, GainMode, ReplayGain, Tags};
use crate::playlist::{self, Playlist, Repeat};
use crate::settings::{self, Settings};
use crate::spectrum::{self, Analyzer};
use crate::ui;

/// The track that plays (or is paused).
#[derive(Clone, Debug, Default)]
pub struct NowPlaying {
    pub path: PathBuf,
    pub tags: Tags,
    pub details: Option<Details>,
    pub duration: Option<f64>,
    pub codec: String,
    /// Bumped when the cover changes, so the window uploads it again.
    pub cover_id: u64,
    pub gain: Option<ReplayGain>,
}

/// The playlist's quick filter (/): only entries matching every word show.
#[derive(Clone, Debug)]
pub struct Filter {
    pub field: TextField,
    /// Keys go to the field.
    pub editing: bool,
    /// Matching entry indices, in playlist order.
    pub matches: Vec<usize>,
}

impl Default for Filter {
    fn default() -> Self {
        Filter { field: TextField::default().flush(), editing: false, matches: Vec::new() }
    }
}

pub struct Prompt {
    pub title: String,
    pub label: String,
    pub field: TextField,
    pub action: PromptAction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptAction {
    SavePlaylist,
}

pub struct ThemePicker {
    pub schemes: Vec<ColorScheme>,
    pub cursor: usize,
    pub top: usize,
    /// Restored on Esc.
    pub original: String,
}

pub enum Modal {
    Browser(Browser),
    Prompt(Prompt),
    Themes(ThemePicker),
    Settings(Box<ui::settings::SettingsDialog>),
    Help,
}

pub struct Toast {
    pub text: String,
    pub error: bool,
    pub until: Instant,
}

/// Which bar the left button is dragging.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drag {
    Seek,
    Volume,
}

pub struct App {
    pub settings: Settings,
    config_dir: PathBuf,
    /// False in screenshot mode and tests: nothing is written.
    persist: bool,
    pub theme: Theme,
    /// Every color scheme (built-ins, then the shared folder's), as last read.
    pub schemes: Vec<ColorScheme>,
    /// The color-schemes folder shared with Orchard Commander (tests: a temp dir).
    pub schemes_dir: PathBuf,
    /// The system is in dark mode (for following it).
    pub system_dark: bool,
    /// Installed fonts, for the settings (looked up when they open).
    pub fonts: ui::settings::FontInfo,
    pub player: Player,
    pub playlist: Playlist,
    pub now: Option<NowPlaying>,
    pub analyzer: Analyzer,
    pub vis: spectrum::Mode,
    pub shuffle: bool,
    pub repeat: Repeat,
    pub gain_mode: GainMode,
    pub filter: Option<Filter>,
    pub modal: Option<Modal>,
    pub toast: Option<Toast>,
    pub tasks: Tasks<App>,
    /// First playlist row shown.
    pub list_top: usize,
    /// Grid size and cell size in pixels, from the window.
    pub cols: u16,
    pub rows: u16,
    pub cell_px: (f32, f32),
    pub quit: bool,
    /// Shown instead of the player's clock (screenshots).
    pub fake_position: Option<f64>,
    pub fake_state: Option<State>,
    cover_counter: u64,
    drag: Option<Drag>,
    /// Tracks that failed in a row (stops skipping through a dead playlist).
    failures: usize,
    /// The entry handed to the player to follow the current one gaplessly.
    queued: Option<(usize, PathBuf)>,
}

/// Queue the next track this long before the current one ends.
const QUEUE_AHEAD: f64 = 10.0;

impl App {
    /// `persist`: read and write settings and the session playlist; `audio`:
    /// open the output device.
    pub fn new(wake: Waker, persist: bool, audio: bool) -> App {
        let config_dir = settings::config_dir();
        let settings = if persist { Settings::load(&config_dir) } else { Settings::default() };
        let schemes_dir = orchard_theme::dirs::schemes_dir();
        let schemes = schemes::all_schemes(&schemes_dir);
        let system_dark = true;
        let theme = Theme::new(&settings.appearance(), scheme_in(&schemes, settings.appearance().scheme_name(system_dark)));
        let tasks = Tasks::new(4, wake.clone());
        let player_wake = wake.clone();
        let player = Player::new(move || player_wake(), audio);
        player.set_volume(settings.volume);
        let gain_mode = GainMode::from_label(&settings.replay_gain);
        player.set_gain_mode(gain_mode);
        let mut app = App {
            gain_mode,
            filter: None,
            queued: None,
            vis: spectrum::Mode::from_label(&settings.visualizer),
            shuffle: settings.shuffle,
            repeat: Repeat::from_label(&settings.repeat),
            settings,
            config_dir,
            persist,
            theme,
            schemes,
            schemes_dir,
            system_dark,
            fonts: Default::default(),
            player,
            playlist: Playlist::default(),
            now: None,
            analyzer: Analyzer::new(),
            modal: None,
            toast: None,
            tasks,
            list_top: 0,
            cols: 100,
            rows: 36,
            cell_px: (8.0, 16.0),
            quit: false,
            fake_position: None,
            fake_state: None,
            cover_counter: 0,
            drag: None,
            failures: 0,
        };
        if let Some(e) = app.player.output_error().map(str::to_string) {
            app.toast(format!("No sound: {e} — music plays as soon as a device is ready"), true);
        }
        if persist {
            app.restore_session();
        }
        app
    }

    // -- session ---------------------------------------------------------

    fn restore_session(&mut self) {
        let file = settings::session_file(&self.config_dir);
        let current = self.settings.current;
        self.tasks.spawn(
            move || playlist::load_m3u(&file).unwrap_or_default(),
            move |app, entries| {
                if !app.playlist.is_empty() {
                    return; // files from the command line came first
                }
                let paths: Vec<PathBuf> = entries.iter().map(|e| e.path.clone()).collect();
                app.playlist.entries = entries;
                if let Some(c) = current.filter(|&c| c < app.playlist.len()) {
                    app.playlist.cursor = c;
                    app.playlist.current = Some(c);
                }
                app.playlist_changed();
                app.scroll_to_cursor();
                app.request_tags(paths);
            },
        );
    }

    /// Write settings and the session playlist (on quit).
    pub fn save_session(&mut self) {
        if !self.persist {
            return;
        }
        self.save_settings();
        if let Err(e) = playlist::save_m3u(&self.playlist.entries, &settings::session_file(&self.config_dir)) {
            log::warn!("saving the session playlist: {e}");
        }
    }

    /// Write the settings, with the player's current volume, modes and track.
    pub fn save_settings(&mut self) {
        if !self.persist {
            return;
        }
        self.settings.volume = self.player.volume();
        self.settings.shuffle = self.shuffle;
        self.settings.repeat = self.repeat.label().into();
        self.settings.visualizer = self.vis.label().into();
        self.settings.replay_gain = self.gain_mode.label().into();
        self.settings.current = self.playlist.current;
        if let Err(e) = self.settings.save(&self.config_dir) {
            log::warn!("saving settings: {e}");
        }
    }

    // -- settings ----------------------------------------------------------

    /// Tests: write the settings into `dir`.
    #[cfg(test)]
    pub(crate) fn persist_in(&mut self, dir: &Path) {
        self.config_dir = dir.to_path_buf();
        self.persist = true;
    }

    /// Change settings: the player follows at once and they are saved.
    pub fn update_settings(&mut self, change: impl FnOnce(&mut Settings)) {
        change(&mut self.settings);
        self.apply_settings();
        self.save_settings();
    }

    /// Make the player look and sound as the settings say (the look, the
    /// ReplayGain mode); also ends a scheme preview.
    pub fn apply_settings(&mut self) {
        let a = self.settings.appearance();
        self.theme = Theme::new(&a, self.scheme(a.scheme_name(self.system_dark)));
        let gain = GainMode::from_label(&self.settings.replay_gain);
        if gain != self.gain_mode {
            self.gain_mode = gain;
            self.player.set_gain_mode(gain);
        }
    }

    /// The color scheme with this name (as last read), or the default one.
    pub fn scheme(&self, name: &str) -> ColorScheme {
        scheme_in(&self.schemes, name)
    }

    /// Read the color-schemes folder again (Orchard Commander may have saved one).
    pub fn refresh_schemes(&mut self) {
        self.schemes = schemes::all_schemes(&self.schemes_dir);
    }

    /// Show `scheme` on the whole player without choosing it (the editor's preview).
    pub fn preview_scheme(&mut self, scheme: ColorScheme) {
        self.theme = Theme::new(&self.settings.appearance(), scheme);
    }

    /// The system switched between light and dark mode.
    pub fn set_system_dark(&mut self, dark: bool) {
        if dark != self.system_dark {
            self.system_dark = dark;
            let editing = matches!(&self.modal, Some(Modal::Settings(d)) if d.editor.is_some());
            if self.settings.follow_system_appearance && !editing {
                self.apply_settings();
            }
        }
    }

    pub fn open_settings(&mut self) {
        ui::settings::open(self);
    }

    pub fn set_gain_mode(&mut self, mode: GainMode) {
        self.update_settings(|s| s.replay_gain = mode.label().into());
    }

    // -- frame loop ------------------------------------------------------

    /// Run what workers and the player sent. Returns whether anything changed.
    pub fn process(&mut self) -> bool {
        let mut changed = false;
        for done in self.tasks.take() {
            done(self);
            changed = true;
        }
        for n in self.player.take_notices() {
            changed = true;
            match n {
                Notice::Finished => {
                    self.failures = 0;
                    match self.queued.take() {
                        // Queued too late to follow without a gap: start it now.
                        Some((i, path)) if self.playlist.entries.get(i).is_some_and(|e| e.path == path) => self.play_index(i),
                        _ => self.advance(false),
                    }
                }
                Notice::Opened(info) => self.apply_info(info),
                Notice::Advanced(info) => self.advanced(info),
                Notice::Error(path, e) => {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.toast(format!("Can't play {name}: {e}"), true);
                    if self.now.as_ref().is_some_and(|n| n.path == path) {
                        self.failures += 1;
                        if self.failures < self.playlist.len().max(1) {
                            self.advance(false);
                        } else {
                            self.failures = 0;
                            self.stop();
                        }
                    }
                }
            }
        }
        changed
    }

    /// What the decoder found out about the playing track.
    fn apply_info(&mut self, info: TrackInfo) {
        if let Some(now) = self.now.as_mut().filter(|n| n.path == info.path) {
            now.duration = info.duration.or(now.tags.duration);
            now.codec = info.codec;
            now.gain = Some(info.gain);
        }
    }

    /// The queued track took over gaplessly.
    fn advanced(&mut self, info: TrackInfo) {
        let index = match self.queued.take() {
            Some((i, p)) if p == info.path && self.playlist.entries.get(i).is_some_and(|e| e.path == p) => Some(i),
            _ => self.playlist.entries.iter().position(|e| e.path == info.path),
        };
        let Some(index) = index else { return };
        self.failures = 0;
        self.playlist.current = Some(index);
        self.playlist.mark_played(index);
        self.set_now(index);
        self.apply_info(info);
    }

    /// Hand the player the next track shortly before this one ends, so it
    /// follows without a gap.
    fn maybe_queue(&mut self) {
        if self.queued.is_some() || self.player.state() == State::Stopped || self.now.is_none() {
            return;
        }
        let Some(d) = self.duration() else { return };
        if d - self.player.position() > QUEUE_AHEAD {
            return;
        }
        if let Some(i) = self.playlist.next(self.shuffle, self.repeat, false) {
            let path = self.playlist.entries[i].path.clone();
            self.player.queue(Some(&path));
            self.queued = Some((i, path));
        }
    }

    /// Forget the queued track (the playlist or the play order changed).
    fn unqueue(&mut self) {
        if self.queued.take().is_some() {
            self.player.queue(None);
        }
    }

    /// After the entries changed: what plays next may differ, the filter too.
    fn playlist_changed(&mut self) {
        self.unqueue();
        self.refilter();
    }

    /// Animation step: the visualizer, the toast's timeout, the gapless queue.
    pub fn tick(&mut self, now: Instant) {
        if self.toast.as_ref().is_some_and(|t| now >= t.until) {
            self.toast = None;
        }
        if self.fake_position.is_some() {
            return;
        }
        self.maybe_queue();
        let count = ui::bar_count(self);
        let playing = self.player.state() == State::Playing;
        let samples = (playing && self.vis == spectrum::Mode::Bars).then(|| self.player.scope(spectrum::FFT_SIZE));
        self.analyzer.update(samples.as_deref(), self.player.rate(), count, now);
    }

    /// How soon the window wants the next frame: `None` = only on events.
    pub fn next_frame(&self, now: Instant) -> Option<Instant> {
        let toast = self.toast.as_ref().map(|t| t.until);
        let anim = if self.player.state() == State::Playing {
            Some(now + if self.vis == spectrum::Mode::Off { Duration::from_millis(250) } else { Duration::from_millis(16) })
        } else if self.analyzer.is_animating() {
            Some(now + Duration::from_millis(16))
        } else {
            None
        };
        [toast, anim].into_iter().flatten().min()
    }

    pub fn resized(&mut self, cols: u16, rows: u16, cell_px: (f32, f32)) {
        self.cols = cols;
        self.rows = rows;
        self.cell_px = cell_px;
        self.scroll_to_cursor();
    }

    /// The player's state (or the screenshot's).
    pub fn state(&self) -> State {
        self.fake_state.unwrap_or_else(|| self.player.state())
    }

    pub fn position(&self) -> f64 {
        self.fake_position.unwrap_or_else(|| self.player.position())
    }

    pub fn duration(&self) -> Option<f64> {
        self.now.as_ref().and_then(|n| n.duration.or(n.tags.duration))
    }

    pub fn toast(&mut self, text: impl Into<String>, error: bool) {
        let secs = if error { 6 } else { 3 };
        self.toast = Some(Toast { text: text.into(), error, until: Instant::now() + Duration::from_secs(secs) });
    }

    // -- playlist & metadata ---------------------------------------------

    /// Add files, folders (recursively) and playlists' contents; `play`
    /// starts the first new track.
    pub fn add_paths(&mut self, paths: Vec<PathBuf>, play: bool) {
        if paths.is_empty() {
            return;
        }
        self.tasks.spawn(
            move || browser::expand(&paths),
            move |app, entries| {
                if entries.is_empty() {
                    app.toast("No audio files found in what you added", true);
                    return;
                }
                let first = app.playlist.len();
                let n = entries.len();
                let paths: Vec<PathBuf> = entries.iter().map(|e| e.path.clone()).collect();
                app.playlist.entries.extend(entries);
                app.toast(format!("Added {n} track{}", if n == 1 { "" } else { "s" }), false);
                app.playlist_changed();
                if play || app.player.state() == State::Stopped && app.now.is_none() && first == 0 {
                    app.playlist.cursor = first;
                    if play {
                        app.play_index(first);
                    }
                }
                app.scroll_to_cursor();
                app.request_tags(paths);
            },
        );
    }

    /// Read tags in the background, in small batches so the visible rows fill in early.
    fn request_tags(&mut self, paths: Vec<PathBuf>) {
        for chunk in paths.chunks(24) {
            let chunk = chunk.to_vec();
            self.tasks.spawn(
                move || chunk.into_iter().map(|p| { let t = meta::read_tags(&p); (p, t) }).collect::<Vec<_>>(),
                |app, results| {
                    for (path, tags) in results {
                        for e in app.playlist.entries.iter_mut().filter(|e| e.path == path) {
                            e.tags = Some(tags.clone());
                        }
                        if let Some(now) = app.now.as_mut().filter(|n| n.path == path) {
                            if now.duration.is_none() {
                                now.duration = tags.duration;
                            }
                            now.tags = tags;
                        }
                    }
                    if app.filter.is_some() {
                        app.refilter();
                    }
                },
            );
        }
    }

    pub fn load_playlist(&mut self, path: PathBuf) {
        let p = path.clone();
        self.tasks.spawn(
            move || playlist::load_m3u(&p),
            move |app, result| match result {
                Ok(entries) => {
                    app.stop();
                    app.playlist.clear();
                    let n = entries.len();
                    let paths: Vec<PathBuf> = entries.iter().map(|e| e.path.clone()).collect();
                    app.playlist.entries = entries;
                    app.playlist.file = Some(path.clone());
                    app.list_top = 0;
                    app.playlist_changed();
                    app.settings.last_dir = path.parent().map(Path::to_path_buf);
                    app.toast(format!("Loaded {n} track{} from {}", if n == 1 { "" } else { "s" }, meta::file_stem(&path)), false);
                    app.request_tags(paths);
                }
                Err(e) => app.toast(format!("Can't load {}: {}", path.display(), meta::io_reason(&e)), true),
            },
        );
    }

    pub fn save_playlist(&mut self, path: PathBuf) {
        let entries = self.playlist.entries.clone();
        let p = path.clone();
        self.tasks.spawn(
            move || playlist::save_m3u(&entries, &p).map(|_| entries.len()),
            move |app, result| match result {
                Ok(n) => {
                    app.playlist.file = Some(path.clone());
                    app.toast(format!("Saved {n} track{} to {}", if n == 1 { "" } else { "s" }, path.display()), false);
                }
                Err(e) => app.toast(format!("Can't save {}: {}", path.display(), meta::io_reason(&e)), true),
            },
        );
    }

    // -- transport -------------------------------------------------------

    pub fn play_index(&mut self, index: usize) {
        let Some(entry) = self.playlist.entries.get(index) else { return };
        let path = entry.path.clone();
        self.queued = None; // the player drops it too
        self.playlist.current = Some(index);
        self.playlist.mark_played(index);
        self.player.play(&path, 0.0);
        self.set_now(index);
    }

    /// The now-playing panel for entry `index`; details and cover load in the background.
    fn set_now(&mut self, index: usize) {
        let Some(entry) = self.playlist.entries.get(index).cloned() else { return };
        let path = entry.path.clone();
        self.now = Some(NowPlaying {
            path: path.clone(),
            tags: entry.tags.clone().unwrap_or_else(|| Tags {
                title: entry.title.clone(),
                duration: entry.duration,
                ..Tags::default()
            }),
            duration: entry.duration(),
            ..NowPlaying::default()
        });
        self.tasks.spawn(
            {
                let path = path.clone();
                move || meta::read_details(&path)
            },
            move |app, details| {
                app.cover_counter += 1;
                let id = app.cover_counter;
                if let Some(now) = app.now.as_mut().filter(|n| n.path == path) {
                    now.details = Some(details);
                    now.cover_id = id;
                }
            },
        );
    }

    pub fn play_pause(&mut self) {
        match self.player.state() {
            State::Playing => self.player.pause(),
            State::Paused => self.player.resume(),
            State::Stopped => self.play(),
        }
    }

    /// Play: resume, or start the selected track.
    pub fn play(&mut self) {
        match self.player.state() {
            State::Paused => self.player.resume(),
            _ if !self.playlist.is_empty() => {
                let i = self.playlist.cursor.min(self.playlist.len() - 1);
                self.play_index(i);
            }
            _ => self.open_browser(browser::Mode::Add),
        }
    }

    pub fn pause(&mut self) {
        match self.player.state() {
            State::Playing => self.player.pause(),
            State::Paused => self.player.resume(),
            State::Stopped => {}
        }
    }

    pub fn stop(&mut self) {
        self.queued = None;
        self.player.stop();
    }

    /// The next track (`user`: the Next key, which skips even under repeat-one).
    pub fn advance(&mut self, user: bool) {
        match self.playlist.next(self.shuffle, self.repeat, user) {
            Some(i) => {
                // The selection follows a skip, not the playlist running on.
                if user {
                    self.playlist.cursor = i;
                    self.scroll_to_cursor();
                }
                self.play_index(i);
            }
            None => self.stop(),
        }
    }

    pub fn previous(&mut self) {
        if self.position() > 3.0 && self.player.state() != State::Stopped {
            self.player.seek(0.0);
            return;
        }
        if let Some(i) = self.playlist.previous(self.shuffle) {
            self.playlist.cursor = i;
            self.play_index(i);
            self.scroll_to_cursor();
        }
    }

    pub fn seek_by(&mut self, delta: f64) {
        let mut to = (self.position() + delta).max(0.0);
        if let Some(d) = self.duration() {
            to = to.min((d - 0.5).max(0.0));
        }
        self.player.seek(to);
    }

    pub fn seek_fraction(&mut self, frac: f64) {
        if let Some(d) = self.duration() {
            self.player.seek(frac.clamp(0.0, 1.0) * d);
        }
    }

    pub fn set_volume(&mut self, v: f32) {
        self.player.set_volume((v * 100.0).round() / 100.0);
    }

    pub fn volume_by(&mut self, delta: f32) {
        self.set_volume(self.player.volume() + delta);
    }

    // -- playlist editing ------------------------------------------------

    pub fn list_height(&self) -> usize {
        ui::layout(self).list.rows as usize
    }

    /// Rows the playlist shows: every entry, or the filter's matches.
    pub fn view_len(&self) -> usize {
        self.filter.as_ref().map_or(self.playlist.len(), |f| f.matches.len())
    }

    /// The entry shown at view row `row`.
    pub fn view_entry(&self, row: usize) -> Option<usize> {
        match &self.filter {
            Some(f) => f.matches.get(row).copied(),
            None => (row < self.playlist.len()).then_some(row),
        }
    }

    /// The view row of entry `index`, if shown.
    pub fn view_row(&self, index: usize) -> Option<usize> {
        match &self.filter {
            Some(f) => f.matches.binary_search(&index).ok(),
            None => (index < self.playlist.len()).then_some(index),
        }
    }

    pub fn scroll_to_cursor(&mut self) {
        let h = self.list_height().max(1);
        let c = self.view_row(self.playlist.cursor).unwrap_or(0);
        if c < self.list_top {
            self.list_top = c;
        } else if c >= self.list_top + h {
            self.list_top = c + 1 - h;
        }
        self.list_top = self.list_top.min(self.view_len().saturating_sub(h));
    }

    fn move_cursor(&mut self, delta: isize) {
        let n = self.view_len();
        if n == 0 {
            return;
        }
        let row = match self.view_row(self.playlist.cursor) {
            Some(r) => (r as isize + delta).clamp(0, n as isize - 1) as usize,
            None => 0,
        };
        if let Some(i) = self.view_entry(row) {
            self.playlist.cursor = i;
        }
        self.scroll_to_cursor();
    }

    /// The cursor's entry, if it is shown.
    fn selected(&self) -> Option<usize> {
        self.view_row(self.playlist.cursor).map(|_| self.playlist.cursor)
    }

    pub fn remove_selected(&mut self) {
        let Some(i) = self.selected() else { return };
        let row = self.view_row(i).unwrap_or(0);
        self.playlist.remove(i);
        self.playlist_changed();
        // Keep the selection on the row that moved up.
        let row = row.min(self.view_len().saturating_sub(1));
        if let Some(e) = self.view_entry(row) {
            self.playlist.cursor = e;
        }
        self.scroll_to_cursor();
    }

    pub fn clear_playlist(&mut self) {
        let n = self.playlist.len();
        self.stop();
        self.playlist.clear();
        self.now = None;
        self.list_top = 0;
        self.filter = None;
        self.toast(format!("Removed {n} track{}", if n == 1 { "" } else { "s" }), false);
    }

    // -- quick filter ----------------------------------------------------

    pub fn start_filter(&mut self) {
        self.filter.get_or_insert_with(Filter::default).editing = true;
        self.refilter();
    }

    pub fn clear_filter(&mut self) {
        self.filter = None;
        self.scroll_to_cursor();
    }

    /// Recompute the matches; the cursor moves onto one if it is hidden.
    pub fn refilter(&mut self) {
        let Some(f) = &mut self.filter else { return };
        let words: Vec<String> = f.field.text.to_lowercase().split_whitespace().map(str::to_string).collect();
        f.matches = self
            .playlist
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                if words.is_empty() {
                    return true;
                }
                let mut hay = e.display().to_lowercase();
                if let Some(t) = &e.tags {
                    for extra in [&t.album, &t.year, &t.genre].into_iter().flatten() {
                        hay.push(' ');
                        hay.push_str(&extra.to_lowercase());
                    }
                }
                hay.push(' ');
                hay.push_str(&e.path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default());
                words.iter().all(|w| hay.contains(w.as_str()))
            })
            .map(|(i, _)| i)
            .collect();
        if f.matches.binary_search(&self.playlist.cursor).is_err() {
            if let Some(&first) = f.matches.first() {
                self.playlist.cursor = first;
            }
            self.list_top = 0;
        }
        self.scroll_to_cursor();
    }

    /// Keys while typing in the filter; returns whether the key was taken.
    fn filter_key(&mut self, ev: &KeyEvent) -> bool {
        let Some(f) = &mut self.filter else { return false };
        if !f.editing {
            return false;
        }
        match &ev.key {
            Key::Named(Named::Escape) => self.clear_filter(),
            Key::Named(Named::Enter) => {
                f.editing = false;
                if let Some(i) = self.selected() {
                    self.play_index(i);
                }
            }
            Key::Named(Named::Tab) => f.editing = false,
            Key::Named(Named::Up | Named::Down | Named::PageUp | Named::PageDown | Named::F(_)) => return false,
            _ => {
                if f.field.handle(ev) {
                    self.refilter();
                } else {
                    return false;
                }
            }
        }
        true
    }

    pub fn cycle_gain(&mut self) {
        self.set_gain_mode(self.gain_mode.next());
        self.toast(format!("ReplayGain: {}", self.gain_mode.label()), false);
    }

    // -- dialogs ---------------------------------------------------------

    fn start_dir(&self) -> PathBuf {
        self.settings
            .last_dir
            .clone()
            .filter(|d| d.is_dir())
            .or_else(dirs::audio_dir)
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("/"))
    }

    pub fn open_browser(&mut self, mode: browser::Mode) {
        let dir = self.start_dir();
        self.modal = Some(Modal::Browser(Browser::new(mode, dir.clone())));
        self.list_dir(dir);
    }

    fn list_dir(&mut self, dir: PathBuf) {
        let Some(Modal::Browser(b)) = &mut self.modal else { return };
        b.going_to(dir.clone());
        let mode = b.mode;
        self.tasks.spawn(
            {
                let dir = dir.clone();
                move || browser::list(&dir, mode)
            },
            move |app, result| {
                if let Some(Modal::Browser(b)) = &mut app.modal
                    && b.dir == dir {
                        b.listed(result);
                        let h = ui::dialog_list_height(app);
                        if let Some(Modal::Browser(b)) = &mut app.modal {
                            b.scroll_into_view(h);
                        }
                    }
            },
        );
    }

    pub fn open_save_prompt(&mut self) {
        if self.playlist.is_empty() {
            self.toast("Can't save: the playlist is empty — add tracks with F2", true);
            return;
        }
        let path = self.playlist.file.clone().unwrap_or_else(|| self.start_dir().join("Playlist.m3u"));
        self.modal = Some(Modal::Prompt(Prompt {
            title: "Save playlist".into(),
            label: "Save as M3U:".into(),
            field: TextField::new(path.to_string_lossy()).flush(),
            action: PromptAction::SavePlaylist,
        }));
    }

    pub fn open_theme_picker(&mut self) {
        self.refresh_schemes();
        let schemes = self.schemes.clone();
        let cursor = schemes.iter().position(|s| s.name == self.theme.scheme.name).unwrap_or(0);
        self.modal = Some(Modal::Themes(ThemePicker {
            schemes,
            cursor,
            top: 0,
            original: self.theme.scheme.name.clone(),
        }));
    }

    /// Use `scheme` for the current mode (the scheme setting, or the light
    /// or dark one while following the system); saved when the picker closes.
    pub fn apply_scheme(&mut self, scheme: ColorScheme) {
        let a = self.settings.appearance();
        let slot = match a.scheme_setting(self.system_dark) {
            "light_scheme" => ui::settings::Slot::Light,
            "dark_scheme" => ui::settings::Slot::Dark,
            _ => ui::settings::Slot::Color,
        };
        slot.set(&mut self.settings, &scheme.name);
        self.theme = Theme::new(&self.settings.appearance(), scheme);
    }

    /// ⌘+ / ⌘- / ⌘0.
    pub fn zoom(&mut self, step: i64) {
        let (lo, hi) = schemes::FONT_SIZE_RANGE;
        let size = if step == 0 { settings::DEFAULT_FONT_SIZE } else { (self.settings.font_size + step).clamp(lo, hi) };
        self.update_settings(|s| s.font_size = size);
    }

    // -- input -----------------------------------------------------------

    pub fn handle_key(&mut self, ev: &KeyEvent) {
        if self.modal.is_some() {
            self.modal_key(ev);
            return;
        }
        if self.filter_key(ev) {
            return;
        }
        let m = ev.mods;
        if m.primary() {
            match &ev.key {
                Key::Char('f') => self.start_filter(),
                Key::Char('o') => self.open_browser(browser::Mode::Add),
                Key::Char('s') => self.open_save_prompt(),
                Key::Char('l') => self.open_browser(browser::Mode::LoadPlaylist),
                Key::Char('q') => self.quit = true,
                Key::Char(',') => self.open_settings(),
                Key::Char('=' | '+') => self.zoom(1),
                Key::Char('-') => self.zoom(-1),
                Key::Char('0') => self.zoom(0),
                Key::Named(Named::Up) => self.move_selected(-1),
                Key::Named(Named::Down) => self.move_selected(1),
                _ => {}
            }
            return;
        }
        let page = self.list_height().max(2) as isize - 1;
        match &ev.key {
            Key::Named(Named::F(n)) => self.fkey(*n),
            Key::Named(Named::Up) if m.alt => self.move_selected(-1),
            Key::Named(Named::Down) if m.alt => self.move_selected(1),
            Key::Named(Named::Up) => self.move_cursor(-1),
            Key::Named(Named::Down) => self.move_cursor(1),
            Key::Named(Named::PageUp) => self.move_cursor(-page),
            Key::Named(Named::PageDown) => self.move_cursor(page),
            Key::Named(Named::Home) => self.move_cursor(isize::MIN / 2),
            Key::Named(Named::End) => self.move_cursor(isize::MAX / 2),
            Key::Named(Named::Left) => self.seek_by(if m.shift { -30.0 } else { -5.0 }),
            Key::Named(Named::Right) => self.seek_by(if m.shift { 30.0 } else { 5.0 }),
            Key::Named(Named::Enter) => {
                if let Some(i) = self.selected() {
                    self.play_index(i);
                }
            }
            Key::Named(Named::Escape) if self.filter.is_some() => self.clear_filter(),
            Key::Named(Named::Delete) if m.shift => self.clear_playlist(),
            Key::Named(Named::Delete | Named::Backspace) => self.remove_selected(),
            Key::Named(Named::Insert) => self.open_browser(browser::Mode::Add),
            _ => match ev.typed() {
                Some(" ") => self.play_pause(),
                Some("z" | "Z") => self.previous(),
                Some("x" | "X") => self.play(),
                Some("c" | "C") => self.pause(),
                Some("v" | "V") => self.stop(),
                Some("b" | "B") => self.advance(true),
                Some("+" | "=") => self.volume_by(0.05),
                Some("-" | "_") => self.volume_by(-0.05),
                Some("s" | "S") => self.toggle_shuffle(),
                Some("r" | "R") => self.cycle_repeat(),
                Some("a" | "A") => self.cycle_vis(),
                Some("j" | "J") => self.jump_to_current(),
                Some("t" | "T") => self.open_theme_picker(),
                Some("g" | "G") => self.cycle_gain(),
                Some("/") => self.start_filter(),
                Some("?") => self.modal = Some(Modal::Help),
                _ => {}
            },
        }
    }

    fn move_selected(&mut self, delta: isize) {
        // Reordering a filtered view would be guesswork: clear the filter first.
        if self.playlist.is_empty() || self.filter.is_some() {
            return;
        }
        self.playlist.cursor = self.playlist.move_entry(self.playlist.cursor, delta);
        self.playlist_changed();
        self.scroll_to_cursor();
    }

    pub fn toggle_shuffle(&mut self) {
        self.unqueue();
        self.shuffle = !self.shuffle;
        self.toast(format!("Shuffle: {}", if self.shuffle { "on" } else { "off" }), false);
    }

    pub fn cycle_repeat(&mut self) {
        self.unqueue();
        self.repeat = self.repeat.next();
        self.toast(format!("Repeat: {}", self.repeat.label()), false);
    }

    pub fn cycle_vis(&mut self) {
        self.vis = self.vis.next();
    }

    fn jump_to_current(&mut self) {
        if let Some(c) = self.playlist.current {
            self.playlist.cursor = c;
            self.scroll_to_cursor();
        }
    }

    /// The F-key bar.
    pub fn fkey(&mut self, n: u8) {
        match n {
            1 => self.modal = Some(Modal::Help),
            2 => self.open_browser(browser::Mode::Add),
            3 => self.cycle_vis(),
            4 => self.open_save_prompt(),
            5 => self.open_browser(browser::Mode::LoadPlaylist),
            6 => self.toggle_shuffle(),
            7 => self.cycle_repeat(),
            8 => self.remove_selected(),
            9 => self.open_settings(),
            10 => self.quit = true,
            _ => {}
        }
    }

    fn modal_key(&mut self, ev: &KeyEvent) {
        let Some(modal) = self.modal.take() else { return };
        let list_h = ui::dialog_list_height(self);
        self.modal = match modal {
            Modal::Settings(mut d) => d.handle_key(self, ev).then_some(Modal::Settings(d)),
            Modal::Help => match &ev.key {
                Key::Named(Named::Escape | Named::Enter | Named::F(1) | Named::F(10)) => None,
                _ if ev.typed() == Some("?") || ev.typed() == Some("q") => None,
                _ => Some(Modal::Help),
            },
            Modal::Prompt(mut p) => match &ev.key {
                Key::Named(Named::Escape) => None,
                Key::Named(Named::Enter) => {
                    self.confirm_prompt(&p);
                    None
                }
                _ => {
                    p.field.handle(ev);
                    Some(Modal::Prompt(p))
                }
            },
            Modal::Themes(mut t) => {
                let n = t.schemes.len();
                let before = t.cursor;
                let close = match &ev.key {
                    Key::Named(Named::Escape) => {
                        let orig = self.scheme(&t.original);
                        self.apply_scheme(orig);
                        true
                    }
                    Key::Named(Named::Enter) => {
                        self.save_settings();
                        true
                    }
                    Key::Named(Named::Up) => {
                        t.cursor = t.cursor.saturating_sub(1);
                        false
                    }
                    Key::Named(Named::Down) => {
                        t.cursor = (t.cursor + 1).min(n.saturating_sub(1));
                        false
                    }
                    Key::Named(Named::Home) => {
                        t.cursor = 0;
                        false
                    }
                    Key::Named(Named::End) => {
                        t.cursor = n.saturating_sub(1);
                        false
                    }
                    _ => false,
                };
                if t.cursor != before {
                    // Live preview.
                    if let Some(s) = t.schemes.get(t.cursor).cloned() {
                        self.apply_scheme(s);
                    }
                }
                if t.cursor < t.top {
                    t.top = t.cursor;
                } else if t.cursor >= t.top + list_h.max(1) {
                    t.top = t.cursor + 1 - list_h.max(1);
                }
                (!close).then_some(Modal::Themes(t))
            }
            Modal::Browser(mut b) => {
                let page = list_h.max(2) as isize - 1;
                let action = match &ev.key {
                    Key::Named(Named::Escape | Named::F(10)) => browser::Action::Close,
                    Key::Named(Named::Up) => {
                        b.move_cursor(-1);
                        browser::Action::None
                    }
                    Key::Named(Named::Down) => {
                        b.move_cursor(1);
                        browser::Action::None
                    }
                    Key::Named(Named::PageUp) => {
                        b.move_cursor(-page);
                        browser::Action::None
                    }
                    Key::Named(Named::PageDown) => {
                        b.move_cursor(page);
                        browser::Action::None
                    }
                    Key::Named(Named::Home) => {
                        b.move_cursor(isize::MIN / 2);
                        browser::Action::None
                    }
                    Key::Named(Named::End) => {
                        b.move_cursor(isize::MAX / 2);
                        browser::Action::None
                    }
                    Key::Named(Named::Enter) if ev.mods.shift => b.add_folder(),
                    Key::Named(Named::Enter) => b.activate(),
                    Key::Named(Named::Right) => match b.item(b.cursor) {
                        Some(i) if i.is_dir => browser::Action::Open(i.path.clone()),
                        _ => browser::Action::None,
                    },
                    Key::Named(Named::Backspace | Named::Left) => browser::parent(&b.dir).map_or(browser::Action::None, browser::Action::Open),
                    Key::Named(Named::Insert) => {
                        b.toggle_mark();
                        browser::Action::None
                    }
                    Key::Named(Named::F(3)) => b.add_folder(),
                    _ => match ev.typed() {
                        Some(" ") => {
                            b.toggle_mark();
                            browser::Action::None
                        }
                        Some("*") => {
                            b.mark_all();
                            browser::Action::None
                        }
                        Some("~") => dirs::home_dir().map_or(browser::Action::None, browser::Action::Open),
                        Some(t) => {
                            b.search(t, Instant::now());
                            browser::Action::None
                        }
                        None => browser::Action::None,
                    },
                };
                b.scroll_into_view(list_h);
                self.browser_action(b, action)
            }
        };
    }

    fn browser_action(&mut self, b: Browser, action: browser::Action) -> Option<Modal> {
        match action {
            browser::Action::None => Some(Modal::Browser(b)),
            browser::Action::Close => None,
            browser::Action::Open(dir) => {
                self.modal = Some(Modal::Browser(b));
                self.list_dir(dir);
                self.modal.take()
            }
            browser::Action::Add(paths) => {
                self.settings.last_dir = Some(b.dir.clone());
                let play = self.player.state() == State::Stopped;
                self.add_paths(paths, play);
                None
            }
            browser::Action::Load(path) => {
                self.load_playlist(path);
                None
            }
        }
    }

    fn confirm_prompt(&mut self, p: &Prompt) {
        match p.action {
            PromptAction::SavePlaylist => {
                let text = p.field.text.trim();
                if text.is_empty() {
                    return;
                }
                let mut path = expand_tilde(text);
                if path.is_relative() {
                    path = self.start_dir().join(path);
                }
                if !meta::is_playlist(&path) {
                    path.set_extension("m3u");
                }
                self.settings.last_dir = path.parent().map(Path::to_path_buf);
                self.save_playlist(path);
            }
        }
    }

    pub fn handle_mouse(&mut self, ev: &MouseEvent) {
        if self.modal.is_some() {
            self.modal_mouse(ev);
            return;
        }
        let l = ui::layout(self);
        let hit = |r: orchard_grid::CellRect| orchard_kit::widgets::contains(r, ev.col, ev.row);
        let frac = |r: orchard_grid::CellRect, cw: f32| {
            ((ev.x - r.col as f32 * cw) / (r.cols as f32 * cw).max(1.0)).clamp(0.0, 1.0) as f64
        };
        let cw = self.cell_px.0;
        match ev.kind {
            MouseKind::Down => {
                if hit(l.seek) {
                    self.drag = Some(Drag::Seek);
                    self.seek_fraction(frac(l.seek, cw));
                } else if hit(l.volume) {
                    self.drag = Some(Drag::Volume);
                    self.set_volume(frac(l.volume, cw) as f32);
                } else if hit(l.vis) {
                    self.cycle_vis();
                } else if hit(l.list) {
                    let row = self.list_top + (ev.row - l.list.row) as usize;
                    if let Some(i) = self.view_entry(row) {
                        self.playlist.cursor = i;
                        if ev.double {
                            self.play_index(i);
                        }
                    }
                } else if hit(l.footer) {
                    if let Some((_, i)) = orchard_kit::widgets::key_bar_items(l.footer, ui::FKEYS)
                        .into_iter()
                        .find(|(r, _)| orchard_kit::widgets::contains(*r, ev.col, ev.row))
                    {
                        self.fkey(i as u8 + 1);
                    }
                } else if let Some(b) = ui::button_at(self, &l, ev.col, ev.row) {
                    self.button(b);
                }
            }
            MouseKind::Drag => match self.drag {
                Some(Drag::Seek) => self.seek_fraction(frac(l.seek, cw)),
                Some(Drag::Volume) => self.set_volume(frac(l.volume, cw) as f32),
                None => {}
            },
            MouseKind::Up => self.drag = None,
            MouseKind::Scroll(dy) => {
                if hit(l.list) {
                    let h = l.list.rows as usize;
                    let max = self.view_len().saturating_sub(h);
                    let top = (self.list_top as f32 - dy * 3.0).round().clamp(0.0, max as f32);
                    self.list_top = top as usize;
                } else if hit(l.player) {
                    self.volume_by(dy.signum() * 0.02);
                }
            }
        }
    }

    pub fn button(&mut self, b: ui::Button) {
        match b {
            ui::Button::Prev => self.previous(),
            ui::Button::Play => self.play(),
            ui::Button::Pause => self.pause(),
            ui::Button::Stop => self.stop(),
            ui::Button::Next => self.advance(true),
            ui::Button::Shuffle => self.toggle_shuffle(),
            ui::Button::Repeat => self.cycle_repeat(),
        }
    }

    fn modal_mouse(&mut self, ev: &MouseEvent) {
        if let Some(Modal::Settings(mut d)) = self.modal.take() {
            if d.handle_mouse(self, ev) {
                self.modal = Some(Modal::Settings(d));
            }
            return;
        }
        let Some(list) = ui::dialog_list_rect(self) else { return };
        let hit = orchard_kit::widgets::contains(list, ev.col, ev.row);
        match (&mut self.modal, ev.kind) {
            (Some(Modal::Browser(b)), MouseKind::Down) if hit => {
                let row = b.top + (ev.row - list.row) as usize;
                if row < b.rows() {
                    b.cursor = row;
                    if ev.double {
                        let action = b.activate();
                        let Some(Modal::Browser(b)) = self.modal.take() else { return };
                        self.modal = self.browser_action(b, action);
                    }
                }
            }
            (Some(Modal::Browser(b)), MouseKind::Scroll(dy)) => {
                let max = b.rows().saturating_sub(list.rows as usize);
                b.top = (b.top as f32 - dy * 3.0).round().clamp(0.0, max as f32) as usize;
            }
            (Some(Modal::Themes(t)), MouseKind::Down) if hit => {
                let row = t.top + (ev.row - list.row) as usize;
                if row < t.schemes.len() {
                    t.cursor = row;
                    let s = t.schemes[row].clone();
                    let close = ev.double;
                    self.apply_scheme(s);
                    if close {
                        self.modal = None;
                        self.save_settings();
                    }
                }
            }
            (Some(Modal::Help), MouseKind::Down) => self.modal = None,
            _ => {}
        }
    }
}

/// The scheme named `name` in `schemes`, or the default one.
fn scheme_in(schemes: &[ColorScheme], name: &str) -> ColorScheme {
    schemes
        .iter()
        .find(|s| s.name == name)
        .or_else(|| schemes::builtin_scheme(schemes::DEFAULT_SCHEME))
        .cloned()
        .unwrap_or_else(|| Theme::default_dark().scheme)
}

fn expand_tilde(text: &str) -> PathBuf {
    match text.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().map_or_else(|| PathBuf::from(text), |h| h.join(rest)),
        None => PathBuf::from(text),
    }
}

/// "3:07", "1:02:03".
pub fn format_time(secs: f64) -> String {
    let t = secs.max(0.0) as u64;
    let (h, m, s) = (t / 3600, t / 60 % 60, t % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

#[cfg(test)]
mod tests {
    use orchard_kit::input::KeyEvent;

    use super::*;
    use crate::playlist::Entry;

    fn app_with(titles: &[(&str, &str)]) -> App {
        let mut app = App::new(std::sync::Arc::new(|| {}), false, false);
        app.playlist.entries = titles
            .iter()
            .map(|(artist, title)| {
                let mut e = Entry::new(PathBuf::from(format!("/m/{title}.mp3")));
                e.tags = Some(Tags { artist: Some(artist.to_string()), title: Some(title.to_string()), ..Tags::default() });
                e
            })
            .collect();
        app
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            app.handle_key(&KeyEvent::char(c));
        }
    }

    #[test]
    fn quick_filter_narrows_and_navigates() {
        let mut app = app_with(&[("Air", "Kelly"), ("Moby", "Porcelain"), ("Air", "Talisman"), ("Bonobo", "Kerala")]);
        app.handle_key(&KeyEvent::char('/'));
        assert!(app.filter.as_ref().unwrap().editing);
        type_text(&mut app, "air");
        assert_eq!(app.filter.as_ref().unwrap().matches, [0, 2]);
        assert_eq!(app.playlist.cursor, 0);
        app.handle_key(&KeyEvent::named(Named::Down));
        assert_eq!(app.playlist.cursor, 2, "Down moves to the next match");
        // Every word must match, anywhere.
        type_text(&mut app, " tal");
        assert_eq!(app.filter.as_ref().unwrap().matches, [2]);
        // Letters went to the field, not to the player's hotkeys.
        assert!(!app.shuffle);
        app.handle_key(&KeyEvent::named(Named::Escape));
        assert!(app.filter.is_none());
        assert_eq!(app.view_len(), 4);
    }

    #[test]
    fn removing_in_a_filtered_view_keeps_the_view() {
        let mut app = app_with(&[("Air", "Kelly"), ("Moby", "Porcelain"), ("Air", "Talisman")]);
        app.start_filter();
        type_text(&mut app, "air");
        app.handle_key(&KeyEvent { key: orchard_kit::input::Key::Named(Named::Tab), mods: Default::default(), text: None });
        app.handle_key(&KeyEvent::named(Named::Delete));
        assert_eq!(app.playlist.len(), 2);
        assert_eq!(app.filter.as_ref().unwrap().matches, [1]);
        assert_eq!(app.playlist.cursor, 1);
    }

    #[test]
    fn times() {
        assert_eq!(format_time(0.0), "0:00");
        assert_eq!(format_time(187.9), "3:07");
        assert_eq!(format_time(3723.0), "1:02:03");
    }
}
