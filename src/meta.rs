//! What a track is: tags for the playlist (oc-rename's `AudioTags` provider,
//! the same reader Orchard Commander's multi-rename uses) and, for the track
//! that plays, the stream details and cover art (lofty).

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;

use lofty::config::ParseOptions;
use lofty::prelude::*;
use lofty::probe::Probe;
use oc_rename::metadata::{AudioTags, MetadataProvider, Source};

/// Extensions the player opens (what symphonia decodes).
pub const AUDIO_EXTENSIONS: &[&str] = &["mp3", "ogg", "oga", "flac", "wav", "m4a", "m4b", "aac", "mp4", "aiff", "aif", "caf", "mka", "webm", "opus"];
pub const PLAYLIST_EXTENSIONS: &[&str] = &["m3u", "m3u8"];

fn has_ext(path: &Path, list: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| list.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

pub fn is_audio(path: &Path) -> bool {
    has_ext(path, AUDIO_EXTENSIONS)
}

pub fn is_playlist(path: &Path) -> bool {
    has_ext(path, PLAYLIST_EXTENSIONS)
}

/// Tags of one file; missing ones are None.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub year: Option<String>,
    pub genre: Option<String>,
    pub track: Option<String>,
    /// Whole seconds.
    pub duration: Option<f64>,
    /// kbit/s.
    pub bitrate: Option<u32>,
}

impl Tags {
    /// "Artist – Title", or what there is of it.
    pub fn display(&self, path: &Path) -> String {
        let title = self.title.clone().unwrap_or_else(|| file_stem(path));
        match &self.artist {
            Some(a) => format!("{a} – {title}"),
            None => title,
        }
    }
}

/// Why a file operation failed, in words for the user: "you don't have
/// permission", "not found". Other errors keep the system's description,
/// without " (os error N)".
pub fn io_reason(e: &std::io::Error) -> String {
    use std::io::ErrorKind as K;
    match e.kind() {
        K::NotFound => "it doesn't exist (anymore)".into(),
        K::PermissionDenied => "you don't have permission".into(),
        K::StorageFull => "the disk is full".into(),
        K::ReadOnlyFilesystem => "the disk is read-only".into(),
        K::IsADirectory => "it's a folder".into(),
        _ => {
            let text = e.to_string();
            let text = text.rfind(" (os error ").map_or(text.as_str(), |i| &text[..i]).to_string();
            let mut chars = text.chars();
            match chars.next() {
                Some(first) => first.to_lowercase().chain(chars).collect(),
                None => text,
            }
        }
    }
}

pub fn file_stem(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

/// The tags of `path` (blocking; call from a worker).
pub fn read_tags(path: &Path) -> Tags {
    let Ok(mut file) = File::open(path) else { return Tags::default() };
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut src = Source { name: &name, local_path: Some(path), file: &mut file };
    let Ok(mut m) = AudioTags.read(&mut src) else { return Tags::default() };
    let mut take = |k: &str| m.remove(k).filter(|v| !v.is_empty());
    Tags {
        title: take("title"),
        artist: take("artist").or_else(|| take("albumartist")),
        album: take("album"),
        year: take("year"),
        genre: take("genre"),
        track: take("track"),
        duration: take("duration").and_then(|d| parse_duration(&d)),
        bitrate: take("bitrate").and_then(|b| b.parse().ok()),
    }
}

/// oc-rename's "1h02m03s" / "3m25s" / "45s" back to seconds.
fn parse_duration(text: &str) -> Option<f64> {
    let mut total = 0u64;
    let mut num = String::new();
    for c in text.chars() {
        if c.is_ascii_digit() {
            num.push(c);
            continue;
        }
        let n: u64 = num.parse().ok()?;
        num.clear();
        total += n * match c {
            'h' => 3600,
            'm' => 60,
            's' => 1,
            _ => return None,
        };
    }
    num.is_empty().then_some(total as f64)
}

/// Which ReplayGain value evens out the loudness.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GainMode {
    Off,
    /// Every track at the same loudness.
    #[default]
    Track,
    /// Albums keep their own dynamics (falls back to the track gain).
    Album,
}

impl GainMode {
    pub fn next(self) -> Self {
        match self {
            GainMode::Off => GainMode::Track,
            GainMode::Track => GainMode::Album,
            GainMode::Album => GainMode::Off,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            GainMode::Off => "off",
            GainMode::Track => "track",
            GainMode::Album => "album",
        }
    }

    pub fn from_label(s: &str) -> Self {
        match s {
            "off" => GainMode::Off,
            "album" => GainMode::Album,
            _ => GainMode::Track,
        }
    }

    pub fn to_u8(self) -> u8 {
        self as u8
    }

    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => GainMode::Off,
            2 => GainMode::Album,
            _ => GainMode::Track,
        }
    }
}

/// A file's ReplayGain tags (dB, and linear peaks).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ReplayGain {
    pub track_gain: Option<f32>,
    pub track_peak: Option<f32>,
    pub album_gain: Option<f32>,
    pub album_peak: Option<f32>,
}

impl ReplayGain {
    /// The gain in dB for `mode`, lowered so the peak doesn't clip; `None`
    /// without a usable tag.
    pub fn db(&self, mode: GainMode) -> Option<f32> {
        let (gain, peak) = match mode {
            GainMode::Off => return None,
            GainMode::Track => (self.track_gain, self.track_peak),
            GainMode::Album => match self.album_gain {
                Some(g) => (Some(g), self.album_peak),
                None => (self.track_gain, self.track_peak),
            },
        };
        let gain = gain?;
        let ceiling = peak.filter(|&p| p > 0.0).map_or(f32::INFINITY, |p| -20.0 * p.log10());
        Some(gain.min(ceiling))
    }

    /// The linear factor for `mode` (1.0 without a tag).
    pub fn factor(&self, mode: GainMode) -> f32 {
        self.db(mode).map_or(1.0, |db| 10f32.powf(db / 20.0))
    }
}

/// "-6.20 dB" / "+1.5 dB" / "0.98" -> the number.
fn parse_number(s: &str) -> Option<f32> {
    let t = s.trim().trim_end_matches(|c: char| c.is_alphabetic() || c.is_whitespace());
    t.trim_start_matches('+').parse::<f32>().ok().filter(|v| v.is_finite())
}

/// The ReplayGain tags of `path` (blocking; the decoder thread reads them
/// when it opens a track). Opus' R128 gains are converted (they are relative
/// to −23 LUFS, ReplayGain to −18).
pub fn read_replay_gain(path: &Path) -> ReplayGain {
    let Ok(file) = File::open(path) else { return ReplayGain::default() };
    let options = ParseOptions::new().read_properties(false).read_cover_art(false);
    let Ok(probe) = Probe::new(BufReader::new(file)).options(options).guess_file_type() else {
        return ReplayGain::default();
    };
    let Ok(tagged) = probe.read() else { return ReplayGain::default() };
    let mut rg = ReplayGain::default();
    for tag in tagged.tags() {
        let get = |k: ItemKey| tag.get_string(k).and_then(parse_number);
        rg.track_gain = rg.track_gain.or_else(|| get(ItemKey::ReplayGainTrackGain));
        rg.track_peak = rg.track_peak.or_else(|| get(ItemKey::ReplayGainTrackPeak));
        rg.album_gain = rg.album_gain.or_else(|| get(ItemKey::ReplayGainAlbumGain));
        rg.album_peak = rg.album_peak.or_else(|| get(ItemKey::ReplayGainAlbumPeak));
    }
    if tagged.file_type() == lofty::file::FileType::Opus && (rg.track_gain.is_none() || rg.album_gain.is_none()) {
        let (track, album) = r128_gains(path);
        rg.track_gain = rg.track_gain.or(track);
        rg.album_gain = rg.album_gain.or(album);
    }
    rg
}

/// Opus' `R128_TRACK_GAIN` / `R128_ALBUM_GAIN` (Q7.8 dB against −23 LUFS)
/// as ReplayGain dB; lofty's generic tag leaves these keys out.
fn r128_gains(path: &Path) -> (Option<f32>, Option<f32>) {
    let Ok(mut file) = File::open(path) else { return (None, None) };
    let options = ParseOptions::new().read_properties(false);
    let Ok(opus) = lofty::ogg::OpusFile::read_from(&mut file, options) else { return (None, None) };
    let vc = opus.vorbis_comments();
    let get = |k: &str| vc.get(k).and_then(|v| v.trim().parse::<i16>().ok()).map(|q| q as f32 / 256.0 + 5.0);
    (get("R128_TRACK_GAIN"), get("R128_ALBUM_GAIN"))
}

/// Decoded cover art, scaled down for the info panel.
#[derive(Clone, Debug, PartialEq)]
pub struct Cover {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
}

/// What the now-playing panel shows beyond the tags.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Details {
    pub sample_rate: Option<u32>,
    pub channels: Option<u8>,
    pub bit_depth: Option<u8>,
    pub bitrate: Option<u32>,
    pub cover: Option<Cover>,
}

/// Longest side of the cover image kept in memory.
const COVER_PX: u32 = 320;

/// Stream details and cover art of `path` (blocking; call from a worker).
pub fn read_details(path: &Path) -> Details {
    let Ok(file) = File::open(path) else { return Details::default() };
    let options = ParseOptions::new().read_properties(true).read_cover_art(true);
    let Ok(probe) = Probe::new(BufReader::new(file)).options(options).guess_file_type() else {
        return Details::default();
    };
    let Ok(tagged) = probe.read() else { return Details::default() };
    let p = tagged.properties();
    let cover = tagged
        .tags()
        .iter()
        .flat_map(|t| t.pictures())
        .find(|pic| pic.pic_type() == lofty::picture::PictureType::CoverFront)
        .or_else(|| tagged.tags().iter().flat_map(|t| t.pictures()).next())
        .and_then(|pic| decode_cover(pic.data()))
        .or_else(|| folder_cover(path));
    Details {
        sample_rate: p.sample_rate(),
        channels: p.channels(),
        bit_depth: p.bit_depth(),
        bitrate: p.audio_bitrate().or(p.overall_bitrate()).filter(|&b| b > 0),
        cover,
    }
}

/// cover.jpg / folder.png… next to the file.
fn folder_cover(path: &Path) -> Option<Cover> {
    let dir = path.parent()?;
    for name in ["cover", "folder", "front", "album", "Cover", "Folder", "Front", "AlbumArt"] {
        for ext in ["jpg", "jpeg", "png", "webp"] {
            let p = dir.join(format!("{name}.{ext}"));
            if let Ok(bytes) = std::fs::read(&p)
                && let Some(c) = decode_cover(&bytes) {
                    return Some(c);
                }
        }
    }
    None
}

fn decode_cover(bytes: &[u8]) -> Option<Cover> {
    let img = image::load_from_memory(bytes).ok()?;
    let img = if img.width() > COVER_PX || img.height() > COVER_PX {
        img.resize(COVER_PX, COVER_PX, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    Some(Cover { width: rgba.width(), height: rgba.height(), rgba: Arc::new(rgba.into_raw()) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_parse_back() {
        assert_eq!(parse_duration("45s"), Some(45.0));
        assert_eq!(parse_duration("3m25s"), Some(205.0));
        assert_eq!(parse_duration("1h02m03s"), Some(3723.0));
        assert_eq!(parse_duration("abc"), None);
    }

    /// Tags and cover of the files in `$OMP_TEST_MUSIC`, printed.
    #[test]
    fn reads_test_music() {
        let Some(dir) = std::env::var_os("OMP_TEST_MUSIC") else { return };
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            let t = read_tags(&p);
            let d = read_details(&p);
            eprintln!(
                "{}: {:?} / {:?} {:?}s {:?} kbps {:?} Hz {:?} ch cover {:?}",
                p.file_name().unwrap().to_string_lossy(),
                t.artist,
                t.title,
                t.duration,
                d.bitrate,
                d.sample_rate,
                d.channels,
                d.cover.as_ref().map(|c| (c.width, c.height))
            );
            eprintln!("    replay gain: {:?}", read_replay_gain(&p));
        }
    }

    #[test]
    fn replay_gain_math() {
        let rg = ReplayGain { track_gain: Some(-6.0), track_peak: Some(0.5), album_gain: Some(4.0), album_peak: Some(0.9) };
        assert_eq!(rg.db(GainMode::Off), None);
        assert_eq!(rg.db(GainMode::Track), Some(-6.0));
        // +4 dB would clip a 0.9 peak: held at about +0.92 dB.
        let album = rg.db(GainMode::Album).unwrap();
        assert!((album - 0.915).abs() < 0.01, "{album}");
        assert!((rg.factor(GainMode::Track) - 0.501).abs() < 0.001);
        assert_eq!(ReplayGain::default().factor(GainMode::Track), 1.0);
        let track_only = ReplayGain { track_gain: Some(-3.0), ..ReplayGain::default() };
        assert_eq!(track_only.db(GainMode::Album), Some(-3.0));
        assert_eq!(parse_number("-6.20 dB"), Some(-6.2));
        assert_eq!(parse_number("+1.5 dB"), Some(1.5));
        assert_eq!(parse_number("0.988"), Some(0.988));
    }

    #[test]
    fn extensions() {
        assert!(is_audio(Path::new("/a/B.MP3")));
        assert!(is_audio(Path::new("x.ogg")));
        assert!(!is_audio(Path::new("x.txt")));
        assert!(is_playlist(Path::new("x.m3u8")));
    }
}
