//! The file browser dialog (F2 Open, F5 Load): folders and the files the
//! player can use, marking, quick search by typing.
//!
//! On Windows ".." from a drive root (C:\) leads to "Drives", a virtual
//! folder listing the drives (C:, D:, …), like Orchard Commander's. It is
//! the empty path here ([`is_drives`]). Drives are only opened, never
//! marked or added whole, so nothing scans a drive by accident.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::meta::{is_audio, is_playlist};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Add music (files, folders, playlists' contents) to the playlist.
    Add,
    /// Replace the playlist with an M3U file.
    LoadPlaylist,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

/// What a key in the browser asks the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    None,
    Close,
    /// List this folder.
    Open(PathBuf),
    /// Add these files / folders (recursively) and close.
    Add(Vec<PathBuf>),
    Load(PathBuf),
}

#[derive(Clone, Debug)]
pub struct Browser {
    pub mode: Mode,
    pub dir: PathBuf,
    /// `None` while listing.
    pub items: Option<Vec<Item>>,
    pub error: Option<String>,
    pub cursor: usize,
    pub top: usize,
    pub marked: BTreeSet<usize>,
    search: String,
    search_at: Option<Instant>,
    /// Select this entry when the listing arrives (the folder we came from).
    select: Option<PathBuf>,
}

impl Browser {
    pub fn new(mode: Mode, dir: PathBuf) -> Self {
        Browser {
            mode,
            dir,
            items: None,
            error: None,
            cursor: 0,
            top: 0,
            marked: BTreeSet::new(),
            search: String::new(),
            search_at: None,
            select: None,
        }
    }

    pub fn title(&self) -> &'static str {
        match self.mode {
            Mode::Add => "Open",
            Mode::LoadPlaylist => "Load playlist",
        }
    }

    /// Rows: ".." (unless at the root), then the items.
    pub fn rows(&self) -> usize {
        self.items.as_ref().map_or(0, |i| i.len()) + usize::from(self.has_parent())
    }

    pub fn has_parent(&self) -> bool {
        parent(&self.dir).is_some()
    }

    /// "Drives", or the folder's path.
    pub fn place(&self) -> String {
        if is_drives(&self.dir) { DRIVES.to_string() } else { self.dir.display().to_string() }
    }

    /// The item at row `r` (`None` for "..").
    pub fn item(&self, r: usize) -> Option<&Item> {
        let i = r.checked_sub(usize::from(self.has_parent()))?;
        self.items.as_ref()?.get(i)
    }

    pub fn going_to(&mut self, dir: PathBuf) {
        self.select = (Some(&dir) == parent(&self.dir).as_ref()).then(|| self.dir.clone());
        self.dir = dir;
        self.items = None;
        self.error = None;
        self.marked.clear();
        self.cursor = 0;
        self.top = 0;
    }

    pub fn listed(&mut self, result: Result<Vec<Item>, String>) {
        match result {
            Ok(items) => {
                self.cursor = self
                    .select
                    .take()
                    .and_then(|s| items.iter().position(|i| i.path == s))
                    .map_or(0, |i| i + usize::from(self.has_parent()));
                self.items = Some(items);
            }
            Err(e) => {
                self.error = Some(e);
                self.items = Some(Vec::new());
            }
        }
    }

    pub fn scroll_into_view(&mut self, height: usize) {
        let h = height.max(1);
        if self.cursor < self.top {
            self.top = self.cursor;
        } else if self.cursor >= self.top + h {
            self.top = self.cursor + 1 - h;
        }
        self.top = self.top.min(self.rows().saturating_sub(h));
    }

    pub fn move_cursor(&mut self, delta: isize) {
        let n = self.rows();
        if n == 0 {
            return;
        }
        self.cursor = (self.cursor as isize + delta).clamp(0, n as isize - 1) as usize;
    }

    /// Enter on the cursor row.
    pub fn activate(&self) -> Action {
        if self.cursor == 0 && self.has_parent() {
            return parent(&self.dir).map_or(Action::None, Action::Open);
        }
        let Some(item) = self.item(self.cursor) else { return Action::None };
        if !self.marked.is_empty() && self.mode == Mode::Add {
            return Action::Add(self.marked.iter().filter_map(|&r| self.item(r)).map(|i| i.path.clone()).collect());
        }
        match (item.is_dir, self.mode) {
            (true, _) => Action::Open(item.path.clone()),
            (false, Mode::Add) => Action::Add(vec![item.path.clone()]),
            (false, Mode::LoadPlaylist) => Action::Load(item.path.clone()),
        }
    }

    /// Add the folder under the cursor, or the one shown.
    pub fn add_folder(&self) -> Action {
        if self.mode != Mode::Add || is_drives(&self.dir) {
            return Action::None;
        }
        match self.item(self.cursor) {
            Some(i) if i.is_dir => Action::Add(vec![i.path.clone()]),
            _ => Action::Add(vec![self.dir.clone()]),
        }
    }

    pub fn toggle_mark(&mut self) {
        if self.item(self.cursor).is_some() && !is_drives(&self.dir) && !self.marked.remove(&self.cursor) {
            self.marked.insert(self.cursor);
        }
        self.move_cursor(1);
    }

    /// Mark every file (not the folders); again to unmark all.
    pub fn mark_all(&mut self) {
        let files: BTreeSet<usize> = (0..self.rows()).filter(|&r| self.item(r).is_some_and(|i| !i.is_dir)).collect();
        if self.marked == files {
            self.marked.clear();
        } else {
            self.marked = files;
        }
    }

    /// Typing jumps to the first entry starting with what was typed.
    pub fn search(&mut self, text: &str, now: Instant) {
        if self.search_at.is_none_or(|t| now.duration_since(t) > Duration::from_millis(900)) {
            self.search.clear();
        }
        self.search_at = Some(now);
        self.search.push_str(&text.to_lowercase());
        let found = (0..self.rows()).find(|&r| self.item(r).is_some_and(|i| i.name.to_lowercase().starts_with(&self.search)));
        if let Some(r) = found {
            self.cursor = r;
        }
    }
}

/// The name of the virtual folder of drives.
pub const DRIVES: &str = "Drives";

/// The virtual folder of drives (Windows).
pub fn is_drives(dir: &Path) -> bool {
    dir.as_os_str().is_empty()
}

/// Where ".." leads: the parent folder; from a Windows drive root, Drives.
pub fn parent(dir: &Path) -> Option<PathBuf> {
    parent_on(dir, cfg!(windows))
}

fn parent_on(dir: &Path, windows: bool) -> Option<PathBuf> {
    if is_drives(dir) {
        return None;
    }
    if windows && drive_letter(dir).is_some() {
        return Some(PathBuf::new());
    }
    dir.parent().map(Path::to_path_buf)
}

/// "C" for the drive root "C:\" (or "C:", "C:/"); `None` for anything else.
fn drive_letter(dir: &Path) -> Option<char> {
    let s = dir.to_str()?;
    let mut chars = s.chars();
    let letter = chars.next().filter(char::is_ascii_alphabetic)?;
    (chars.next() == Some(':') && matches!(chars.as_str(), "" | "\\" | "/")).then(|| letter.to_ascii_uppercase())
}

/// The drives as folders "C:" … (their roots aren't touched: an empty card
/// reader can make that hang).
fn drives() -> Vec<Item> {
    drive_letters().into_iter().map(|l| Item { name: format!("{l}:"), path: PathBuf::from(format!("{l}:\\")), is_dir: true }).collect()
}

#[cfg(windows)]
fn drive_letters() -> Vec<char> {
    // SAFETY: no arguments; returns a bit mask of the drive letters in use.
    let mask = unsafe { windows_sys::Win32::Storage::FileSystem::GetLogicalDrives() };
    (0..26u8).filter(|i| mask & (1 << i) != 0).map(|i| (b'A' + i) as char).collect()
}

#[cfg(not(windows))]
fn drive_letters() -> Vec<char> {
    Vec::new()
}

/// The folders and usable files in `dir`, folders first, by name (blocking).
pub fn list(dir: &Path, mode: Mode) -> Result<Vec<Item>, String> {
    if is_drives(dir) {
        return Ok(drives());
    }
    let rd = std::fs::read_dir(dir).map_err(|e| crate::meta::io_reason(&e))?;
    let mut items: Vec<Item> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            let path = e.path();
            // Follows symlinks: a linked music folder is a folder.
            let is_dir = path.is_dir();
            let usable = is_dir
                || match mode {
                    Mode::Add => is_audio(&path) || is_playlist(&path),
                    Mode::LoadPlaylist => is_playlist(&path),
                };
            usable.then_some(Item { name, path, is_dir })
        })
        .collect();
    items.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| natural_cmp(&a.name, &b.name)));
    Ok(items)
}

/// Audio files under `paths` (folders recursively, playlists expanded), in
/// natural order per folder (blocking).
pub fn expand(paths: &[PathBuf]) -> Vec<crate::playlist::Entry> {
    let mut out = Vec::new();
    for p in paths {
        if p.is_dir() {
            walk(p, &mut out, 0);
        } else if is_playlist(p) {
            out.extend(crate::playlist::load_m3u(p).unwrap_or_default());
        } else if is_audio(p) {
            out.push(crate::playlist::Entry::new(p.clone()));
        }
    }
    out
}

fn walk(dir: &Path, out: &mut Vec<crate::playlist::Entry>, depth: usize) {
    if depth > 32 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| e.path())
        .collect();
    entries.sort_by(|a, b| natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
    let (dirs, files): (Vec<_>, Vec<_>) = entries.into_iter().partition(|p| p.is_dir());
    out.extend(files.into_iter().filter(|p| is_audio(p)).map(crate::playlist::Entry::new));
    for d in dirs {
        walk(&d, out, depth + 1);
    }
}

/// Case-insensitive, with digit runs compared as numbers ("2 b" < "10 a").
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(p), Some(q)) if p.is_ascii_digit() && q.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        s.push(c);
                        it.next();
                    }
                    s
                };
                let (m, n) = (take(&mut x), take(&mut y));
                let (mt, nt) = (m.trim_start_matches('0'), n.trim_start_matches('0'));
                let o = mt.len().cmp(&nt.len()).then_with(|| mt.cmp(nt));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(p), Some(q)) => {
                let o = p.to_lowercase().cmp(q.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                x.next();
                y.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn drive_roots_lead_to_drives_on_windows() {
        for root in ["C:\\", "c:", "D:/"] {
            assert_eq!(parent_on(Path::new(root), true), Some(PathBuf::new()), "{root}");
        }
        assert_eq!(drive_letter(Path::new("c:\\")), Some('C'));
        assert_eq!(drive_letter(Path::new("C:\\Music")), None);
        assert_eq!(drive_letter(Path::new("/Users")), None);
        assert_eq!(parent_on(Path::new(""), true), None, "Drives has no parent");
        assert_eq!(parent_on(Path::new("/"), false), None);
        assert_eq!(parent_on(Path::new("/Users/me"), false), Some(PathBuf::from("/Users")));
    }

    fn drives_browser() -> Browser {
        let mut b = Browser::new(Mode::Add, PathBuf::from("D:\\"));
        b.going_to(PathBuf::new());
        let drive = |l: &str| Item { name: format!("{l}:"), path: PathBuf::from(format!("{l}:\\")), is_dir: true };
        b.listed(Ok(vec![drive("C"), drive("D"), drive("E")]));
        b
    }

    #[test]
    fn drives_lists_the_drives_without_a_way_up() {
        let b = drives_browser();
        assert_eq!(b.place(), "Drives");
        assert!(!b.has_parent());
        assert_eq!(b.rows(), 3, "no '..'");
        assert_eq!(b.item(0).map(|i| i.name.as_str()), Some("C:"));
        // It came up from D:\: the cursor is on D:.
        assert_eq!(b.cursor, 1);
    }

    #[test]
    fn drives_are_opened_never_added_whole() {
        let mut b = drives_browser();
        b.toggle_mark();
        assert!(b.marked.is_empty(), "Space doesn't mark a drive");
        b.mark_all();
        assert!(b.marked.is_empty());
        assert_eq!(b.add_folder(), Action::None, "F3 doesn't add a drive");
        b.cursor = 1;
        assert_eq!(b.activate(), Action::Open(PathBuf::from("D:\\")), "Enter opens it");
    }

    #[test]
    fn natural_order() {
        let mut v = vec!["10 b.mp3", "2 a.mp3", "1 c.mp3", "B.mp3", "a.mp3"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["1 c.mp3", "2 a.mp3", "10 b.mp3", "a.mp3", "B.mp3"]);
    }

    #[test]
    fn lists_and_expands() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir(d.path().join("CD2")).unwrap();
        fs::write(d.path().join("CD2/01.flac"), b"").unwrap();
        fs::write(d.path().join("02.mp3"), b"").unwrap();
        fs::write(d.path().join("10.mp3"), b"").unwrap();
        fs::write(d.path().join("notes.txt"), b"").unwrap();
        fs::write(d.path().join("list.m3u"), "10.mp3\n").unwrap();
        let names: Vec<_> = list(d.path(), Mode::Add).unwrap().into_iter().map(|i| i.name).collect();
        assert_eq!(names, ["CD2", "02.mp3", "10.mp3", "list.m3u"]);
        let names: Vec<_> = list(d.path(), Mode::LoadPlaylist).unwrap().into_iter().map(|i| i.name).collect();
        assert_eq!(names, ["CD2", "list.m3u"]);
        let all: Vec<_> = expand(&[d.path().to_path_buf()]).into_iter().map(|e| e.path).collect();
        assert_eq!(all, [d.path().join("02.mp3"), d.path().join("10.mp3"), d.path().join("CD2/01.flac")]);
    }

    #[test]
    fn enter_on_marked_adds_them() {
        let mut b = Browser::new(Mode::Add, PathBuf::from("/m"));
        let item = |n: &str, d| Item { name: n.into(), path: PathBuf::from("/m").join(n), is_dir: d };
        b.listed(Ok(vec![item("x", true), item("a.mp3", false), item("b.mp3", false)]));
        assert_eq!(b.cursor, 0);
        assert_eq!(b.activate(), Action::Open(PathBuf::from("/")));
        b.mark_all();
        assert_eq!(b.marked.len(), 2);
        b.cursor = 1;
        assert_eq!(b.activate(), Action::Add(vec![PathBuf::from("/m/a.mp3"), PathBuf::from("/m/b.mp3")]));
    }
}
