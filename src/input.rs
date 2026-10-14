//! Window-system-neutral input events (the binary converts winit's).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Named {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    Enter,
    Escape,
    Backspace,
    Delete,
    Insert,
    Tab,
    F(u8),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    /// A character key, as on the key cap without modifiers (lower case).
    Char(char),
    Named(Named),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// ⌘ / the Windows key.
    pub logo: bool,
}

impl Mods {
    /// ⌘ on macOS, Ctrl elsewhere.
    pub fn primary(&self) -> bool {
        if cfg!(target_os = "macos") { self.logo } else { self.ctrl }
    }

    pub fn none(&self) -> bool {
        !(self.shift || self.ctrl || self.alt || self.logo)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Key,
    pub mods: Mods,
    /// The text the key types (with Shift / Option applied), if any.
    pub text: Option<String>,
}

impl KeyEvent {
    pub fn named(n: Named) -> Self {
        KeyEvent { key: Key::Named(n), mods: Mods::default(), text: None }
    }

    pub fn char(c: char) -> Self {
        KeyEvent { key: Key::Char(c.to_ascii_lowercase()), mods: Mods::default(), text: Some(c.to_string()) }
    }

    pub fn with(mut self, mods: Mods) -> Self {
        self.mods = mods;
        self
    }

    pub fn is(&self, n: Named) -> bool {
        self.key == Key::Named(n)
    }

    /// Typed printable text without command modifiers.
    pub fn typed(&self) -> Option<&str> {
        if self.mods.ctrl || self.mods.logo {
            return None;
        }
        self.text.as_deref().filter(|t| !t.is_empty() && t.chars().all(|c| !c.is_control()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MouseKind {
    Down,
    Up,
    /// Moved with the left button held.
    Drag,
    /// Wheel: positive is up / away, in rows.
    Scroll(f32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MouseEvent {
    pub kind: MouseKind,
    pub col: u16,
    pub row: u16,
    /// Pixel position, for pixel-exact bars.
    pub x: f32,
    pub y: f32,
    /// The second click of a double click (on `Down`).
    pub double: bool,
}
