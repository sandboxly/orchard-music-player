//! The playlist: entries, the cursor, what plays next (shuffle, repeat), and
//! M3U / extended M3U files.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::meta::Tags;

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub path: PathBuf,
    /// From `#EXTINF` until the tags are read.
    pub title: Option<String>,
    pub duration: Option<f64>,
    pub tags: Option<Tags>,
}

impl Entry {
    pub fn new(path: PathBuf) -> Self {
        Entry { path, title: None, duration: None, tags: None }
    }

    pub fn display(&self) -> String {
        match (&self.tags, &self.title) {
            (Some(t), _) if t.title.is_some() || t.artist.is_some() => t.display(&self.path),
            (_, Some(title)) => title.clone(),
            _ => crate::meta::file_stem(&self.path),
        }
    }

    pub fn duration(&self) -> Option<f64> {
        self.tags.as_ref().and_then(|t| t.duration).or(self.duration)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

impl Repeat {
    pub fn next(self) -> Self {
        match self {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Repeat::Off => "off",
            Repeat::All => "all",
            Repeat::One => "one",
        }
    }

    pub fn from_label(s: &str) -> Self {
        match s {
            "all" => Repeat::All,
            "one" => Repeat::One,
            _ => Repeat::Off,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Playlist {
    pub entries: Vec<Entry>,
    /// The selected row.
    pub cursor: usize,
    /// The entry that plays (or is paused).
    pub current: Option<usize>,
    /// Where it was last saved / loaded from.
    pub file: Option<PathBuf>,
    /// Entries already played in this shuffle round.
    played: Vec<usize>,
    seed: u64,
}

impl Playlist {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn add(&mut self, paths: impl IntoIterator<Item = PathBuf>) -> usize {
        let before = self.entries.len();
        self.entries.extend(paths.into_iter().map(Entry::new));
        self.entries.len() - before
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.cursor = 0;
        self.current = None;
        self.played.clear();
        self.file = None;
    }

    /// Remove the entry at `index`, keeping the cursor and the current entry in place.
    pub fn remove(&mut self, index: usize) {
        if index >= self.entries.len() {
            return;
        }
        self.entries.remove(index);
        self.current = match self.current {
            Some(c) if c == index => None,
            Some(c) if c > index => Some(c - 1),
            c => c,
        };
        self.played.retain(|&i| i != index);
        for i in &mut self.played {
            if *i > index {
                *i -= 1;
            }
        }
        self.cursor = self.cursor.min(self.entries.len().saturating_sub(1));
    }

    /// Move the entry at `index` by `delta` rows; returns its new index.
    pub fn move_entry(&mut self, index: usize, delta: isize) -> usize {
        let n = self.entries.len();
        if index >= n {
            return index;
        }
        let to = (index as isize + delta).clamp(0, n as isize - 1) as usize;
        if to == index {
            return index;
        }
        let e = self.entries.remove(index);
        self.entries.insert(to, e);
        let fix = |i: usize| {
            if i == index {
                to
            } else if index < to && i > index && i <= to {
                i - 1
            } else if to < index && i >= to && i < index {
                i + 1
            } else {
                i
            }
        };
        self.current = self.current.map(fix);
        for p in &mut self.played {
            *p = fix(*p);
        }
        to
    }

    /// Total length of the entries whose length is known, and whether all are known.
    pub fn total_duration(&self) -> (f64, bool) {
        let mut all = true;
        let sum = self
            .entries
            .iter()
            .map(|e| e.duration().unwrap_or_else(|| {
                all = false;
                0.0
            }))
            .sum();
        (sum, all)
    }

    pub fn mark_played(&mut self, index: usize) {
        if !self.played.contains(&index) {
            self.played.push(index);
        }
    }

    /// The entry after the current one; `None` at the end without repeat.
    /// `user` is a press of Next: it skips even under repeat-one.
    pub fn next(&mut self, shuffle: bool, repeat: Repeat, user: bool) -> Option<usize> {
        let n = self.entries.len();
        if n == 0 {
            return None;
        }
        if repeat == Repeat::One && !user {
            return self.current.or(Some(self.cursor.min(n - 1)));
        }
        if shuffle {
            if let Some(c) = self.current {
                self.mark_played(c);
            }
            let mut left: Vec<usize> = (0..n).filter(|i| !self.played.contains(i)).collect();
            if left.is_empty() {
                if repeat == Repeat::Off && !user {
                    self.played.clear();
                    return None;
                }
                self.played.clear();
                left = (0..n).filter(|&i| Some(i) != self.current || n == 1).collect();
            }
            return Some(left[self.random(left.len())]);
        }
        match self.current {
            None => Some(0),
            Some(c) if c + 1 < n => Some(c + 1),
            Some(_) if repeat == Repeat::All || user && repeat != Repeat::Off => Some(0),
            Some(_) => None,
        }
    }

    pub fn previous(&mut self, shuffle: bool) -> Option<usize> {
        let n = self.entries.len();
        if n == 0 {
            return None;
        }
        if shuffle {
            // Back through the shuffle history.
            if let Some(c) = self.current {
                self.played.retain(|&i| i != c);
            }
            if let Some(p) = self.played.pop() {
                return Some(p);
            }
        }
        Some(match self.current {
            Some(c) if c > 0 => c - 1,
            Some(_) => n - 1,
            None => 0,
        })
    }

    /// xorshift: no need for a crate to pick a random track.
    fn random(&mut self, below: usize) -> usize {
        if self.seed == 0 {
            self.seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0x9e37_79b9_7f4a_7c15, |d| d.as_nanos() as u64)
                | 1;
        }
        let mut x = self.seed;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.seed = x;
        (x % below.max(1) as u64) as usize
    }
}

// ---------------------------------------------------------------------------
// M3U
// ---------------------------------------------------------------------------

/// Read an M3U / M3U8 file: relative paths are relative to its folder;
/// `#EXTINF:<seconds>,<title>` gives the length and title until tags are read.
/// URLs and missing files are kept out.
pub fn load_m3u(path: &Path) -> io::Result<Vec<Entry>> {
    let bytes = fs::read(path)?;
    let text = String::from_utf8(bytes).unwrap_or_else(|e| e.into_bytes().iter().map(|&b| b as char).collect());
    let base = path.parent().unwrap_or(Path::new("."));
    Ok(parse_m3u(&text, base))
}

pub fn parse_m3u(text: &str, base: &Path) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut info: Option<(Option<f64>, Option<String>)> = None;
    for line in text.lines() {
        let line = line.trim_start_matches('\u{feff}').trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("#EXTINF:") {
            let (secs, title) = rest.split_once(',').unwrap_or((rest, ""));
            // "#EXTINF:123 tvg-id=…,Title": the length is the first word.
            let secs = secs.split_whitespace().next().and_then(|s| s.parse::<f64>().ok()).filter(|&s| s > 0.0);
            let title = Some(title.trim().to_string()).filter(|t| !t.is_empty());
            info = Some((secs, title));
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        if line.contains("://") && !line.starts_with("file://") {
            info = None;
            continue;
        }
        let raw = line.strip_prefix("file://").unwrap_or(line);
        let raw = percent_decode(raw, line.starts_with("file://"));
        let p = PathBuf::from(raw.replace('\\', std::path::MAIN_SEPARATOR_STR));
        let p = if p.is_absolute() { p } else { base.join(p) };
        let mut e = Entry::new(p);
        if let Some((secs, title)) = info.take() {
            e.duration = secs;
            e.title = title;
        }
        out.push(e);
    }
    out
}

fn percent_decode(s: &str, enabled: bool) -> String {
    if !enabled || !s.contains('%') {
        return s.to_string();
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len()
            && let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Extended M3U, UTF-8. Paths inside the playlist's folder are written
/// relative, so a music folder can move with its playlists.
pub fn to_m3u(entries: &[Entry], playlist_path: &Path) -> String {
    let base = playlist_path.parent();
    let mut out = String::from("#EXTM3U\n");
    for e in entries {
        let secs = e.duration().map_or(-1, |d| d.round() as i64);
        out.push_str(&format!("#EXTINF:{secs},{}\n", e.display()));
        let rel = base.and_then(|b| e.path.strip_prefix(b).ok()).unwrap_or(&e.path);
        out.push_str(&rel.to_string_lossy());
        out.push('\n');
    }
    out
}

pub fn save_m3u(entries: &[Entry], path: &Path) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("m3u.tmp");
    fs::write(&tmp, to_m3u(entries, path))?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(n: usize) -> Playlist {
        let mut p = Playlist::default();
        p.add((0..n).map(|i| PathBuf::from(format!("/m/{i}.mp3"))));
        p
    }

    #[test]
    fn m3u_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let pl = dir.path().join("list.m3u");
        let mut a = Entry::new(dir.path().join("sub/a.mp3"));
        a.title = Some("Band – Song".into());
        a.duration = Some(201.4);
        // Absolute on this platform (on Windows, a path without a drive isn't).
        let elsewhere = PathBuf::from(if cfg!(windows) { r"C:\elsewhere\b.ogg" } else { "/elsewhere/b.ogg" });
        let b = Entry::new(elsewhere.clone());
        save_m3u(&[a, b], &pl).unwrap();
        let text = fs::read_to_string(&pl).unwrap();
        assert!(text.starts_with("#EXTM3U\n#EXTINF:201,Band – Song\nsub/a.mp3\n"));
        let back = load_m3u(&pl).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].path, dir.path().join("sub/a.mp3"));
        assert_eq!(back[0].title.as_deref(), Some("Band – Song"));
        assert_eq!(back[0].duration, Some(201.0));
        assert_eq!(back[1].path, elsewhere);
        assert_eq!(back[1].duration, None);
    }

    #[test]
    fn m3u_skips_urls_and_decodes_file_urls() {
        let e = parse_m3u("#EXTM3U\nhttp://radio/x\nfile:///a/b%20c.mp3\n# comment\nrel.flac\n", Path::new("/base"));
        let paths: Vec<_> = e.iter().map(|e| e.path.clone()).collect();
        assert_eq!(paths, [PathBuf::from("/a/b c.mp3"), PathBuf::from("/base/rel.flac")]);
    }

    #[test]
    fn sequential_next_and_repeat() {
        let mut p = list(3);
        assert_eq!(p.next(false, Repeat::Off, false), Some(0));
        p.current = Some(2);
        assert_eq!(p.next(false, Repeat::Off, false), None);
        assert_eq!(p.next(false, Repeat::All, false), Some(0));
        assert_eq!(p.next(false, Repeat::One, false), Some(2));
        assert_eq!(p.next(false, Repeat::One, true), Some(0));
        assert_eq!(p.previous(false), Some(1));
    }

    #[test]
    fn shuffle_plays_each_once() {
        let mut p = list(5);
        let mut seen = Vec::new();
        while let Some(i) = p.next(true, Repeat::Off, false) {
            assert!(!seen.contains(&i), "{i} twice in {seen:?}");
            seen.push(i);
            p.current = Some(i);
        }
        assert_eq!(seen.len(), 5);
    }

    #[test]
    fn remove_and_move_keep_current() {
        let mut p = list(4);
        p.current = Some(2);
        p.remove(0);
        assert_eq!(p.current, Some(1));
        let to = p.move_entry(1, -1);
        assert_eq!((to, p.current), (0, Some(0)));
        p.move_entry(2, -2);
        assert_eq!(p.current, Some(1));
        p.remove(1);
        assert_eq!(p.current, None);
    }
}
