//! The output device (cpal), followed: the stream lives on its own thread,
//! which owns the cpal host for the player's lifetime (on Windows the COM
//! apartment that watches the default device lives on the thread that first
//! touches cpal). When the system's default output changes (headphones
//! connecting, a switch in the system settings) or the stream breaks, the
//! stream is rebuilt on the new default. The thread checks every 1.5 s, and
//! at once when the system reports a route change. Without a device it
//! keeps looking for one.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use super::Shared;

/// How often the default device is compared with the open one, and how
/// often a missing device is looked for.
const FOLLOW_EVERY: Duration = Duration::from_millis(1500);
/// How long the player waits at start for the device.
const OPEN_TIMEOUT: Duration = Duration::from_secs(3);

/// The running output thread; dropping it stops the stream.
pub(super) struct Output {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Drop for Output {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(h) = self.thread.take() {
            h.thread().unpark();
            let _ = h.join();
        }
    }
}

/// Start following the default device. The error says why there is no
/// device yet (the thread goes on looking for one).
pub(super) fn start(shared: &Arc<Shared>) -> (Option<Output>, Option<String>) {
    let stop = Arc::new(AtomicBool::new(false));
    let (ready, answer) = mpsc::sync_channel(1);
    let (s, st) = (shared.clone(), stop.clone());
    let spawned = thread::Builder::new().name("audio output".into()).spawn(move || {
        let mut backend = cpal_backend::Cpal::new(s.clone());
        let mut f = Follower::new(move |rate, channels| s.set_format(rate, channels));
        f.step(&mut backend, Instant::now());
        let _ = ready.send(f.last_error.clone());
        while !st.load(Ordering::Acquire) {
            thread::park_timeout(Duration::from_millis(100));
            f.step(&mut backend, Instant::now());
        }
    });
    let thread = match spawned {
        Ok(h) => h,
        Err(e) => {
            log::warn!("audio output thread: {e}");
            return (None, Some("the audio device can't be opened".into()));
        }
    };
    let error = match answer.recv_timeout(OPEN_TIMEOUT) {
        Ok(None) => None,
        Ok(Some(e)) if e == NO_DEVICE => Some("no audio device found".into()),
        Ok(Some(e)) => {
            log::warn!("audio output: {e}");
            Some("the audio device can't be opened".into())
        }
        Err(_) => Some("the audio device didn't respond".into()),
    };
    (Some(Output { stop, thread: Some(thread) }), error)
}

/// The open error when the system has no output device at all.
const NO_DEVICE: &str = "no output device";

/// What following the default device needs from the system: cpal in the
/// app, a scripted fake in the tests.
trait Backend {
    type Stream: DeviceStream;
    /// The system's default output device, if there is one.
    fn default_id(&mut self) -> Option<String>;
    /// Open a playing stream on the default device.
    fn open(&mut self) -> Result<Self::Stream, String>;
}

trait DeviceStream {
    fn id(&self) -> Option<&str>;
    fn format(&self) -> (u32, u16);
    /// The stream stopped working (device gone, stream invalidated).
    fn broken(&self) -> bool;
    /// The system reported a route change since the last call.
    fn take_changed(&self) -> bool;
}

/// The device stream and when the default device was last compared with it.
struct Follower<S, F> {
    current: Option<S>,
    set_format: F,
    last_check: Option<Instant>,
    /// Why the last open failed (logged once per outage, not per retry).
    last_error: Option<String>,
}

impl<S: DeviceStream, F: FnMut(u32, u16)> Follower<S, F> {
    fn new(set_format: F) -> Self {
        Follower { current: None, set_format, last_check: None, last_error: None }
    }

    /// Follow the default device; `now` is when to judge whether a check
    /// is due.
    fn step<B: Backend<Stream = S>>(&mut self, backend: &mut B, now: Instant) {
        let due = self.last_check.is_none_or(|t| now.duration_since(t) >= FOLLOW_EVERY);
        let rebuild = match &self.current {
            Some(s) if s.broken() => Some("the stream stopped"),
            Some(s) => {
                let changed = s.take_changed();
                if due || changed {
                    self.last_check = Some(now);
                    // No default at all is usually a moment in a handover:
                    // keep the working stream until there is a new one.
                    match backend.default_id() {
                        Some(id) if Some(id.as_str()) != s.id() => Some("the default device changed"),
                        _ => None,
                    }
                } else {
                    None
                }
            }
            None if due => Some("looking for a device"),
            None => None,
        };
        if let Some(why) = rebuild {
            self.last_check = Some(now);
            self.reopen(backend, why);
        }
    }

    fn reopen<B: Backend<Stream = S>>(&mut self, backend: &mut B, why: &str) {
        let old = self.current.take(); // stops the old stream before opening a new one
        let old_id = old.as_ref().map(|s| s.id().unwrap_or("?").to_string());
        drop(old);
        match backend.open() {
            Ok(s) => {
                let (rate, channels) = s.format();
                log::info!("audio output: {} → {} ({rate} Hz, {channels} channels; {why})", old_id.as_deref().unwrap_or("none"), s.id().unwrap_or("?"));
                (self.set_format)(rate, channels);
                self.current = Some(s);
                self.last_error = None;
            }
            Err(e) => {
                if old_id.is_some() || self.last_error.as_deref() != Some(e.as_str()) {
                    log::info!("audio output: {} → none ({e}; {why})", old_id.as_deref().unwrap_or("none"));
                }
                self.last_error = Some(e);
            }
        }
    }
}

mod cpal_backend {
    use super::*;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{FromSample, SampleFormat, SizedSample, Stream, StreamConfig};

    pub(super) struct Cpal {
        host: cpal::Host,
        shared: Arc<Shared>,
    }

    impl Cpal {
        pub(super) fn new(shared: Arc<Shared>) -> Cpal {
            Cpal { host: cpal::default_host(), shared }
        }
    }

    impl Backend for Cpal {
        type Stream = Open;

        fn default_id(&mut self) -> Option<String> {
            self.host.default_output_device().and_then(|d| d.id().ok()).map(|id| id.to_string())
        }

        fn open(&mut self) -> Result<Open, String> {
            let device = self.host.default_output_device().ok_or(NO_DEVICE)?;
            let supported = device.default_output_config().map_err(|e| e.to_string())?;
            let config = StreamConfig { channels: supported.channels(), sample_rate: supported.sample_rate(), buffer_size: cpal::BufferSize::Default };
            let flags = Flags::default();
            let shared = &self.shared;
            // f32 everywhere it's offered; the device's own format otherwise.
            let stream = build::<f32>(&device, config, shared, &flags)
                .or_else(|e| match supported.sample_format() {
                    SampleFormat::I16 => build::<i16>(&device, config, shared, &flags),
                    SampleFormat::I32 => build::<i32>(&device, config, shared, &flags),
                    SampleFormat::U16 => build::<u16>(&device, config, shared, &flags),
                    _ => Err(e),
                })
                .map_err(|e| e.to_string())?;
            stream.play().map_err(|e| e.to_string())?;
            Ok(Open { _stream: stream, id: device.id().ok().map(|id| id.to_string()), format: (config.sample_rate, config.channels), flags })
        }
    }

    #[derive(Clone, Default)]
    struct Flags {
        broken: Arc<AtomicBool>,
        changed: Arc<AtomicBool>,
    }

    pub(super) struct Open {
        _stream: Stream,
        id: Option<String>,
        format: (u32, u16),
        flags: Flags,
    }

    impl DeviceStream for Open {
        fn id(&self) -> Option<&str> {
            self.id.as_deref()
        }

        fn format(&self) -> (u32, u16) {
            self.format
        }

        fn broken(&self) -> bool {
            self.flags.broken.load(Ordering::Acquire)
        }

        fn take_changed(&self) -> bool {
            self.flags.changed.swap(false, Ordering::AcqRel)
        }
    }

    fn build<T>(device: &cpal::Device, config: StreamConfig, shared: &Arc<Shared>, flags: &Flags) -> Result<Stream, cpal::Error>
    where
        T: SizedSample + FromSample<f32>,
    {
        let channels = config.channels as usize;
        let s = shared.clone();
        let mut scratch: Vec<f32> = Vec::new();
        let flags = flags.clone();
        let owner = thread::current(); // the output thread
        device.build_output_stream::<T, _, _>(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                scratch.resize(data.len(), 0.0);
                s.render(&mut scratch, channels);
                for (d, v) in data.iter_mut().zip(&scratch) {
                    *d = T::from_sample(*v);
                }
            },
            move |e: cpal::Error| {
                use cpal::ErrorKind::*;
                match e.kind() {
                    StreamInvalidated | DeviceNotAvailable => {
                        log::info!("audio output: {e}");
                        flags.broken.store(true, Ordering::Release);
                        owner.unpark();
                    }
                    // cpal may have rerouted the stream itself; check the
                    // default device now instead of in 1.5 s.
                    DeviceChanged => {
                        log::info!("audio output: {e}");
                        flags.changed.store(true, Ordering::Release);
                        owner.unpark();
                    }
                    _ => log::debug!("audio output: {e}"),
                }
            },
            Some(OPEN_TIMEOUT),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// The system as the tests script it.
    #[derive(Default)]
    struct System {
        /// The default device: (id, rate, channels).
        default: Option<(&'static str, u32, u16)>,
        /// Opens that fail before one succeeds.
        failing_opens: usize,
        opens: Vec<&'static str>,
        /// Ids of the streams still alive.
        live: Vec<&'static str>,
        /// The flags of the last stream opened.
        last: Option<(Rc<AtomicBool>, Rc<AtomicBool>)>,
    }

    struct FakeStream {
        id: &'static str,
        format: (u32, u16),
        broken: Rc<AtomicBool>,
        changed: Rc<AtomicBool>,
        system: Rc<RefCell<System>>,
    }

    impl Drop for FakeStream {
        fn drop(&mut self) {
            let mut s = self.system.borrow_mut();
            if let Some(i) = s.live.iter().position(|id| *id == self.id) {
                s.live.remove(i);
            }
        }
    }

    impl DeviceStream for FakeStream {
        fn id(&self) -> Option<&str> {
            Some(self.id)
        }
        fn format(&self) -> (u32, u16) {
            self.format
        }
        fn broken(&self) -> bool {
            self.broken.load(Ordering::Acquire)
        }
        fn take_changed(&self) -> bool {
            self.changed.swap(false, Ordering::AcqRel)
        }
    }

    struct Fake(Rc<RefCell<System>>);

    impl Backend for Fake {
        type Stream = FakeStream;
        fn default_id(&mut self) -> Option<String> {
            self.0.borrow().default.map(|d| d.0.to_string())
        }
        fn open(&mut self) -> Result<FakeStream, String> {
            let mut sys = self.0.borrow_mut();
            let (id, rate, channels) = sys.default.ok_or("no output device")?;
            if sys.failing_opens > 0 {
                sys.failing_opens -= 1;
                return Err("device busy".into());
            }
            sys.opens.push(id);
            sys.live.push(id);
            let (broken, changed) = (Rc::<AtomicBool>::default(), Rc::<AtomicBool>::default());
            sys.last = Some((broken.clone(), changed.clone()));
            Ok(FakeStream { id, format: (rate, channels), broken, changed, system: self.0.clone() })
        }
    }

    type Formats = Rc<RefCell<Vec<(u32, u16)>>>;

    struct Rig {
        sys: Rc<RefCell<System>>,
        backend: Fake,
        formats: Formats,
        f: Follower<FakeStream, Box<dyn FnMut(u32, u16)>>,
        now: Instant,
    }

    impl Rig {
        fn new(default: Option<(&'static str, u32, u16)>) -> Rig {
            let sys = Rc::new(RefCell::new(System { default, ..System::default() }));
            let formats: Formats = Rc::default();
            let fs = formats.clone();
            let f = Follower::new(Box::new(move |r, c| fs.borrow_mut().push((r, c))) as Box<dyn FnMut(u32, u16)>);
            Rig { backend: Fake(sys.clone()), formats, f, sys, now: Instant::now() }
        }
        fn step(&mut self, after: Duration) {
            self.now += after;
            self.f.step(&mut self.backend, self.now);
        }
        fn set_default(&mut self, d: Option<(&'static str, u32, u16)>) {
            self.sys.borrow_mut().default = d;
        }
        fn opens(&self) -> Vec<&'static str> {
            self.sys.borrow().opens.clone()
        }
        fn live(&self) -> Vec<&'static str> {
            self.sys.borrow().live.clone()
        }
        fn last_flags(&self) -> (Rc<AtomicBool>, Rc<AtomicBool>) {
            self.sys.borrow().last.clone().expect("a stream")
        }
    }

    const SPEAKERS: Option<(&str, u32, u16)> = Some(("speakers", 48000, 2));
    const HEADPHONES: Option<(&str, u32, u16)> = Some(("headphones", 44100, 2));
    const SOON: Duration = Duration::from_millis(100);

    #[test]
    fn moves_to_a_new_default_device() {
        let mut r = Rig::new(SPEAKERS);
        r.step(SOON);
        assert_eq!(r.live(), ["speakers"]);
        r.set_default(HEADPHONES);
        r.step(SOON);
        assert_eq!(r.opens(), ["speakers"], "not before the next check");
        r.step(FOLLOW_EVERY);
        assert_eq!(r.opens(), ["speakers", "headphones"]);
        assert_eq!(r.live(), ["headphones"], "the old stream is gone");
        assert_eq!(*r.formats.borrow(), [(48000, 2), (44100, 2)], "the decoder hears about the new format");
    }

    #[test]
    fn a_reported_route_change_is_checked_at_once() {
        let mut r = Rig::new(SPEAKERS);
        r.step(SOON);
        r.set_default(HEADPHONES);
        r.last_flags().1.store(true, Ordering::Release);
        r.step(SOON);
        assert_eq!(r.live(), ["headphones"]);
    }

    #[test]
    fn a_route_change_to_the_same_device_keeps_the_stream() {
        let mut r = Rig::new(SPEAKERS);
        r.step(SOON);
        r.last_flags().1.store(true, Ordering::Release);
        r.step(SOON);
        r.step(FOLLOW_EVERY);
        assert_eq!(r.opens(), ["speakers"]);
    }

    #[test]
    fn a_broken_stream_is_rebuilt_at_once() {
        let mut r = Rig::new(SPEAKERS);
        r.step(SOON);
        r.last_flags().0.store(true, Ordering::Release);
        r.step(SOON);
        assert_eq!(r.opens(), ["speakers", "speakers"]);
        assert_eq!(r.live(), ["speakers"]);
    }

    #[test]
    fn a_moment_without_a_default_device_keeps_the_stream() {
        let mut r = Rig::new(SPEAKERS);
        r.step(SOON);
        r.set_default(None); // a handover between devices
        r.step(FOLLOW_EVERY);
        assert_eq!(r.live(), ["speakers"]);
        r.set_default(HEADPHONES);
        r.step(FOLLOW_EVERY);
        assert_eq!(r.live(), ["headphones"]);
    }

    #[test]
    fn a_vanished_device_is_replaced_when_another_appears() {
        let mut r = Rig::new(HEADPHONES);
        r.step(SOON);
        r.set_default(None);
        r.last_flags().0.store(true, Ordering::Release);
        r.step(SOON);
        assert_eq!(r.live(), Vec::<&str>::new());
        r.step(FOLLOW_EVERY);
        assert!(r.live().is_empty(), "still none");
        r.set_default(SPEAKERS);
        r.step(FOLLOW_EVERY);
        assert_eq!(r.live(), ["speakers"]);
        assert_eq!(r.formats.borrow().last(), Some(&(48000, 2)));
    }

    #[test]
    fn a_failing_open_is_retried_but_not_in_a_busy_loop() {
        let mut r = Rig::new(HEADPHONES);
        r.sys.borrow_mut().failing_opens = 2;
        r.step(SOON);
        r.step(SOON);
        assert!(r.live().is_empty());
        r.step(FOLLOW_EVERY);
        assert!(r.live().is_empty());
        r.step(FOLLOW_EVERY);
        assert_eq!(r.live(), ["headphones"]);
    }
}
