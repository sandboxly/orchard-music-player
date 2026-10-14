//! Media keys and the system's Now Playing (souvlaki: MPRemoteCommandCenter
//! and MPNowPlayingInfoCenter on macOS, MPRIS on Linux). Key presses arrive
//! on another thread and are posted to the app; `sync` tells the system
//! what plays whenever that changes.

use std::path::PathBuf;
use std::time::Duration;

use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig, SeekDirection};

use crate::app::App;
use crate::audio::State;
use crate::tasks::Tasks;

/// What the system was last told.
#[derive(Clone, Debug, Default, PartialEq)]
struct Shown {
    path: Option<PathBuf>,
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    duration: Option<u64>,
    cover_id: u64,
    state: Option<State>,
    /// The position in 5-second steps: re-sent now and then and after seeks.
    position_step: i64,
}

pub struct MediaKeys {
    controls: MediaControls,
    shown: Shown,
    cover_file: Option<PathBuf>,
}

/// Seek step for the system's plain "seek forward/backward" commands.
const SEEK_STEP: f64 = 10.0;

impl MediaKeys {
    /// `None` where the platform offers no media controls (or Windows,
    /// which needs the window handle).
    pub fn new(tasks: Tasks<App>) -> Option<MediaKeys> {
        let config = PlatformConfig {
            display_name: "Orchard Music Player",
            dbus_name: "orchard_music_player",
            hwnd: None,
        };
        let mut controls = MediaControls::new(config).map_err(|e| log::info!("media keys: {e:?}")).ok()?;
        controls
            .attach(move |event| tasks.post(move |app: &mut App| app.media_event(event)))
            .map_err(|e| log::info!("media keys: {e:?}"))
            .ok()?;
        Some(MediaKeys { controls, shown: Shown::default(), cover_file: None })
    }

    /// Tell the system about the track and state, if they changed.
    pub fn sync(&mut self, app: &App) {
        let state = app.state();
        let now = app.now.as_ref().filter(|_| state != State::Stopped);
        let want = Shown {
            path: now.map(|n| n.path.clone()),
            title: now.map(|n| n.tags.title.clone().unwrap_or_else(|| crate::meta::file_stem(&n.path))),
            artist: now.and_then(|n| n.tags.artist.clone()),
            album: now.and_then(|n| n.tags.album.clone()),
            duration: app.duration().map(|d| d as u64),
            cover_id: now.map_or(0, |n| n.cover_id),
            state: Some(state),
            position_step: (app.position() / 5.0) as i64,
        };
        if want == self.shown {
            return;
        }
        let track_changed = (&want.path, &want.title, &want.artist, &want.album, want.duration, want.cover_id)
            != (&self.shown.path, &self.shown.title, &self.shown.artist, &self.shown.album, self.shown.duration, self.shown.cover_id);
        if track_changed {
            let cover_url = now.and_then(|n| self.write_cover(n));
            let meta = MediaMetadata {
                title: want.title.as_deref(),
                artist: want.artist.as_deref(),
                album: want.album.as_deref(),
                cover_url: cover_url.as_deref(),
                duration: want.duration.map(Duration::from_secs),
            };
            if let Err(e) = self.controls.set_metadata(meta) {
                log::debug!("now playing: {e:?}");
            }
        }
        let progress = Some(MediaPosition(Duration::from_secs_f64(app.position().max(0.0))));
        let playback = match state {
            State::Playing => MediaPlayback::Playing { progress },
            State::Paused => MediaPlayback::Paused { progress },
            State::Stopped => MediaPlayback::Stopped,
        };
        if let Err(e) = self.controls.set_playback(playback) {
            log::debug!("now playing: {e:?}");
        }
        self.shown = want;
    }

    /// The cover as a PNG in the cache folder, for the system to load; a
    /// new name per cover so it isn't served from a cache.
    fn write_cover(&mut self, now: &crate::app::NowPlaying) -> Option<String> {
        if let Some(old) = self.cover_file.take() {
            let _ = std::fs::remove_file(old);
        }
        let cover = now.details.as_ref()?.cover.as_ref()?;
        let dir = dirs::cache_dir()?.join("orchard-music-player");
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join(format!("cover-{}.png", now.cover_id));
        image::save_buffer(&path, &cover.rgba, cover.width, cover.height, image::ExtendedColorType::Rgba8).ok()?;
        let url = format!("file://{}", path.to_string_lossy().replace('%', "%25").replace(' ', "%20"));
        self.cover_file = Some(path);
        Some(url)
    }
}

impl Drop for MediaKeys {
    fn drop(&mut self) {
        if let Some(old) = self.cover_file.take() {
            let _ = std::fs::remove_file(old);
        }
    }
}

impl App {
    /// A media key or a Now Playing control.
    pub fn media_event(&mut self, event: MediaControlEvent) {
        match event {
            MediaControlEvent::Toggle => self.play_pause(),
            MediaControlEvent::Play => self.play(),
            MediaControlEvent::Pause => {
                if self.player.state() == State::Playing {
                    self.player.pause();
                }
            }
            MediaControlEvent::Stop => self.stop(),
            MediaControlEvent::Next => self.advance(true),
            MediaControlEvent::Previous => self.previous(),
            MediaControlEvent::Seek(dir) => self.seek_by(signed(dir, SEEK_STEP)),
            MediaControlEvent::SeekBy(dir, d) => self.seek_by(signed(dir, d.as_secs_f64())),
            MediaControlEvent::SetPosition(MediaPosition(p)) => self.player.seek(p.as_secs_f64()),
            MediaControlEvent::SetVolume(v) => self.set_volume(v as f32),
            MediaControlEvent::Quit => self.quit = true,
            MediaControlEvent::OpenUri(_) | MediaControlEvent::Raise => {}
        }
    }
}

fn signed(dir: SeekDirection, secs: f64) -> f64 {
    match dir {
        SeekDirection::Forward => secs,
        SeekDirection::Backward => -secs,
    }
}
