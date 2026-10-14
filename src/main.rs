//! The window: winit event loop, GPU surface, input conversion, wake-ups
//! from workers and the audio thread. Everything else is in `App`.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use oc_grid::winit;
use oc_grid::{FontConfig, Frame, Rendered, WindowRenderer};
use orchard_music_player::App;
use orchard_music_player::input::{Key, KeyEvent, Mods, MouseEvent, MouseKind, Named};
use orchard_music_player::ui;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key as WKey, ModifiersState, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::window::{Window, WindowId};

mod screenshot;

#[derive(Debug, Clone, Copy)]
enum UserEvent {
    Wake,
}

struct Win {
    window: Arc<Window>,
    gpu: WindowRenderer,
    frame: Frame,
    font: FontConfig,
    /// The cover image the renderer has.
    cover: u64,
}

struct Platform {
    app: App,
    win: Option<Win>,
    mods: ModifiersState,
    cursor: PhysicalPosition<f64>,
    button_down: bool,
    last_click: Option<(Instant, (u16, u16))>,
    scroll_px: f64,
    dropped: Vec<PathBuf>,
    startup: Vec<PathBuf>,
    /// When the next animation frame is due. Kept across wake-ups, so a
    /// stream of other events (mouse moves) can't keep pushing it back.
    next_frame: Option<Instant>,
    media: Option<orchard_music_player::media::MediaKeys>,
}

const DOUBLE_CLICK: Duration = Duration::from_millis(400);

impl Platform {
    fn cell_px(&self) -> (f32, f32) {
        self.win.as_ref().map_or((8.0, 16.0), |w| {
            let m = w.gpu.renderer().metrics();
            (m.width as f32, m.height as f32)
        })
    }

    fn cell_at(&self, p: PhysicalPosition<f64>) -> (u16, u16) {
        let (cw, ch) = self.cell_px();
        let col = (p.x.max(0.0) / cw as f64) as u16;
        let row = (p.y.max(0.0) / ch as f64) as u16;
        (col.min(self.app.cols.saturating_sub(1)), row.min(self.app.rows.saturating_sub(1)))
    }

    fn mouse(&mut self, kind: MouseKind, double: bool) {
        let (col, row) = self.cell_at(self.cursor);
        let ev = MouseEvent { kind, col, row, x: self.cursor.x as f32, y: self.cursor.y as f32, double };
        self.app.handle_mouse(&ev);
    }

    /// Grid size for the window's current pixel size and font.
    fn sync_size(&mut self) {
        let Some(w) = &mut self.win else { return };
        let (cols, rows) = w.gpu.grid_size();
        if (cols, rows) != (w.frame.cols(), w.frame.rows()) {
            w.frame.resize(cols, rows);
        }
        let m = w.gpu.renderer().metrics();
        self.app.resized(cols, rows, (m.width as f32, m.height as f32));
    }

    fn redraw(&mut self) {
        if let Some(w) = &self.win {
            w.window.request_redraw();
        }
    }

    fn draw(&mut self) {
        self.next_frame = None;
        // Font size changed (⌘+ / ⌘-)?
        let font = self.app.theme.font.clone();
        if let Some(w) = &mut self.win
            && w.font != font {
                if let Err(e) = w.gpu.renderer_mut().set_font(font.clone()) {
                    log::warn!("font: {e}");
                }
                w.font = font;
            }
        self.sync_size();
        self.app.tick(Instant::now());
        let Some(w) = &mut self.win else { return };
        // Cover art: upload when it changes.
        if let Some(now) = &self.app.now
            && now.cover_id != w.cover {
                if let Some(c) = now.details.as_ref().and_then(|d| d.cover.as_ref()) {
                    let r = w.gpu.renderer_mut();
                    if w.cover != 0 {
                        r.remove_image(ui::cover_image(w.cover));
                    }
                    if let Err(e) = r.set_image(ui::cover_image(now.cover_id), c.width, c.height, &c.rgba) {
                        log::warn!("cover: {e}");
                    }
                }
                w.cover = now.cover_id;
            }
        ui::draw(&self.app, &mut w.frame);
        match w.gpu.render(&w.frame) {
            Ok(Rendered::Retry) => w.window.request_redraw(),
            Ok(_) => {}
            Err(e) => log::warn!("render: {e}"),
        }
    }

    fn quit(&mut self, el: &ActiveEventLoop) {
        self.app.save_session();
        el.exit();
    }
}

fn convert_key(ev: &winit::event::KeyEvent, mods: ModifiersState) -> Option<KeyEvent> {
    let m = Mods {
        shift: mods.shift_key(),
        ctrl: mods.control_key(),
        alt: mods.alt_key(),
        logo: mods.super_key(),
    };
    let key = match ev.key_without_modifiers() {
        WKey::Named(n) => Key::Named(match n {
            NamedKey::ArrowUp => Named::Up,
            NamedKey::ArrowDown => Named::Down,
            NamedKey::ArrowLeft => Named::Left,
            NamedKey::ArrowRight => Named::Right,
            NamedKey::PageUp => Named::PageUp,
            NamedKey::PageDown => Named::PageDown,
            NamedKey::Home => Named::Home,
            NamedKey::End => Named::End,
            NamedKey::Enter => Named::Enter,
            NamedKey::Escape => Named::Escape,
            NamedKey::Backspace => Named::Backspace,
            NamedKey::Delete => Named::Delete,
            NamedKey::Insert => Named::Insert,
            NamedKey::Tab => Named::Tab,
            NamedKey::Space => return Some(KeyEvent { key: Key::Char(' '), mods: m, text: Some(" ".into()) }),
            NamedKey::MediaPlayPause => return Some(KeyEvent::char(' ')),
            n => {
                let f = format!("{n:?}");
                Named::F(f.strip_prefix('F').and_then(|d| d.parse::<u8>().ok())?)
            }
        }),
        WKey::Character(s) => Key::Char(s.chars().next()?.to_lowercase().next()?),
        _ => return None,
    };
    Some(KeyEvent { key, mods: m, text: ev.text.as_ref().map(|t| t.to_string()) })
}

impl ApplicationHandler<UserEvent> for Platform {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.win.is_some() {
            return;
        }
        let (w, h) = self.app.settings.window_size.unwrap_or((980, 680));
        let attrs = Window::default_attributes()
            .with_title("Orchard Music Player")
            .with_inner_size(LogicalSize::new(w.max(480), h.max(320)))
            .with_min_inner_size(LogicalSize::new(480, 320));
        let window = match el.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("orchard-music-player: can't open a window: {e}");
                el.exit();
                return;
            }
        };
        let font = self.app.theme.font.clone();
        let gpu = match WindowRenderer::new(window.clone(), font.clone()) {
            Ok(g) => g,
            Err(e) => {
                eprintln!("orchard-music-player: {e}");
                el.exit();
                return;
            }
        };
        let (cols, rows) = gpu.grid_size();
        let frame = Frame::new(cols, rows, self.app.theme.colors.background);
        self.win = Some(Win { window, gpu, frame, font, cover: 0 });
        self.media = orchard_music_player::media::MediaKeys::new(self.app.tasks.clone());
        self.sync_size();
        let startup = std::mem::take(&mut self.startup);
        if !startup.is_empty() {
            self.app.playlist.clear();
            self.app.add_paths(startup, true);
        }
        self.redraw();
    }

    fn new_events(&mut self, _: &ActiveEventLoop, cause: StartCause) {
        if matches!(cause, StartCause::ResumeTimeReached { .. }) {
            self.redraw();
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, _: UserEvent) {
        if self.app.process() {
            self.redraw();
        }
        if self.app.quit {
            self.quit(el);
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let mut input = true;
        match event {
            WindowEvent::CloseRequested => {
                self.quit(el);
                return;
            }
            WindowEvent::RedrawRequested => {
                self.draw();
                input = false;
            }
            WindowEvent::Resized(PhysicalSize { width, height }) => {
                if let Some(w) = &mut self.win {
                    w.gpu.resize(width, height);
                    let s = PhysicalSize::new(width, height).to_logical::<u32>(w.window.scale_factor());
                    self.app.settings.window_size = Some((s.width, s.height));
                }
                self.sync_size();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                if let Some(w) = &mut self.win {
                    let _ = w.gpu.set_scale_factor(scale_factor);
                }
                self.sync_size();
            }
            WindowEvent::Occluded(false) => {}
            WindowEvent::ModifiersChanged(m) => {
                self.mods = m.state();
                input = false;
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if let Some(k) = convert_key(&event, self.mods) {
                    self.app.handle_key(&k);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = position;
                if self.button_down {
                    self.mouse(MouseKind::Drag, false);
                } else {
                    input = false;
                }
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                if state == ElementState::Pressed {
                    self.button_down = true;
                    let cell = self.cell_at(self.cursor);
                    let now = Instant::now();
                    let double = self
                        .last_click
                        .is_some_and(|(t, c)| c == cell && now.duration_since(t) < DOUBLE_CLICK);
                    self.last_click = if double { None } else { Some((now, cell)) };
                    self.mouse(MouseKind::Down, double);
                } else {
                    self.button_down = false;
                    self.mouse(MouseKind::Up, false);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let rows = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => {
                        self.scroll_px += p.y;
                        let ch = self.cell_px().1 as f64;
                        let whole = (self.scroll_px / ch).trunc();
                        self.scroll_px -= whole * ch;
                        whole as f32
                    }
                };
                if rows != 0.0 {
                    self.mouse(MouseKind::Scroll(rows), false);
                }
            }
            WindowEvent::DroppedFile(path) => self.dropped.push(path),
            _ => input = false,
        }
        if input {
            if self.app.quit {
                self.quit(el);
                return;
            }
            self.redraw();
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if !self.dropped.is_empty() {
            let paths = std::mem::take(&mut self.dropped);
            let play = self.app.state() == orchard_music_player::audio::State::Stopped;
            self.app.add_paths(paths, play);
        }
        if self.app.process() {
            self.redraw();
        }
        if let Some(m) = &mut self.media {
            m.sync(&self.app);
        }
        let now = Instant::now();
        if self.next_frame.is_some_and(|t| now >= t) {
            self.next_frame = None;
            self.redraw();
        }
        if self.next_frame.is_none() {
            self.next_frame = self.app.next_frame(now);
        }
        el.set_control_flow(match self.next_frame {
            Some(t) => ControlFlow::WaitUntil(t),
            None => ControlFlow::Wait,
        });
    }

    fn exiting(&mut self, _: &ActiveEventLoop) {
        // ⌘Q from the macOS app menu ends the loop without CloseRequested.
        self.app.save_session();
    }
}

fn usage() -> &'static str {
    "usage: orchard-music-player [FILE|FOLDER|PLAYLIST.m3u]...\n       orchard-music-player --version | --self-test\n       orchard-music-player --screenshot OUT.png [--size COLSxROWS] [--theme NAME]"
}

/// `--self-test`: decoding and drawing work in this build (for packaging
/// checks; no window, no audio device).
fn self_test() -> ExitCode {
    println!("Orchard Music Player {}", env!("CARGO_PKG_VERSION"));
    let dir = std::env::temp_dir();
    let decode = orchard_music_player::audio::self_test(&dir);
    let shot = dir.join(format!("orchard-music-player-self-test-{}.png", std::process::id()));
    let draw = match screenshot::run(&[shot.to_string_lossy().into_owned()]) {
        ExitCode::SUCCESS => Ok("the UI drew headlessly".to_string()),
        _ => Err("the UI didn't draw".to_string()),
    };
    let _ = std::fs::remove_file(&shot);
    let mut ok = true;
    for (what, result) in [("audio", decode), ("render", draw)] {
        match result {
            Ok(done) => println!("ok    {what}: {done}"),
            Err(e) => {
                ok = false;
                println!("FAIL  {what}: {e}");
            }
        }
    }
    if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{}", usage());
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "-V" || a == "--version") {
        println!("Orchard Music Player {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--self-test") {
        return self_test();
    }
    if args.first().map(String::as_str) == Some("--screenshot") {
        return screenshot::run(&args[1..]);
    }
    let startup: Vec<PathBuf> = args.iter().map(PathBuf::from).collect();

    let event_loop = match EventLoop::<UserEvent>::with_user_event().build() {
        Ok(el) => el,
        Err(e) => {
            eprintln!("orchard-music-player: {e}");
            return ExitCode::FAILURE;
        }
    };
    let proxy = Mutex::new(event_loop.create_proxy());
    let wake: orchard_music_player::tasks::Waker = Arc::new(move || {
        let _ = proxy.lock().unwrap().send_event(UserEvent::Wake);
    });
    let app = App::new(wake, true, true);
    let mut platform = Platform {
        app,
        win: None,
        mods: ModifiersState::empty(),
        cursor: PhysicalPosition::new(0.0, 0.0),
        button_down: false,
        last_click: None,
        scroll_px: 0.0,
        dropped: Vec::new(),
        startup,
        next_frame: None,
        media: None,
    };
    match event_loop.run_app(&mut platform) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("orchard-music-player: {e}");
            ExitCode::FAILURE
        }
    }
}
