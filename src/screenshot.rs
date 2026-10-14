//! `--screenshot OUT.png`: a demo state drawn headlessly (docs, checking
//! the look without a window or audio device).

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use oc_grid::{Frame, HeadlessRenderer};
use orchard_music_player::App;
use orchard_music_player::app::NowPlaying;
use orchard_music_player::audio::State;
use orchard_music_player::meta::{Details, Tags};
use orchard_music_player::playlist::Entry;
use orchard_music_player::ui;

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

pub fn run(args: &[String]) -> ExitCode {
    let Some(out) = args.first().map(PathBuf::from) else {
        eprintln!("--screenshot needs an output file");
        return ExitCode::FAILURE;
    };
    let mut size = (112u16, 38u16);
    let mut theme: Option<String> = None;
    let mut modal: Option<String> = None;
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--size" => {
                if let Some((c, r)) = it.next().and_then(|s| s.split_once('x')) {
                    size = (c.parse().unwrap_or(size.0), r.parse().unwrap_or(size.1));
                }
            }
            "--theme" => theme = it.next().cloned(),
            "--dialog" => modal = it.next().cloned(),
            _ => {}
        }
    }
    let mut app = App::new(Arc::new(|| {}), false, false);
    if let Some(t) = theme {
        let s = oc_config::themes::get_scheme(&orchard_music_player::theme::schemes_dir(), &t);
        app.apply_scheme(s);
    }
    demo(&mut app);

    let mut r = match HeadlessRenderer::new(app.theme.font.clone(), 2.0) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("renderer: {e}");
            return ExitCode::FAILURE;
        }
    };
    let m = r.renderer().metrics();
    app.resized(size.0, size.1, (m.width as f32, m.height as f32));
    fake_spectrum(&mut app);
    match modal.as_deref() {
        Some("help") => app.modal = Some(orchard_music_player::app::Modal::Help),
        Some("themes") => app.open_theme_picker(),
        Some("save") => app.open_save_prompt(),
        Some("filter") => {
            app.start_filter();
            if let Some(f) = &mut app.filter {
                f.field = orchard_music_player::ui::widgets::TextField::new("ai");
            }
            app.refilter();
        }
        _ => {}
    }
    let mut frame = Frame::new(size.0, size.1, app.theme.colors.background);
    ui::draw(&app, &mut frame);
    match r.render(&frame).and_then(|img| img.save_png(&out)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("screenshot: {e}");
            ExitCode::FAILURE
        }
    }
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
