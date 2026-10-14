//! The settings dialog and the scheme editor, driven by keys and clicks
//! like a user would. Schemes and settings go to temp dirs only.

use std::path::Path;
use std::sync::Arc;

use oc_config::themes;
use oc_grid::Rgba;

use super::*;
use crate::input::{Mods, MouseKind};
use crate::meta::GainMode;

fn app_in(dir: &Path) -> App {
    let mut app = App::new(Arc::new(|| {}), false, false);
    app.schemes_dir = dir.join("color-schemes");
    app.refresh_schemes();
    app.resized(112, 36, (8.0, 16.0));
    app
}

fn primary(c: char) -> KeyEvent {
    let mac = cfg!(target_os = "macos");
    KeyEvent::char(c).with(Mods { logo: mac, ctrl: !mac, ..Mods::default() })
}

fn key(app: &mut App, n: Named) {
    app.handle_key(&KeyEvent::named(n));
}

fn shift_tab(app: &mut App) {
    app.handle_key(&KeyEvent::named(Named::Tab).with(Mods { shift: true, ..Mods::default() }));
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        app.handle_key(&KeyEvent::char(c));
    }
}

fn dialog(app: &App) -> &SettingsDialog {
    match &app.modal {
        Some(Modal::Settings(d)) => d,
        _ => panic!("the settings aren't open"),
    }
}

fn editor(app: &App) -> &SchemeEditor {
    dialog(app).editor.as_ref().expect("the scheme editor is open")
}

/// Tab until control `id` has the focus.
fn focus_on(app: &mut App, id: &'static str) {
    for _ in 0..30 {
        if dialog(app).focus == Focus::Ctl(id) {
            return;
        }
        key(app, Named::Tab);
    }
    panic!("no control {id}");
}

fn click(app: &mut App, kind: MouseKind, col: u16, row: u16) {
    app.handle_mouse(&MouseEvent { kind, col, row, x: 0.0, y: 0.0, double: false });
}

#[test]
fn keys_move_through_pages_and_controls() {
    let tmp = tempfile::tempdir().unwrap();
    let mut app = app_in(tmp.path());
    app.handle_key(&primary(','));
    assert_eq!(dialog(&app).focus, Focus::Sections);
    let mut seen = Vec::new();
    for _ in 0..12 {
        key(&mut app, Named::Tab);
        seen.push(dialog(&app).focus);
    }
    let ids = [
        "follow_system",
        "scheme",
        "scheme.edit",
        "scheme.create",
        "font_family",
        "font_style",
        "font_size",
        "line_spacing",
        "frame_style",
        "restore",
    ];
    let mut want: Vec<Focus> = ids.iter().map(|id| Focus::Ctl(id)).collect();
    want.extend([Focus::Close, Focus::Sections]);
    assert_eq!(seen, want);
    shift_tab(&mut app);
    assert_eq!(dialog(&app).focus, Focus::Close);
    // ↑/↓ move too, without wrapping.
    key(&mut app, Named::Up);
    assert_eq!(dialog(&app).focus, Focus::Ctl("restore"));
    // The page list: ↓ is the next page; → goes into it.
    focus_on(&mut app, "follow_system");
    shift_tab(&mut app);
    key(&mut app, Named::Down);
    assert_eq!(dialog(&app).page, 1);
    key(&mut app, Named::Right);
    assert_eq!(dialog(&app).focus, Focus::Ctl("replay_gain"));
    key(&mut app, Named::Escape);
    assert!(app.modal.is_none());
    // F9 opens it too, and closes it.
    app.handle_key(&KeyEvent::named(Named::F(9)));
    assert_eq!(dialog(&app).page, 0);
    app.handle_key(&KeyEvent::named(Named::F(9)));
    assert!(app.modal.is_none());
}

#[test]
fn changes_apply_at_once_and_are_saved() {
    let tmp = tempfile::tempdir().unwrap();
    let mut app = app_in(tmp.path());
    app.persist_in(tmp.path());
    app.fkey(9);

    // A color scheme: → picks the next one, the player shows it.
    focus_on(&mut app, "scheme");
    key(&mut app, Named::Right);
    let second = themes::builtin_schemes()[1].name.clone();
    assert_eq!(app.settings.color_scheme, second);
    assert_eq!(app.theme.scheme.name, second);
    assert_eq!(Settings::load(tmp.path()).color_scheme, second, "saved at once");

    // Font size: ←/→, typing, the window's font follows.
    focus_on(&mut app, "font_size");
    key(&mut app, Named::Right);
    assert_eq!(app.settings.font_size, 15);
    assert_eq!(app.theme.font.size, 15.0);
    type_text(&mut app, "2");
    assert_eq!(app.settings.font_size, 15, "2 pt is too small: kept until a valid size is typed");
    type_text(&mut app, "0");
    assert_eq!(app.settings.font_size, 20);
    type_text(&mut app, "0");
    assert_eq!(app.settings.font_size, 20, "200 is above the maximum: not taken");
    assert_eq!(Settings::load(tmp.path()).font_size, 20);

    // Line spacing and frames.
    focus_on(&mut app, "line_spacing");
    key(&mut app, Named::Right);
    assert_eq!(app.settings.line_spacing, 1.05);
    assert_eq!(app.theme.font.line_spacing, 1.05);
    focus_on(&mut app, "frame_style");
    key(&mut app, Named::Right);
    assert_eq!(app.settings.frame_style, "single");
    assert_eq!(app.theme.frame, oc_grid::BoxStyle::from_name("single").unwrap());

    // Following the system: light and dark mode get a list each.
    focus_on(&mut app, "follow_system");
    app.handle_key(&KeyEvent::char(' '));
    assert!(app.settings.follow_system_appearance);
    focus_on(&mut app, "scheme.light");
    focus_on(&mut app, "scheme.dark");
    app.set_system_dark(false);
    assert_eq!(app.theme.scheme.name, app.settings.light_scheme);
    app.set_system_dark(true);
    assert_eq!(app.theme.scheme.name, app.settings.dark_scheme);

    // ReplayGain on the Playback page, the same setting as the G key.
    app.modal = None;
    app.fkey(9);
    key(&mut app, Named::Down);
    focus_on(&mut app, "replay_gain");
    key(&mut app, Named::Left);
    assert_eq!(app.gain_mode, GainMode::Off);
    assert_eq!(Settings::load(tmp.path()).replay_gain, "off");
    key(&mut app, Named::Escape);
    app.handle_key(&KeyEvent::char('g'));
    assert_eq!(app.gain_mode, GainMode::Track);
    assert_eq!(app.settings.replay_gain, "track");
}

#[test]
fn lists_open_and_choose() {
    let tmp = tempfile::tempdir().unwrap();
    let mut app = app_in(tmp.path());
    app.open_settings();
    focus_on(&mut app, "frame_style");
    key(&mut app, Named::Enter);
    assert!(dialog(&app).popup.is_some());
    key(&mut app, Named::End);
    key(&mut app, Named::Enter);
    assert!(dialog(&app).popup.is_none());
    assert_eq!(app.settings.frame_style, FRAME_STYLES.last().unwrap().0);
    // Esc closes an open list, not the dialog.
    key(&mut app, Named::Enter);
    key(&mut app, Named::Escape);
    assert!(dialog(&app).popup.is_none());
    assert!(app.modal.is_some());
}

#[test]
fn restore_defaults_resets_the_page() {
    let tmp = tempfile::tempdir().unwrap();
    let mut app = app_in(tmp.path());
    app.update_settings(|s| {
        s.color_scheme = "nord".into();
        s.follow_system_appearance = true;
        s.dark_scheme = "dracula".into();
        s.font_size = 22;
        s.line_spacing = 1.4;
        s.frame_style = "double".into();
        s.font_style = "Bold".into();
        s.replay_gain = "album".into();
    });
    app.open_settings();
    focus_on(&mut app, "restore");
    key(&mut app, Named::Enter);
    let d = Settings::default();
    let s = &app.settings;
    assert_eq!(
        (&s.color_scheme, s.follow_system_appearance, &s.dark_scheme, s.font_size, s.line_spacing, &s.frame_style, &s.font_style),
        (&d.color_scheme, d.follow_system_appearance, &d.dark_scheme, d.font_size, d.line_spacing, &d.frame_style, &d.font_style)
    );
    assert_eq!(app.theme, crate::theme::Theme::new(&d.appearance(), app.scheme(&d.color_scheme)));
    assert_eq!(s.replay_gain, "album", "the Playback page has its own Restore defaults");
    key(&mut app, Named::Escape);
    app.open_settings();
    key(&mut app, Named::Down);
    focus_on(&mut app, "restore");
    key(&mut app, Named::Enter);
    assert_eq!(app.settings.replay_gain, d.replay_gain);
    assert_eq!(app.gain_mode, GainMode::Track);
}

#[test]
fn mouse_switches_pages_and_turns_numbers() {
    let tmp = tempfile::tempdir().unwrap();
    let mut app = app_in(tmp.path());
    app.open_settings();
    let r = rects(screen(&app));
    let size = dialog(&app).layout(&app, r.page).ctl("font_size").unwrap().0;
    click(&mut app, MouseKind::Scroll(1.0), size.col + 2, size.row);
    assert_eq!(app.settings.font_size, crate::settings::DEFAULT_FONT_SIZE + 1);
    assert_eq!(dialog(&app).focus, Focus::Ctl("font_size"));
    click(&mut app, MouseKind::Down, size.col, size.row);
    assert_eq!(app.settings.font_size, crate::settings::DEFAULT_FONT_SIZE, "a click on the left half turns it down");
    click(&mut app, MouseKind::Down, r.sections.col + 1, r.sections.row + 1);
    assert_eq!(dialog(&app).page, 1);
    let close = close_row().rects(r.buttons)[0];
    click(&mut app, MouseKind::Down, close.col, close.row);
    assert!(app.modal.is_none());
}

#[test]
fn scheme_editor_saves_renames_and_deletes_in_the_shared_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("color-schemes");
    let mut app = app_in(tmp.path());
    let monokai = themes::builtin_scheme("monokai").unwrap().clone();
    app.open_settings();

    // Editing a built-in scheme makes a copy under a new name.
    focus_on(&mut app, "scheme.edit");
    key(&mut app, Named::Enter);
    assert_eq!(editor(&app).name.text, "monokai (custom)");
    assert_eq!(editor(&app).original, None);
    // "Headers and titles" (role 4): ↓ goes Name, Dark, then color by color.
    for _ in 0..6 {
        key(&mut app, Named::Down);
    }
    assert_eq!(editor(&app).focus, scheme_editor::Focus::Hex(4));
    for _ in 0..7 {
        key(&mut app, Named::Backspace);
    }
    type_text(&mut app, "#123456");
    assert_eq!(app.theme.colors.accent, Rgba::rgb(0x12, 0x34, 0x56), "previewed on the player");
    assert_eq!(app.settings.color_scheme, "monokai", "not chosen before it is saved");
    key(&mut app, Named::Enter);
    assert!(dialog(&app).editor.is_none());
    assert_eq!(app.settings.color_scheme, "monokai (custom)");
    let saved = themes::all_schemes(&dir).into_iter().find(|s| s.name == "monokai (custom)").expect("saved");
    assert_eq!(saved.accent, "#123456");
    assert_eq!(saved.pygments_style, monokai.pygments_style, "fields the player doesn't use are kept");
    assert!(app.schemes.iter().any(|s| s.name == "monokai (custom)"));

    // A user scheme is edited in place, and may be renamed.
    key(&mut app, Named::Enter);
    assert_eq!(editor(&app).original.as_deref(), Some("monokai (custom)"));
    for _ in 0..20 {
        key(&mut app, Named::Backspace);
    }
    type_text(&mut app, "Mine");
    key(&mut app, Named::Enter);
    assert_eq!(app.settings.color_scheme, "Mine");
    let names: Vec<String> = themes::all_schemes(&dir).into_iter().filter(|s| !themes::is_builtin(&s.name)).map(|s| s.name).collect();
    assert_eq!(names, ["Mine"]);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "the old file is gone");

    // A taken name is refused with a message; Esc puts the saved colors back.
    focus_on(&mut app, "scheme.create");
    key(&mut app, Named::Enter);
    assert_eq!(editor(&app).name.text, "Mine (copy)");
    for _ in 0..20 {
        key(&mut app, Named::Backspace);
    }
    type_text(&mut app, "monokai");
    key(&mut app, Named::Enter);
    assert!(editor(&app).error.as_deref().is_some_and(|e| e.contains("built-in")), "{:?}", editor(&app).error);
    for _ in 0..2 {
        key(&mut app, Named::Down);
    }
    assert_eq!(editor(&app).focus, scheme_editor::Focus::Hex(0));
    for _ in 0..7 {
        key(&mut app, Named::Backspace);
    }
    type_text(&mut app, "#ff0000");
    assert_eq!(app.theme.colors.background, Rgba::rgb(255, 0, 0));
    key(&mut app, Named::Escape);
    assert!(dialog(&app).editor.is_none());
    assert_eq!(app.theme.colors.background, Rgba::hex(0x1e1f1c), "back to Mine's colors");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);

    // Deleting asks first (Cancel has the focus); the default takes over.
    focus_on(&mut app, "scheme.edit");
    key(&mut app, Named::Enter);
    key(&mut app, Named::Up); // Name → the buttons
    key(&mut app, Named::Right);
    key(&mut app, Named::Enter);
    assert!(editor(&app).confirm.is_some());
    key(&mut app, Named::Enter);
    assert!(editor(&app).confirm.is_none());
    assert!(dir.join("mine.toml").exists());
    key(&mut app, Named::Enter);
    key(&mut app, Named::Left);
    key(&mut app, Named::Enter);
    assert!(dialog(&app).editor.is_none());
    assert!(!dir.join("mine.toml").exists());
    assert_eq!(app.settings.color_scheme, Settings::default().color_scheme);
    assert!(!app.schemes.iter().any(|s| s.name == "Mine"));
}

#[test]
fn color_picker_sets_a_role() {
    let tmp = tempfile::tempdir().unwrap();
    let mut app = app_in(tmp.path());
    app.open_settings();
    focus_on(&mut app, "scheme.edit");
    key(&mut app, Named::Enter);
    // Tab: Name, Dark, then swatch and hex field of each color.
    key(&mut app, Named::Tab);
    key(&mut app, Named::Tab);
    assert_eq!(editor(&app).focus, scheme_editor::Focus::Swatch(0));
    app.handle_key(&KeyEvent::char(' '));
    assert!(editor(&app).picker.is_some());
    // The picker's hex field: Tab ×3 from the hue field.
    for _ in 0..3 {
        key(&mut app, Named::Tab);
    }
    for _ in 0..7 {
        key(&mut app, Named::Backspace);
    }
    type_text(&mut app, "#0a0b0c");
    key(&mut app, Named::Enter);
    assert!(editor(&app).picker.is_none());
    assert_eq!(editor(&app).scheme.background, "#0a0b0c");
    assert_eq!(app.theme.colors.background, Rgba::rgb(10, 11, 12));
}

#[test]
fn typed_numbers() {
    assert_eq!(typed_value("1", (7.0, 40.0), 0), Some(("1".into(), None)));
    assert_eq!(typed_value("14", (7.0, 40.0), 0), Some(("14".into(), Some(14.0))));
    assert_eq!(typed_value("140", (7.0, 40.0), 0), None);
    assert_eq!(typed_value("1.5", (7.0, 40.0), 0), None);
    assert_eq!(typed_value("1,25", (0.8, 2.0), 2), Some(("1.25".into(), Some(1.25))));
    assert_eq!(typed_value("1.255", (0.8, 2.0), 2), None);
}

#[test]
fn every_dialog_fits_small_windows() {
    let tmp = tempfile::tempdir().unwrap();
    for (cols, rows) in [(112, 36), (80, 24), (57, 18), (30, 10)] {
        let mut app = app_in(tmp.path());
        app.resized(cols, rows, (8.0, 16.0));
        app.open_settings();
        let mut f = Frame::new(cols, rows, app.theme.colors.background);
        crate::ui::draw(&app, &mut f);
        focus_on(&mut app, "scheme.edit");
        key(&mut app, Named::Enter);
        app.handle_key(&KeyEvent::named(Named::Tab));
        app.handle_key(&KeyEvent::named(Named::Tab));
        app.handle_key(&KeyEvent::char(' '));
        crate::ui::draw(&app, &mut f);
        let r = rects(screen(&app));
        assert!(right(r.outer) <= cols && bottom(r.outer) <= rows);
    }
}
