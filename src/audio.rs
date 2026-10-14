//! Playback: a decoder thread (symphonia) fills a buffer of interleaved f32
//! samples in the device's format; the device callback (cpal) plays from it,
//! applies the volume and keeps the latest samples for the visualizer.
//!
//! The position is the seek target plus the frames the device took since,
//! both kept under the buffer's lock so a seek never shows a stale time.
//!
//! Gapless: the app queues the next track; when the decoder reaches the end
//! of the current one it opens the queued file and goes on filling the same
//! buffer, marking where the new track starts. When the device plays past
//! that mark the clock restarts and the app hears `Notice::Advanced`.

use std::collections::VecDeque;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::codecs::registry::CodecRegistry;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Time;

use crate::meta::{self, GainMode, ReplayGain};

/// Samples kept for the visualizer (mono).
pub const SCOPE_LEN: usize = 4096;
/// How far the decoder runs ahead of the device.
const AHEAD_SECONDS: f64 = 0.4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Stopped,
    Playing,
    Paused,
}

impl State {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => State::Playing,
            2 => State::Paused,
            _ => State::Stopped,
        }
    }
}

/// What the decoder learned about a track it opened.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackInfo {
    pub path: PathBuf,
    /// Seconds, if the file says.
    pub duration: Option<f64>,
    /// "MP3", "FLAC", "OPUS"…
    pub codec: String,
    pub gain: ReplayGain,
}

/// Things the UI needs to hear about (through the wake callback).
#[derive(Clone, Debug, PartialEq)]
pub enum Notice {
    /// The track played to its end and nothing was queued.
    Finished,
    /// The file couldn't be opened or decoded.
    Error(PathBuf, String),
    /// A track started with `Player::play` opened.
    Opened(TrackInfo),
    /// The queued track took over, without a gap.
    Advanced(TrackInfo),
}

/// Where in the buffered audio the queued track starts.
struct Switch {
    /// Frames since the last reset.
    at: u64,
    info: TrackInfo,
}

struct Buffer {
    samples: VecDeque<f32>,
    /// Seconds at the start of what follows the last seek / load / switch.
    base: f64,
    /// Frames the device took since then.
    played: u64,
    /// Frames the decoder put in since then.
    pushed: u64,
    switches: VecDeque<Switch>,
    /// The track being heard (re-opened when the output's format changes).
    playing: Option<PathBuf>,
    /// The decoder reached the end, with nothing queued.
    eof: bool,
    /// Bumped by every load / seek / stop: the decoder drops audio it
    /// decoded for an older one.
    generation: u64,
}

/// The visualizer's view of the output: a ring of the latest mono samples.
pub struct Scope {
    pub samples: Vec<f32>,
    pub write: usize,
}

struct Shared {
    buf: Mutex<Buffer>,
    scope: Mutex<Scope>,
    state: AtomicU8,
    volume: AtomicU32,
    gain_mode: AtomicU8,
    finished_sent: AtomicBool,
    rate: AtomicU32,
    channels: AtomicU32,
    notices: Mutex<Vec<Notice>>,
    wake: Box<dyn Fn() + Send + Sync>,
    /// For the output thread: the decoder re-opens the track in a new format.
    cmds: Mutex<Option<Sender<Cmd>>>,
}

impl Shared {
    /// The output now runs at `rate` and `channels`. The buffered audio is
    /// in the old format: drop it and have the decoder go on from what was
    /// heard, in the new one.
    fn set_format(&self, rate: u32, channels: u16) {
        let mut b = self.buf.lock().unwrap();
        let old = self.rate.load(Ordering::Relaxed).max(1);
        if old == rate && self.channels.load(Ordering::Relaxed) == channels as u32 {
            return;
        }
        let position = b.base + b.played as f64 / old as f64;
        self.rate.store(rate, Ordering::Relaxed);
        self.channels.store(channels as u32, Ordering::Relaxed);
        if b.eof {
            b.samples.clear(); // the last moment of the playlist: just end
            return;
        }
        let Some(path) = b.playing.clone() else { return };
        // A queued track the decoder already moved on to is queued again.
        let next = b.switches.front().map(|s| s.info.path.clone());
        b.samples.clear();
        b.base = position;
        b.played = 0;
        b.pushed = 0;
        b.switches.clear();
        b.generation += 1;
        let g = b.generation;
        drop(b);
        if let Some(tx) = &*self.cmds.lock().unwrap() {
            let _ = tx.send(Cmd::Reopen(g, path, position, next));
        }
    }

    fn notify(&self, n: Notice) {
        self.notices.lock().unwrap().push(n);
        (self.wake)();
    }

    /// The device callback: fill `out` (interleaved, `channels` per frame).
    fn render(&self, out: &mut [f32], channels: usize) {
        let playing = State::from_u8(self.state.load(Ordering::Acquire)) == State::Playing;
        let volume = f32::from_bits(self.volume.load(Ordering::Relaxed));
        let mut taken = 0;
        let mut ended = false;
        let mut advanced = None;
        if playing {
            let mut b = self.buf.lock().unwrap();
            let n = out.len().min(b.samples.len()) / channels * channels;
            for (o, s) in out.iter_mut().zip(b.samples.drain(..n)) {
                *o = s;
            }
            b.played += (n / channels) as u64;
            if b.switches.front().is_some_and(|s| b.played >= s.at) {
                let s = b.switches.pop_front().expect("checked");
                b.played -= s.at;
                b.pushed -= s.at;
                for later in &mut b.switches {
                    later.at -= s.at;
                }
                b.base = 0.0;
                b.playing = Some(s.info.path.clone());
                advanced = Some(s.info);
            }
            taken = n;
            ended = b.eof && b.samples.is_empty();
        }
        // Visualize before the volume, so it moves the same at any level.
        if taken > 0 {
            let mut sc = self.scope.lock().unwrap();
            for frame in out[..taken].chunks_exact(channels) {
                let m = frame.iter().sum::<f32>() / channels as f32;
                let w = sc.write;
                sc.samples[w] = m;
                sc.write = (w + 1) % SCOPE_LEN;
            }
        }
        for o in &mut out[..taken] {
            *o *= volume;
        }
        out[taken..].fill(0.0);
        if let Some(info) = advanced {
            self.notify(Notice::Advanced(info));
        }
        if ended && !self.finished_sent.swap(true, Ordering::AcqRel) {
            self.notify(Notice::Finished);
        }
    }
}

enum Cmd {
    Load(u64, PathBuf, f64),
    /// The output's format changed: the playing track again, from where it
    /// was heard, then the queued one (if the decoder had moved on to it).
    Reopen(u64, PathBuf, f64, Option<PathBuf>),
    Seek(u64, f64),
    Stop(u64),
    /// Play this after the current track (None: nothing).
    Queue(Option<PathBuf>),
    Quit,
}

pub struct Player {
    shared: Arc<Shared>,
    cmds: Sender<Cmd>,
    decoder: Option<thread::JoinHandle<()>>,
    output: Option<output::Output>,
    output_error: Option<String>,
}

impl Player {
    /// Open the default output device, and follow it when it changes.
    /// Without one the player still runs (silently, the clock stands still
    /// until a device appears) and `output_error` says why.
    pub fn new(wake: impl Fn() + Send + Sync + 'static, with_output: bool) -> Player {
        let shared = Arc::new(Shared {
            buf: Mutex::new(Buffer {
                samples: VecDeque::new(),
                base: 0.0,
                played: 0,
                pushed: 0,
                switches: VecDeque::new(),
                playing: None,
                eof: false,
                generation: 0,
            }),
            scope: Mutex::new(Scope { samples: vec![0.0; SCOPE_LEN], write: 0 }),
            state: AtomicU8::new(0),
            volume: AtomicU32::new(0.8f32.to_bits()),
            gain_mode: AtomicU8::new(GainMode::Track.to_u8()),
            finished_sent: AtomicBool::new(false),
            rate: AtomicU32::new(44_100),
            channels: AtomicU32::new(2),
            notices: Mutex::new(Vec::new()),
            wake: Box::new(wake),
            cmds: Mutex::new(None),
        });
        let (tx, rx) = mpsc::channel();
        *shared.cmds.lock().unwrap() = Some(tx.clone());
        let (output, output_error) = if with_output { output::start(&shared) } else { (None, None) };
        if let Some(e) = &output_error {
            log::warn!("no audio output: {e}");
        }
        let s = shared.clone();
        let decoder = thread::Builder::new()
            .name("decoder".into())
            .spawn(move || decoder_thread(&s, &rx))
            .ok();
        Player { shared, cmds: tx, decoder, output, output_error }
    }

    pub fn output_error(&self) -> Option<&str> {
        self.output_error.as_deref()
    }

    /// Start playing `path` from `start` seconds (drops what was queued).
    pub fn play(&self, path: &Path, start: f64) {
        self.shared.finished_sent.store(false, Ordering::Release);
        self.clear_scope();
        self.shared.state.store(1, Ordering::Release);
        self.reset_buffer(start, |b| {
            b.playing = Some(path.to_path_buf());
            Cmd::Load(b.generation, path.to_path_buf(), start)
        });
    }

    /// The track to play after the current one, without a gap.
    pub fn queue(&self, next: Option<&Path>) {
        let _ = self.cmds.send(Cmd::Queue(next.map(Path::to_path_buf)));
    }

    pub fn pause(&self) {
        if self.state() == State::Playing {
            self.shared.state.store(2, Ordering::Release);
        }
    }

    pub fn resume(&self) {
        if self.state() == State::Paused {
            self.shared.state.store(1, Ordering::Release);
        }
    }

    pub fn stop(&self) {
        self.shared.state.store(0, Ordering::Release);
        self.reset_buffer(0.0, |b| {
            b.playing = None;
            Cmd::Stop(b.generation)
        });
        self.clear_scope();
    }

    pub fn seek(&self, seconds: f64) {
        if self.state() == State::Stopped {
            return;
        }
        let seconds = seconds.max(0.0);
        self.shared.finished_sent.store(false, Ordering::Release);
        self.reset_buffer(seconds, |b| Cmd::Seek(b.generation, seconds));
    }

    pub fn state(&self) -> State {
        State::from_u8(self.shared.state.load(Ordering::Acquire))
    }

    /// Seconds into the track.
    pub fn position(&self) -> f64 {
        let b = self.shared.buf.lock().unwrap();
        b.base + b.played as f64 / self.rate() as f64
    }

    pub fn volume(&self) -> f32 {
        f32::from_bits(self.shared.volume.load(Ordering::Relaxed))
    }

    pub fn set_volume(&self, v: f32) {
        self.shared.volume.store(v.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    /// ReplayGain mode; applies from the next decoded block on.
    pub fn set_gain_mode(&self, mode: GainMode) {
        self.shared.gain_mode.store(mode.to_u8(), Ordering::Relaxed);
    }

    /// The output's sample rate.
    pub fn rate(&self) -> u32 {
        self.shared.rate.load(Ordering::Relaxed).max(1)
    }

    pub fn take_notices(&self) -> Vec<Notice> {
        std::mem::take(&mut *self.shared.notices.lock().unwrap())
    }

    /// The latest `n` played samples (mono, oldest first).
    pub fn scope(&self, n: usize) -> Vec<f32> {
        let n = n.min(SCOPE_LEN);
        let sc = self.shared.scope.lock().unwrap();
        (0..n).map(|i| sc.samples[(sc.write + SCOPE_LEN - n + i) % SCOPE_LEN]).collect()
    }

    /// Empty the buffer for playing from `base` and send the decoder the
    /// command `then` makes for the new generation. Sent under the lock, so
    /// a format change can't slip in between and be overtaken.
    fn reset_buffer(&self, base: f64, then: impl FnOnce(&mut Buffer) -> Cmd) {
        let mut b = self.shared.buf.lock().unwrap();
        b.samples.clear();
        b.base = base;
        b.played = 0;
        b.pushed = 0;
        b.switches.clear();
        b.eof = false;
        b.generation += 1;
        let _ = self.cmds.send(then(&mut b));
    }

    fn clear_scope(&self) {
        let mut sc = self.shared.scope.lock().unwrap();
        sc.samples.fill(0.0);
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.output.take(); // stops the stream before the decoder
        *self.shared.cmds.lock().unwrap() = None;
        let _ = self.cmds.send(Cmd::Quit);
        if let Some(h) = self.decoder.take() {
            let _ = h.join();
        }
    }
}

// ---------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------

/// Symphonia's codecs plus Opus (libopus).
fn codecs() -> &'static CodecRegistry {
    static REGISTRY: OnceLock<CodecRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut reg = CodecRegistry::new();
        symphonia::default::register_enabled_codecs(&mut reg);
        reg.register_audio_decoder::<symphonia_adapter_libopus::OpusDecoder>();
        reg
    })
}

struct Source {
    path: PathBuf,
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track: u32,
    resampler: Resampler,
    gain: ReplayGain,
    /// Drop decoded audio before this timestamp (after a seek).
    skip_until: i64,
    scratch: Vec<f32>,
}

fn open(path: &Path, out_rate: u32) -> Result<(Source, TrackInfo), String> {
    let file = File::open(path).map_err(|e| meta::io_reason(&e))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| {
            log::info!("{}: {e}", path.display());
            "its file format isn't supported".to_string()
        })?;
    let track = format.default_track(TrackType::Audio).ok_or("it has no audio")?;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or("it has no audio")?
        .clone();
    let decoder = codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .map_err(|e| {
            log::info!("{}: {e}", path.display());
            "its audio format isn't supported".to_string()
        })?;
    let rate = params.sample_rate.unwrap_or(44_100);
    let duration = track.num_frames.map(|n| n as f64 / rate as f64);
    let codec = codecs()
        .get_audio_decoder(params.codec)
        .map(|d| d.codec.info.short_name.to_uppercase())
        .unwrap_or_default();
    let track_id = track.id;
    let gain = meta::read_replay_gain(path);
    let info = TrackInfo { path: path.to_path_buf(), duration, codec, gain };
    Ok((
        Source {
            path: path.to_path_buf(),
            track: track_id,
            format,
            decoder,
            resampler: Resampler::new(rate, out_rate),
            gain,
            skip_until: 0,
            scratch: Vec::new(),
        },
        info,
    ))
}

impl Source {
    fn seek(&mut self, seconds: f64) {
        let Some(time) = Time::try_from_secs_f64(seconds) else { return };
        let to = SeekTo::Time { time, track_id: Some(self.track) };
        match self.format.seek(SeekMode::Accurate, to) {
            Ok(s) => self.skip_until = s.required_ts.get(),
            Err(e) => log::debug!("seek: {e}"),
        }
        self.decoder.reset();
        self.resampler.reset();
    }

    /// Decode the next packet into `out` (device channels and rate, gain
    /// applied). `Ok(false)` at the end of the track.
    fn decode(&mut self, out_channels: usize, mode: GainMode, out: &mut Vec<f32>) -> Result<bool, String> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(p)) => p,
                Ok(None) => return Ok(false),
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(false),
                Err(SymError::ResetRequired) => {
                    self.decoder.reset();
                    continue;
                }
                Err(e) => return Err(damaged(&self.path, &e)),
            };
            if packet.track_id != self.track {
                continue;
            }
            if packet.pts.get().saturating_add(packet.dur.get() as i64) < self.skip_until {
                continue;
            }
            let decoded = match self.decoder.decode(&packet) {
                Ok(d) => d,
                Err(SymError::DecodeError(e)) => {
                    log::debug!("{}: skipping a bad packet: {e}", self.path.display());
                    continue;
                }
                Err(e) => return Err(damaged(&self.path, &e)),
            };
            let ch = decoded.spec().channels().count().max(1);
            self.scratch.resize(decoded.samples_interleaved(), 0.0);
            decoded.copy_to_slice_interleaved(&mut self.scratch[..]);
            let factor = self.gain.factor(mode);
            if factor != 1.0 {
                for s in &mut self.scratch {
                    *s *= factor;
                }
            }
            let mapped = remix(&self.scratch, ch, out_channels);
            self.resampler.process(&mapped, out_channels, out);
            return Ok(true);
        }
    }
}

/// A read or decode error in the middle of a track, for the user (the
/// details go to the log).
fn damaged(path: &Path, e: &SymError) -> String {
    log::info!("{}: {e}", path.display());
    match e {
        SymError::IoError(io) => meta::io_reason(io),
        _ => "the file is damaged".to_string(),
    }
}

/// Interleaved `from` channels to `to` channels: mono spreads, extra channels
/// fold into the first ones, mono output averages.
fn remix(input: &[f32], from: usize, to: usize) -> Vec<f32> {
    if from == to {
        return input.to_vec();
    }
    let frames = input.len() / from;
    let mut out = Vec::with_capacity(frames * to);
    for f in input.chunks_exact(from) {
        if to == 1 {
            out.push(f.iter().sum::<f32>() / from as f32);
        } else if from == 1 {
            out.extend(std::iter::repeat_n(f[0], to));
        } else {
            for c in 0..to {
                out.push(f[c % from]);
            }
        }
    }
    out
}

fn decoder_thread(shared: &Shared, cmds: &Receiver<Cmd>) {
    let mut source: Option<Source> = None;
    let mut queued: Option<PathBuf> = None;
    let mut chunk = Vec::new();
    let mut generation = 0;
    loop {
        let out_rate = shared.rate.load(Ordering::Relaxed).max(1);
        let out_ch = shared.channels.load(Ordering::Relaxed).max(1) as usize;
        let full = {
            let b = shared.buf.lock().unwrap();
            b.generation != generation || b.eof || b.samples.len() as f64 >= AHEAD_SECONDS * out_rate as f64 * out_ch as f64
        };
        let wait = if source.is_none() || full { Duration::from_millis(15) } else { Duration::ZERO };
        let cmd = match cmds.recv_timeout(wait) {
            Ok(c) => Some(c),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let had_cmd = cmd.is_some();
        // The format may have changed while waiting.
        let out_rate = shared.rate.load(Ordering::Relaxed).max(1);
        let out_ch = shared.channels.load(Ordering::Relaxed).max(1) as usize;
        match cmd {
            Some(Cmd::Quit) => return,
            Some(Cmd::Stop(g)) => {
                generation = g;
                source = None;
                queued = None;
            }
            Some(Cmd::Load(g, path, start)) => {
                generation = g;
                queued = None;
                source = match open(&path, out_rate) {
                    Ok((mut s, info)) => {
                        if start > 0.0 {
                            s.seek(start);
                        }
                        shared.notify(Notice::Opened(info));
                        Some(s)
                    }
                    Err(e) => {
                        shared.state.store(0, Ordering::Release);
                        shared.notify(Notice::Error(path, e));
                        None
                    }
                };
            }
            Some(Cmd::Reopen(g, path, start, next)) => {
                generation = g;
                if next.is_some() {
                    queued = next;
                }
                source = match open(&path, out_rate) {
                    Ok((mut s, _)) => {
                        s.seek(start);
                        Some(s)
                    }
                    Err(e) => {
                        shared.notify(Notice::Error(path, e));
                        None
                    }
                };
            }
            Some(Cmd::Seek(g, secs)) => {
                generation = g;
                if let Some(s) = &mut source {
                    s.seek(secs);
                }
            }
            Some(Cmd::Queue(next)) => queued = next,
            None => {}
        }
        if full && !had_cmd {
            continue;
        }
        let Some(s) = &mut source else { continue };
        chunk.clear();
        let mode = GainMode::from_u8(shared.gain_mode.load(Ordering::Relaxed));
        match s.decode(out_ch, mode, &mut chunk) {
            Ok(true) => {
                // A command that came meanwhile (seek, load) must win over this chunk.
                let mut b = shared.buf.lock().unwrap();
                if b.generation == generation {
                    b.samples.extend(chunk.iter().copied());
                    b.pushed += (chunk.len() / out_ch) as u64;
                }
            }
            Ok(false) => {
                // The end: on to the queued track, or done.
                let next = queued.take().map(|p| (open(&p, out_rate), p));
                let mut b = shared.buf.lock().unwrap();
                let current = b.generation == generation;
                match next {
                    Some((Ok((s, info)), _)) => {
                        if current {
                            let at = b.pushed;
                            b.switches.push_back(Switch { at, info });
                        }
                        source = Some(s);
                    }
                    Some((Err(e), path)) => {
                        if current {
                            b.eof = true;
                        }
                        drop(b);
                        source = None;
                        shared.notify(Notice::Error(path, e));
                    }
                    None => {
                        if current {
                            b.eof = true;
                        }
                        source = None;
                    }
                }
            }
            Err(e) => {
                let path = s.path.clone();
                let mut b = shared.buf.lock().unwrap();
                if b.generation == generation {
                    b.eof = true;
                }
                drop(b);
                source = None;
                shared.notify(Notice::Error(path, e));
            }
        }
    }
}

/// `--self-test`: the decoders this build needs are there and a generated
/// file decodes to the right length. Returns what was checked.
pub fn self_test(dir: &Path) -> Result<String, String> {
    use symphonia::core::codecs::audio::well_known::CODEC_ID_OPUS;
    if codecs().get_audio_decoder(CODEC_ID_OPUS).is_none() {
        return Err("the Opus decoder is missing".into());
    }
    let path = dir.join("self-test.wav");
    std::fs::write(&path, sine_wav(44_100, 1.0)).map_err(|e| e.to_string())?;
    let (mut src, _) = open(&path, 48_000)?;
    let mut out = Vec::new();
    while src.decode(2, GainMode::Off, &mut out)? {}
    let _ = std::fs::remove_file(&path);
    let frames = out.len() / 2;
    if frames.abs_diff(48_000) > 100 {
        return Err(format!("1 s of audio decoded to {frames} frames at 48 kHz"));
    }
    Ok("decoders (Opus included), 1 s of WAV resampled to 48 kHz".into())
}

/// A mono 16-bit WAV file of a 440 Hz tone.
fn sine_wav(rate: u32, secs: f32) -> Vec<u8> {
    let n = (rate as f32 * secs) as u32;
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + n * 2).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(n * 2).to_le_bytes());
    for i in 0..n {
        let v = ((i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 12_000.0) as i16;
        b.extend_from_slice(&v.to_le_bytes());
    }
    b
}

// ---------------------------------------------------------------------------
// Resampling
// ---------------------------------------------------------------------------

/// Streaming linear-interpolation resampler (the device rarely matches the
/// file: 44.1 kHz music on a 48 kHz output).
struct Resampler {
    /// Input frames per output frame.
    step: f64,
    pos: f64,
    /// The last input frame of the previous chunk.
    prev: Vec<f32>,
}

impl Resampler {
    fn new(from: u32, to: u32) -> Self {
        Resampler { step: from as f64 / to.max(1) as f64, pos: 0.0, prev: Vec::new() }
    }

    fn reset(&mut self) {
        self.pos = 0.0;
        self.prev.clear();
    }

    fn process(&mut self, input: &[f32], ch: usize, out: &mut Vec<f32>) {
        if (self.step - 1.0).abs() < 1e-9 {
            out.extend_from_slice(input);
            return;
        }
        if input.is_empty() {
            return;
        }
        // Frames: prev (index 0, if any) followed by the input.
        let has_prev = self.prev.len() == ch;
        let offset = usize::from(has_prev);
        let frames = input.len() / ch + offset;
        let at = |i: usize, c: usize| -> f32 {
            if has_prev && i == 0 { self.prev[c] } else { input[(i - offset) * ch + c] }
        };
        while self.pos + 1.0 < frames as f64 {
            let i = self.pos as usize;
            let t = (self.pos - i as f64) as f32;
            for c in 0..ch {
                let (a, b) = (at(i, c), at(i + 1, c));
                out.push(a + (b - a) * t);
            }
            self.pos += self.step;
        }
        self.pos -= (frames - 1) as f64;
        self.prev = input[input.len() - ch..].to_vec();
    }
}

// ---------------------------------------------------------------------------
// Output device
// ---------------------------------------------------------------------------

mod output;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampler_keeps_length_ratio_and_continuity() {
        let mut r = Resampler::new(44_100, 48_000);
        let input: Vec<f32> = (0..44_100).map(|i| i as f32 / 1000.0).collect();
        let mut out = Vec::new();
        for chunk in input.chunks(1000) {
            r.process(chunk, 1, &mut out);
        }
        assert!((out.len() as i64 - 48_000).abs() <= 2, "{}", out.len());
        // A ramp stays a ramp across chunk borders.
        for w in out.windows(2) {
            let d = w[1] - w[0];
            assert!((d - 44.1 / 48_000.0).abs() < 2e-5, "{d}");
        }
    }

    /// Decode a whole file at a 48 kHz stereo output; (output frames, duration).
    fn decode_all(path: &Path) -> Result<(usize, Option<f64>), String> {
        let (mut src, info) = open(path, 48_000)?;
        let mut out = Vec::new();
        while src.decode(2, GainMode::Off, &mut out)? {}
        Ok((out.len() / 2, info.duration))
    }

    fn write_wav(path: &Path, rate: u32, secs: f32) {
        std::fs::write(path, sine_wav(rate, secs)).unwrap();
    }

    #[test]
    fn decodes_and_resamples_a_wav() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.wav");
        write_wav(&p, 44_100, 1.5);
        let (frames, duration) = decode_all(&p).unwrap();
        assert!((duration.unwrap() - 1.5).abs() < 0.01);
        assert!((frames as i64 - 72_000).abs() < 50, "{frames}");
    }

    #[test]
    fn seeking_skips_ahead() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.wav");
        write_wav(&p, 48_000, 2.0);
        let (mut src, ..) = open(&p, 48_000).unwrap();
        src.seek(1.5);
        let mut out = Vec::new();
        while src.decode(2, GainMode::Off, &mut out).unwrap() {}
        assert!((out.len() as i64 / 2 - 24_000).abs() < 2_000, "{}", out.len() / 2);
    }

    /// Every file in `$OMP_TEST_MUSIC` decodes to about its length.
    #[test]
    fn decodes_test_music() {
        let Some(dir) = std::env::var_os("OMP_TEST_MUSIC") else { return };
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            match decode_all(&p) {
                Ok((frames, duration)) => {
                    let secs = frames as f64 / 48_000.0;
                    eprintln!("{}: {secs:.2} s decoded, {duration:?} reported", p.display());
                    if let Some(d) = duration {
                        assert!((secs - d).abs() < 0.2, "{}: {secs} vs {d}", p.display());
                    }
                }
                Err(e) => eprintln!("{}: {e}", p.display()),
            }
        }
    }

    /// Two tracks queued back to back play as one unbroken stream: no
    /// silence at the joint, `Advanced` at the switch, the clock restarts.
    #[test]
    fn queued_track_follows_without_a_gap() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.wav"), dir.path().join("b.wav"));
        write_wav(&a, 44_100, 0.5);
        write_wav(&b, 44_100, 0.5);
        let player = Player::new(|| {}, false);
        player.play(&a, 0.0);
        player.queue(Some(&b));
        let mut out = Vec::new();
        let mut block = vec![0.0f32; 512 * 2];
        let mut position_after_switch = None;
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut notices = Vec::new();
        while std::time::Instant::now() < deadline {
            // Play only what is buffered, like a device that never underruns.
            let ready = {
                let buf = player.shared.buf.lock().unwrap();
                buf.samples.len() >= block.len() || buf.eof
            };
            if !ready {
                thread::sleep(Duration::from_millis(1));
                continue;
            }
            player.shared.render(&mut block, 2);
            out.extend_from_slice(&block);
            let new = player.take_notices();
            if new.iter().any(|n| matches!(n, Notice::Advanced(_))) {
                position_after_switch = Some(player.position());
            }
            let done = new.contains(&Notice::Finished);
            notices.extend(new);
            if done {
                break;
            }
        }
        assert!(matches!(notices.first(), Some(Notice::Opened(i)) if i.path == a), "{notices:?}");
        assert!(notices.iter().any(|n| matches!(n, Notice::Advanced(i) if i.path == b)), "{notices:?}");
        assert!(position_after_switch.unwrap() < 0.05, "{position_after_switch:?}");
        // Trailing silence is the last block's padding; before it, no gap.
        let frames: Vec<f32> = out.chunks_exact(2).map(|f| f[0]).collect();
        let end = frames.iter().rposition(|v| v.abs() > 1e-4).unwrap();
        let longest_quiet = frames[..end].split(|v| v.abs() > 1e-4).map(<[f32]>::len).max().unwrap_or(0);
        assert!(longest_quiet < 4, "a gap of {longest_quiet} frames");
        assert!((end as i64 - 44_100).abs() < 600, "{end} frames for 2 × 0.5 s");
    }

    /// A device with another format takes over mid-track: playback goes on
    /// from what was heard, in the new format, and the queued track still
    /// follows.
    #[test]
    fn a_new_output_format_goes_on_from_the_heard_position() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.wav"), dir.path().join("b.wav"));
        write_wav(&a, 44_100, 1.0);
        write_wav(&b, 44_100, 0.5);
        let player = Player::new(|| {}, false);
        player.play(&a, 0.0);
        player.queue(Some(&b));
        let mut channels = 2;
        let mut switched = None;
        let mut notices = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            let mut block = vec![0.0f32; 512 * channels];
            let ready = {
                let buf = player.shared.buf.lock().unwrap();
                buf.samples.len() >= block.len() || buf.eof
            };
            if !ready {
                thread::sleep(Duration::from_millis(1));
                continue;
            }
            player.shared.render(&mut block, channels);
            if switched.is_none() && player.position() > 0.3 {
                let before = player.position();
                player.shared.set_format(48_000, 1);
                channels = 1;
                switched = Some(before);
                assert!((player.position() - before).abs() < 1e-6, "the clock doesn't jump");
            }
            let new = player.take_notices();
            let done = new.contains(&Notice::Finished);
            notices.extend(new);
            if done {
                break;
            }
        }
        assert!(switched.is_some());
        assert_eq!(player.rate(), 48_000);
        assert!(notices.iter().any(|n| matches!(n, Notice::Advanced(i) if i.path == b)), "{notices:?}");
        assert!(!notices.iter().any(|n| matches!(n, Notice::Error(..))), "{notices:?}");
    }

    #[test]
    fn remix_channels() {
        assert_eq!(remix(&[1.0, 2.0], 1, 2), [1.0, 1.0, 2.0, 2.0]);
        assert_eq!(remix(&[1.0, 3.0], 2, 1), [2.0]);
        assert_eq!(remix(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], 6, 2), [1.0, 2.0]);
    }
}
