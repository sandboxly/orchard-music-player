//! The window: the shared shell (`orchard_shell`: winit event loop, GPU
//! surface, input conversion, wake-ups from workers and the audio thread)
//! around `App`, plus what only the player does there: media keys, dropped
//! files, the cover art. Everything else is in `App`.

// Windows: a GUI program, no console window behind it. Output still reaches
// a redirected stdout (CI's `Start-Process -RedirectStandardOutput`); in a
// console window the flags that print attach to it (`attach_parent_console`).
#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use orchard_grid::{FontConfig, Frame, GridRenderer, Rgba};
use orchard_kit::input::{KeyEvent, MouseEvent};
use orchard_music_player::App;
use orchard_music_player::media::MediaKeys;
use orchard_music_player::ui;
use orchard_shell::winit;
use orchard_shell::{AppInfo, ShellApp, cli};
use winit::keyboard::{Key as WKey, ModifiersState, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::window::Window;

mod screenshot;

const INFO: AppInfo = AppInfo {
    name: "Orchard Music Player",
    bin: "orchard-music-player",
    version: env!("CARGO_PKG_VERSION"),
    default_size: (980, 680),
    min_size: (480, 320),
    exit_after_start_env: "ORCHARD_MUSIC_PLAYER_EXIT_AFTER_START",
};

/// The player in the shell's window.
struct Shell {
    app: App,
    /// The cover image the renderer has.
    cover: u64,
    media: Option<MediaKeys>,
    dropped: Vec<PathBuf>,
    /// Files from the command line, played once the window is up.
    startup: Vec<PathBuf>,
}

impl ShellApp for Shell {
    fn font(&self) -> &FontConfig {
        &self.app.theme.font
    }

    fn background(&self) -> Rgba {
        self.app.theme.colors.background
    }

    fn grid(&self) -> (u16, u16) {
        (self.app.cols, self.app.rows)
    }

    fn window_size(&self) -> Option<(u32, u32)> {
        self.app.settings.window_size
    }

    fn set_window_size(&mut self, logical: (u32, u32)) {
        self.app.settings.window_size = Some(logical);
    }

    fn quit_requested(&self) -> bool {
        self.app.quit
    }

    fn set_system_dark(&mut self, dark: bool) {
        self.app.set_system_dark(dark);
    }

    fn resized(&mut self, cols: u16, rows: u16, cell_px: (f32, f32)) {
        self.app.resized(cols, rows, cell_px);
    }

    fn tick(&mut self, now: Instant) {
        self.app.tick(now);
    }

    fn draw(&self, frame: &mut Frame) {
        ui::draw(&self.app, frame);
    }

    fn process(&mut self) -> bool {
        self.app.process()
    }

    fn next_frame(&self, now: Instant) -> Option<Instant> {
        self.app.next_frame(now)
    }

    fn handle_key(&mut self, ev: &KeyEvent) {
        self.app.handle_key(ev);
    }

    fn handle_mouse(&mut self, ev: &MouseEvent) {
        self.app.handle_mouse(ev);
    }

    fn save(&mut self) {
        self.app.save_session();
    }

    /// A key press as a [`KeyEvent`] (the kit's conversion), `None` for keys
    /// the app doesn't use. The keyboard's play/pause media key arrives as
    /// Space, so it plays and pauses, whatever modifiers are held.
    fn map_key(&self, ev: &winit::event::KeyEvent, mods: ModifiersState) -> Option<KeyEvent> {
        if ev.key_without_modifiers() == WKey::Named(NamedKey::MediaPlayPause) {
            return Some(KeyEvent::char(' '));
        }
        orchard_kit::winit::convert_key(ev, mods)
    }

    fn started(&mut self, window: &Window) {
        self.media = MediaKeys::new(self.app.tasks.clone(), window_handle(window));
        let startup = std::mem::take(&mut self.startup);
        if !startup.is_empty() {
            self.app.playlist.clear();
            self.app.add_paths(startup, true);
        }
    }

    fn before_draw(&mut self, _window: &Window, r: &mut GridRenderer) {
        // Cover art: upload when it changes.
        if let Some(now) = &self.app.now
            && now.cover_id != self.cover
        {
            if let Some(c) = now.details.as_ref().and_then(|d| d.cover.as_ref()) {
                if self.cover != 0 {
                    r.remove_image(ui::cover_image(self.cover));
                }
                if let Err(e) = r.set_image(ui::cover_image(now.cover_id), c.width, c.height, &c.rgba) {
                    log::warn!("cover: {e}");
                }
            }
            self.cover = now.cover_id;
        }
    }

    fn dropped_file(&mut self, path: PathBuf) -> bool {
        self.dropped.push(path);
        true
    }

    fn before_process(&mut self) {
        if !self.dropped.is_empty() {
            let paths = std::mem::take(&mut self.dropped);
            let play = self.app.state() == orchard_music_player::audio::State::Stopped;
            self.app.add_paths(paths, play);
        }
    }

    fn after_process(&mut self) {
        if let Some(m) = &mut self.media {
            m.sync(&self.app);
        }
    }
}

/// The native window handle media keys need on Windows (`None` elsewhere).
fn window_handle(window: &Window) -> Option<*mut std::ffi::c_void> {
    #[cfg(windows)]
    {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let handle = window.window_handle().ok()?;
        match handle.as_raw() {
            RawWindowHandle::Win32(h) => Some(h.hwnd.get() as *mut std::ffi::c_void),
            _ => None,
        }
    }
    #[cfg(not(windows))]
    {
        let _ = window;
        None
    }
}

fn usage() -> &'static str {
    "usage: orchard-music-player [FILE|FOLDER|PLAYLIST.m3u]...\n       orchard-music-player --version | --self-test\n       orchard-music-player --screenshot OUT.png|OUT.webp [--size COLSxROWS] [--theme NAME] [--dialog help|themes|settings|settings-playback|scheme-editor|color-picker|save|filter|browser] [--music DIR [--play N] [--at SECONDS]] [--vis bars|scope|off]"
}

/// `--self-test`: decoding and drawing work in this build (for packaging
/// checks; no window, no audio device).
fn self_test() -> ExitCode {
    cli::self_test(
        &INFO,
        vec![
            ("audio", Box::new(|| orchard_music_player::audio::self_test(&std::env::temp_dir()))),
            ("render", Box::new(|| cli::render_check(&INFO, screenshot::run))),
        ],
    )
}

fn main() -> ExitCode {
    #[cfg(windows)]
    cli::attach_parent_console();
    cli::log_panics(&INFO, orchard_music_player::settings::config_dir);
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = cli::common_args(&INFO, &args, usage(), self_test, screenshot::run) {
        return code;
    }
    let startup: Vec<PathBuf> = args.iter().map(PathBuf::from).collect();
    orchard_shell::run(&INFO, |wake| Shell {
        app: App::new(wake, true, true),
        cover: 0,
        media: None,
        dropped: Vec::new(),
        startup,
    })
}
