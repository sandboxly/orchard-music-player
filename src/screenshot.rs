//! `--screenshot OUT.png|OUT.webp`: a state drawn headlessly (docs, checking
//! the look without a window or audio device). Either a built-in demo, or
//! real files with `--music DIR`: their tags, the playing track's details and
//! cover, and the visualizer fed with its decoded audio at `--at SECONDS`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use orchard_grid::Frame;
use orchard_kit::input::{KeyEvent, Mods, Named};
use orchard_music_player::App;
use orchard_music_player::app::{Modal, NowPlaying, Pick};
use orchard_music_player::audio::{self, State};
use orchard_music_player::meta::{self, Details, Tags};
use orchard_music_player::playlist::{self, Entry};
use orchard_music_player::spectrum;
use orchard_music_player::ui;
use orchard_music_player::ui::settings;
use orchard_shell::shot;

const DEMO: &[(&str, &str, &str, f64)] = &[
    ("Boards of Canada", "Roygbiv", "Music Has the Right to Children", 151.0),
    ("Aphex Twin", "Xtal", "Selected Ambient Works 85–92", 294.0),
    ("Massive Attack", "Teardrop", "Mezzanine", 330.0),
    ("Portishead", "Roads", "Dummy", 305.0),
    ("DJ Shadow", "Midnight in a Perfect World", "Endtroducing.....", 318.0),
    ("Air", "La Femme d'Argent", "Moon Safari", 427.0),
    ("Bonobo", "Kerala", "Migration", 241.0),
    ("Röyksopp", "Eple", "Melody A.M.", 221.0),
    ("Moby", "Porcelain", "Play", 241.0),
    ("Tycho", "Awake", "Awake", 284.0),
    ("Nightmares on Wax", "You Wish", "In a Space Outta Sound", 307.0),
    ("Thievery Corporation", "Lebanese Blonde", "The Mirror Conspiracy", 285.0),
    ("Kruder & Dorfmeister", "High Noon", "The K&D Sessions", 365.0),
    ("Zero 7", "Destiny", "Simple Things", 335.0),
];

/// The cover's image id in the renderer.
const COVER_ID: u64 = 1;
/// Frames of audio the analyzer sees before the shot, so the peak caps
/// hang above the bars as they would.
const WARMUP_FRAMES: usize = 40;
const FPS: f64 = 60.0;

/// The options only this app's screenshots have (the common ones are
/// `orchard_shell::shot`'s).
#[derive(Default)]
struct Options {
    music: Option<PathBuf>,
    /// 1-based playlist position of the playing track.
    play: usize,
    at: f64,
    vis: Option<spectrum::Mode>,
    filter: Option<String>,
    /// 1-based playlist position of the selection.
    cursor: Option<usize>,
    /// Where the file picker opens (else the `--music` folder, else the system's).
    browse: Option<PathBuf>,
    shuffle: bool,
    volume: f32,
}

impl Options {
    /// One of this app's options, with `it` for its value.
    fn parse(&mut self, a: &str, it: &mut std::slice::Iter<'_, String>) {
        match a {
            "--music" => self.music = it.next().map(PathBuf::from),
            "--play" => self.play = it.next().and_then(|s| s.parse().ok()).unwrap_or(1),
            "--at" => self.at = it.next().and_then(|s| s.parse().ok()).unwrap_or(60.0),
            "--vis" => self.vis = it.next().map(|s| spectrum::Mode::from_label(s)),
            "--filter" => self.filter = it.next().cloned(),
            "--cursor" => self.cursor = it.next().and_then(|s| s.parse().ok()),
            "--browse" => self.browse = it.next().map(PathBuf::from),
            "--shuffle" => self.shuffle = true,
            "--volume" => self.volume = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.72),
            _ => {}
        }
    }
}

pub fn run(args: &[String]) -> ExitCode {
    let mut o = Options { play: 1, at: 60.0, volume: 0.72, ..Options::default() };
    let shot = match shot::parse(args, (112, 38), |a, it| o.parse(a, it)) {
        Ok(s) => s,
        Err(code) => return code,
    };
    // Wake-ups from background work (the picker's listing, places and bookmarks).
    let (tx, woken) = mpsc::channel();
    let mut app = App::new(
        Arc::new(move || {
            let _ = tx.send(());
        }),
        false,
        false,
    );
    if let Some(t) = &shot.theme {
        let s = orchard_theme::schemes::get_scheme(&orchard_theme::dirs::schemes_dir(), t);
        app.apply_scheme(s);
    }
    if let Some(v) = o.vis {
        app.vis = v;
    }
    let music = match &o.music {
        Some(dir) => match real(&mut app, dir, &o) {
            Ok(m) => Some(m),
            Err(e) => {
                eprintln!("screenshot: {e}");
                return ExitCode::FAILURE;
            }
        },
        None => {
            demo(&mut app);
            None
        }
    };

    let mut r = match shot::renderer(app.theme.font.clone()) {
        Ok(r) => r,
        Err(code) => return code,
    };
    let (cols, rows) = shot.size;
    app.resized(cols, rows, shot::cell_px(&mut r));
    match &music {
        Some(audio) => real_spectrum(&mut app, audio),
        None => fake_spectrum(&mut app),
    }
    if let Some(cover) = app.now.as_ref().and_then(|n| n.details.as_ref()).and_then(|d| d.cover.as_ref())
        && let Err(e) = r.renderer().set_image(ui::cover_image(COVER_ID), cover.width, cover.height, &cover.rgba)
    {
        eprintln!("cover: {e}");
    }
    if let Some(dir) = &o.browse {
        app.settings.last_dir = Some(dir.canonicalize().unwrap_or_else(|_| dir.clone()));
    }
    match shot.dialog.as_deref() {
        Some("help") => app.modal = Some(Modal::Help),
        Some("themes") => app.open_theme_picker(),
        Some("save") => app.open_save_prompt(),
        Some("filter") => {
            app.start_filter();
            if let Some(f) = &mut app.filter {
                f.field = orchard_kit::widgets::TextField::new(o.filter.as_deref().unwrap_or("ai")).flush().plain();
            }
            app.refilter();
        }
        Some("browser") => open_picker(&mut app, Pick::Music, &woken),
        Some("load") => open_picker(&mut app, Pick::Playlist, &woken),
        Some("bookmarks") => {
            open_picker(&mut app, Pick::Music, &woken);
            let mods = Mods { ctrl: true, logo: true, ..Mods::default() };
            app.handle_key(&KeyEvent::char('d').with(mods));
        }
        Some(d @ ("settings" | "settings-playback" | "scheme-editor" | "color-picker")) => open_settings(&mut app, d),
        Some(other) => {
            eprintln!("screenshot: no dialog '{other}'");
            return ExitCode::FAILURE;
        }
        None => {}
    }
    let mut frame = Frame::new(cols, rows, app.theme.colors.background);
    ui::draw(&app, &mut frame);
    shot::finish(&mut r, &frame, &shot.out)
}

/// The file picker in the folder used last (`--browse`, `--music`), once
/// its listing, places and bookmarks are in (a few seconds at most), with
/// the cursor a few rows down.
fn open_picker(app: &mut App, pick: Pick, woken: &Receiver<()>) {
    app.open_picker(pick);
    let until = Instant::now() + Duration::from_secs(5);
    while matches!(&app.modal, Some(Modal::Picker(p, _)) if p.loading()) && Instant::now() < until {
        let _ = woken.recv_timeout(until.saturating_duration_since(Instant::now()));
        app.process();
    }
    for _ in 0..2 {
        app.handle_key(&KeyEvent::named(Named::Down));
    }
}

/// The settings dialog: the Appearance page with the scheme list focused,
/// the Playback page, or the scheme editor (with its color picker) over it.
fn open_settings(app: &mut App, which: &str) {
    settings::load_fonts_now(app);
    app.open_settings();
    let Some(Modal::Settings(d)) = &mut app.modal else { return };
    match which {
        "settings-playback" => {
            d.page = 1;
            d.focus = settings::Focus::Ctl("replay_gain");
        }
        "settings" => d.focus = settings::Focus::Ctl("scheme"),
        _ => {}
    }
    if which == "scheme-editor" || which == "color-picker" {
        let Some(Modal::Settings(mut d)) = app.modal.take() else { return };
        let mut editor = settings::scheme_editor::SchemeEditor::open(app, settings::Slot::Color, false);
        if which == "color-picker" {
            // "Headers and titles".
            editor.open_picker(4);
        }
        d.editor = Some(editor);
        app.modal = Some(Modal::Settings(d));
    }
}

/// Decoded audio of the playing track, ending at the shot's moment.
struct Audio {
    samples: Vec<f32>,
    rate: u32,
}

/// The files under `dir` as the app adds them, with their tags; the
/// `o.play`th plays at `o.at` seconds.
fn real(app: &mut App, dir: &Path, o: &Options) -> Result<Audio, String> {
    let dir = dir.canonicalize().map_err(|e| format!("{}: {}", dir.display(), meta::io_reason(&e)))?;
    let mut entries = playlist::expand(std::slice::from_ref(&dir));
    if entries.is_empty() {
        return Err(format!("no audio files in {}", dir.display()));
    }
    for e in &mut entries {
        e.tags = Some(meta::read_tags(&e.path));
    }
    app.playlist.entries = entries;
    let cur = o.play.clamp(1, app.playlist.len()) - 1;
    app.playlist.current = Some(cur);
    app.playlist.mark_played(cur);
    app.playlist.cursor = o.cursor.map_or(cur, |c| c.clamp(1, app.playlist.len()) - 1);
    app.settings.last_dir = Some(dir.clone());

    let e = &app.playlist.entries[cur];
    let path = e.path.clone();
    let tags = e.tags.clone().unwrap_or_default();
    let rate = app.player.rate();
    let span = spectrum::FFT_SIZE.max(audio::SCOPE_LEN) + (WARMUP_FRAMES as f64 * rate as f64 / FPS) as usize;
    let from = (o.at - span as f64 / rate as f64).max(0.0);
    let (samples, info) = audio::decode_mono(&path, from, span, rate, app.gain_mode)?;
    if samples.len() < spectrum::FFT_SIZE {
        return Err(format!("{} is shorter than {} s", path.display(), o.at));
    }
    let details: Details = meta::read_details(&path);
    app.now = Some(NowPlaying {
        path,
        duration: tags.duration.or(info.duration),
        tags,
        details: Some(details),
        codec: info.codec,
        cover_id: COVER_ID,
        gain: Some(info.gain),
    });
    app.fake_state = Some(State::Playing);
    app.fake_position = Some(from + samples.len() as f64 / rate as f64);
    app.shuffle = o.shuffle;
    app.player.set_volume(o.volume);
    Ok(Audio { samples, rate })
}

/// The analyzer over the last frames of real audio, as the app runs it at
/// 60 fps; the scope gets the latest samples.
fn real_spectrum(app: &mut App, audio: &Audio) {
    let count = ui::bar_count(app);
    let step = (audio.rate as f64 / FPS) as usize;
    let t0 = Instant::now();
    let n = audio.samples.len();
    for k in 0..=WARMUP_FRAMES {
        let end = n - (WARMUP_FRAMES - k) * step;
        if end < spectrum::FFT_SIZE {
            continue;
        }
        let now = t0 + Duration::from_secs_f64(k as f64 / FPS);
        app.analyzer.update(Some(&audio.samples[..end]), audio.rate, count, now);
    }
    app.player.fill_scope(&audio.samples);
}

fn demo(app: &mut App) {
    app.playlist.entries = DEMO
        .iter()
        .map(|(artist, title, album, secs)| {
            let mut e = Entry::new(PathBuf::from(format!("/Music/{artist}/{album}/{title}.mp3")));
            e.tags = Some(Tags {
                title: Some(title.to_string()),
                artist: Some(artist.to_string()),
                album: Some(album.to_string()),
                duration: Some(*secs),
                ..Tags::default()
            });
            e
        })
        .collect();
    let cur = 2;
    app.playlist.current = Some(cur);
    app.playlist.cursor = 5;
    let e = &app.playlist.entries[cur];
    app.now = Some(NowPlaying {
        path: e.path.clone(),
        tags: Tags { year: Some("1998".into()), ..e.tags.clone().unwrap() },
        details: Some(Details { sample_rate: Some(44_100), channels: Some(2), bitrate: Some(320), ..Details::default() }),
        duration: e.duration(),
        codec: "MP3".into(),
        cover_id: 0,
        gain: Some(orchard_music_player::meta::ReplayGain { track_gain: Some(-6.3), track_peak: Some(0.98), ..Default::default() }),
    });
    app.fake_state = Some(State::Playing);
    app.fake_position = Some(127.0);
    app.shuffle = true;
    app.player.set_volume(0.72);
}

/// A plausible frame of music: loud bass, falling treble, some wobble.
fn fake_spectrum(app: &mut App) {
    let n = ui::bar_count(app);
    let bars: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let base = 0.92 - 0.55 * t;
            let wobble = 0.18 * ((i as f32 * 0.9).sin() * (i as f32 * 0.37).cos());
            (base + wobble).clamp(0.05, 1.0)
        })
        .collect();
    app.analyzer.peaks = bars.iter().enumerate().map(|(i, b)| (b + 0.06 + 0.05 * (i as f32 * 1.3).sin().abs()).min(1.0)).collect();
    app.analyzer.bars = bars;
}
