//! Tap sound: a short click played on every tap, interrupting the previous one.
//!
//! Mirrors `TapSound.swift`: six embedded WAVs, audio started lazily on the first tap
//! and torn down after 30 s without taps. ALSA (`libasound.so.2`) is loaded at runtime
//! with `dlopen`, so there is no link-time dependency; on PipeWire/Pulse systems the
//! "default" PCM routes through the pipewire-alsa / alsa-pulse plugin. If the library
//! or the device is unavailable, playing does nothing.
//!
//! `play()` never blocks: it only sends the choice over a channel to an audio thread.
//! That thread owns the PCM and exits (closing the device) when no tap arrives for
//! `IDLE`, or as soon as the `TapSound` is dropped.

use std::ffi::{c_char, c_int, c_long, c_uint, c_ulong, c_void, CStr};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

/// Tear the audio thread down after this long without a tap.
const IDLE: Duration = Duration::from_secs(30);
/// Requested PCM latency (buffer length) in microseconds.
const LATENCY_US: c_uint = 20_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Thock,
    Click,
    Pop,
    Wood,
    Boop,
    Squeak,
}

impl Choice {
    pub const ALL: [Choice; 6] =
        [Choice::Thock, Choice::Click, Choice::Pop, Choice::Wood, Choice::Boop, Choice::Squeak];

    pub fn parse(s: &str) -> Option<Choice> {
        Choice::ALL.into_iter().find(|c| c.name() == s)
    }

    /// The config's tap sound, or the default (thock) if it names none.
    pub fn or_default(s: &str) -> Choice {
        Choice::parse(s).unwrap_or(Choice::Thock)
    }

    pub fn name(self) -> &'static str {
        match self {
            Choice::Thock => "thock",
            Choice::Click => "click",
            Choice::Pop => "pop",
            Choice::Wood => "wood",
            Choice::Boop => "boop",
            Choice::Squeak => "squeak",
        }
    }

    fn wav(self) -> &'static [u8] {
        match self {
            Choice::Thock => include_bytes!("../assets/sounds/tap_thock.wav"),
            Choice::Click => include_bytes!("../assets/sounds/tap_click.wav"),
            Choice::Pop => include_bytes!("../assets/sounds/tap_pop.wav"),
            Choice::Wood => include_bytes!("../assets/sounds/tap_wood.wav"),
            Choice::Boop => include_bytes!("../assets/sounds/tap_boop.wav"),
            Choice::Squeak => include_bytes!("../assets/sounds/tap_squeak.wav"),
        }
    }
}

pub struct TapSound {
    choice: Choice,
    enabled: bool,
    /// Channel to the audio thread while it is alive.
    tx: Option<Sender<Choice>>,
}

impl TapSound {
    /// Cheap: no audio initialisation and no thread until the first `play()`.
    pub fn new(choice: Choice) -> TapSound {
        TapSound { choice, enabled: true, tx: None }
    }

    pub fn set_choice(&mut self, c: Choice) {
        self.choice = c;
    }

    /// Disabling also lets the audio thread (if any) shut down right away.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        if !on {
            self.tx = None;
        }
    }

    /// Non-blocking: hands the tap to the audio thread, spawning it if needed.
    pub fn play(&mut self) {
        if !self.enabled {
            return;
        }
        if let Some(tx) = &self.tx {
            if tx.send(self.choice).is_ok() {
                return;
            }
        }
        // No thread yet, or it exited after being idle: start a fresh one.
        let (tx, rx) = mpsc::channel();
        let _ = tx.send(self.choice);
        let spawned = thread::Builder::new().name("tinyhand-audio".into()).spawn(move || audio_thread(rx));
        self.tx = spawned.ok().map(|_| tx);
    }
}

// ---------------------------------------------------------------------------
// WAV parsing

struct Pcm16 {
    channels: u16,
    rate: u32,
    /// Interleaved little-endian 16-bit samples.
    data: &'static [u8],
}

impl Pcm16 {
    fn frames(&self) -> usize {
        self.data.len() / (2 * self.channels as usize)
    }
}

/// Finds the `fmt ` and `data` chunks of a RIFF/WAVE file. Only 16-bit PCM is accepted.
fn parse_wav(b: &'static [u8]) -> Option<Pcm16> {
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return None;
    }
    let le16 = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    let le32 = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    let mut fmt = None;
    let mut i = 12;
    while i + 8 <= b.len() {
        let id = &b[i..i + 4];
        let len = le32(i + 4) as usize;
        let body = i + 8;
        let end = body.saturating_add(len).min(b.len());
        if id == b"fmt " && end - body >= 16 {
            let tag = le16(body);
            let channels = le16(body + 2);
            let rate = le32(body + 4);
            let bits = le16(body + 14);
            // 1 = PCM, 0xFFFE = WAVE_FORMAT_EXTENSIBLE (assumed PCM sub-format).
            if (tag == 1 || tag == 0xFFFE) && bits == 16 && channels > 0 && rate > 0 {
                fmt = Some((channels, rate));
            } else {
                return None;
            }
        } else if id == b"data" {
            let (channels, rate) = fmt?;
            return Some(Pcm16 { channels, rate, data: &b[body..end] });
        }
        // Chunks are padded to an even length.
        i = body.saturating_add(len).saturating_add(len & 1);
    }
    None
}

// ---------------------------------------------------------------------------
// ALSA, loaded at runtime

#[allow(non_camel_case_types)]
type snd_pcm_t = c_void;

const RTLD_NOW: c_int = 2;
const SND_PCM_STREAM_PLAYBACK: c_int = 0;
const SND_PCM_NONBLOCK: c_int = 1;
const SND_PCM_FORMAT_S16_LE: c_int = 2;
const SND_PCM_ACCESS_RW_INTERLEAVED: c_int = 3;

extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

struct Alsa {
    open: unsafe extern "C" fn(*mut *mut snd_pcm_t, *const c_char, c_int, c_int) -> c_int,
    nonblock: unsafe extern "C" fn(*mut snd_pcm_t, c_int) -> c_int,
    set_params: unsafe extern "C" fn(*mut snd_pcm_t, c_int, c_int, c_uint, c_uint, c_int, c_uint) -> c_int,
    get_params: unsafe extern "C" fn(*mut snd_pcm_t, *mut c_ulong, *mut c_ulong) -> c_int,
    writei: unsafe extern "C" fn(*mut snd_pcm_t, *const c_void, c_ulong) -> c_long,
    recover: unsafe extern "C" fn(*mut snd_pcm_t, c_int, c_int) -> c_int,
    prepare: unsafe extern "C" fn(*mut snd_pcm_t) -> c_int,
    start: unsafe extern "C" fn(*mut snd_pcm_t) -> c_int,
    delay: unsafe extern "C" fn(*mut snd_pcm_t, *mut c_long) -> c_int,
    drop: unsafe extern "C" fn(*mut snd_pcm_t) -> c_int,
    close: unsafe extern "C" fn(*mut snd_pcm_t) -> c_int,
    strerror: unsafe extern "C" fn(c_int) -> *const c_char,
}

impl Alsa {
    /// Loads libasound and resolves the symbols we use. The library, its parsed config
    /// and any plugin it loads (e.g. PipeWire's) stay resident for the life of the
    /// process: freeing them with `snd_config_update_free_global` after each session
    /// makes the plugin unload and reload every time, which leaks ~20 kB per cycle in
    /// libpipewire and risks crashes on unload.
    #[allow(clippy::missing_transmute_annotations)] // target type comes from the field
    fn load() -> Option<Alsa> {
        unsafe {
            let lib = dlopen(c"libasound.so.2".as_ptr(), RTLD_NOW);
            if lib.is_null() {
                return None;
            }
            macro_rules! sym {
                ($name:literal) => {{
                    let p = dlsym(lib, $name.as_ptr());
                    if p.is_null() {
                        return None;
                    }
                    std::mem::transmute::<*mut c_void, _>(p)
                }};
            }
            Some(Alsa {
                open: sym!(c"snd_pcm_open"),
                nonblock: sym!(c"snd_pcm_nonblock"),
                set_params: sym!(c"snd_pcm_set_params"),
                get_params: sym!(c"snd_pcm_get_params"),
                writei: sym!(c"snd_pcm_writei"),
                recover: sym!(c"snd_pcm_recover"),
                prepare: sym!(c"snd_pcm_prepare"),
                start: sym!(c"snd_pcm_start"),
                delay: sym!(c"snd_pcm_delay"),
                drop: sym!(c"snd_pcm_drop"),
                close: sym!(c"snd_pcm_close"),
                strerror: sym!(c"snd_strerror"),
            })
        }
    }

    fn error(&self, err: c_int) -> String {
        let s = unsafe { (self.strerror)(err) };
        if s.is_null() {
            format!("error {err}")
        } else {
            unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned()
        }
    }
}

/// libasound, loaded on first use by an audio thread (`None` if unavailable).
static ALSA: OnceLock<Option<Alsa>> = OnceLock::new();
/// Held by an audio thread for its whole life, so a respawned thread never touches
/// ALSA while the previous one is still tearing down.
static SESSION: Mutex<()> = Mutex::new(());
/// Report an unusable sound device only once per process.
static WARNED: AtomicBool = AtomicBool::new(false);

fn alsa() -> Option<&'static Alsa> {
    ALSA.get_or_init(Alsa::load).as_ref()
}

fn warn_once(msg: &str) {
    if !WARNED.swap(true, Ordering::Relaxed) {
        eprintln!("tinyhand: tap sound unavailable: {msg}");
    }
}

/// Give freed heap memory back to the OS (glibc's `malloc_trim`, looked up at runtime so
/// other libcs simply skip it). Worth it after a big one-off allocation, like decoding a
/// hand image or closing the audio device.
pub(crate) fn trim_heap() {
    // SAFETY: dlsym with RTLD_DEFAULT (null) and a valid C string; if found, the symbol
    // is glibc's `int malloc_trim(size_t)`, which is safe to call at any time.
    unsafe {
        let p = dlsym(ptr::null_mut(), c"malloc_trim".as_ptr());
        if !p.is_null() {
            let trim: unsafe extern "C" fn(usize) -> c_int = std::mem::transmute(p);
            trim(0);
        }
    }
}

/// An open playback PCM.
struct Device {
    alsa: &'static Alsa,
    pcm: *mut snd_pcm_t,
    channels: u16,
    rate: u32,
    /// Frames per `writei` call, so a new tap can cut in between chunks.
    chunk: usize,
}

impl Device {
    fn open(alsa: &'static Alsa, channels: u16, rate: u32) -> Result<Device, String> {
        unsafe {
            let mut pcm = ptr::null_mut();
            // Open non-blocking so a busy device can't hang us, then switch to blocking
            // writes (each chunk is short, so the wait is bounded).
            let err = (alsa.open)(&raw mut pcm, c"default".as_ptr(), SND_PCM_STREAM_PLAYBACK, SND_PCM_NONBLOCK);
            if err < 0 {
                return Err(alsa.error(err));
            }
            // Owned from here on, so every early return closes the PCM.
            let mut dev = Device { alsa, pcm, channels, rate, chunk: 256 };
            let mut err = (alsa.nonblock)(pcm, 0);
            if err >= 0 {
                err = (alsa.set_params)(
                    pcm,
                    SND_PCM_FORMAT_S16_LE,
                    SND_PCM_ACCESS_RW_INTERLEAVED,
                    channels as c_uint,
                    rate,
                    1,
                    LATENCY_US,
                );
            }
            if err < 0 {
                return Err(alsa.error(err));
            }
            let (mut buffer, mut period) = (0, 0);
            if (alsa.get_params)(pcm, &raw mut buffer, &raw mut period) >= 0 && period > 0 {
                dev.chunk = period as usize;
            }
            Ok(dev)
        }
    }

    /// Stops whatever is playing and readies the stream for new data.
    fn reset(&self) -> bool {
        unsafe {
            (self.alsa.drop)(self.pcm);
            (self.alsa.prepare)(self.pcm) >= 0
        }
    }

    /// Writes up to `frames` frames; returns frames written, or `None` on a fatal error.
    fn write(&self, data: &[u8], frames: usize) -> Option<usize> {
        unsafe {
            let n = (self.alsa.writei)(self.pcm, data.as_ptr().cast(), frames as c_ulong);
            if n >= 0 {
                return Some(n as usize);
            }
            // Underrun (-EPIPE) or suspend (-ESTRPIPE): recover and retry.
            ((self.alsa.recover)(self.pcm, n as c_int, 1) >= 0).then_some(0)
        }
    }

    fn start(&self) {
        // Needed only when the sound is shorter than the start threshold; harmless
        // (-EBADFD) when already running.
        unsafe { (self.alsa.start)(self.pcm) };
    }

    /// How long the audio already written will keep playing (ALSA buffer plus whatever
    /// the sound server has queued), with a little margin.
    fn remaining(&self) -> Duration {
        let mut delay: c_long = 0;
        let frames = match unsafe { (self.alsa.delay)(self.pcm, &raw mut delay) } {
            0.. => delay.max(0) as u64,
            _ => 0,
        };
        Duration::from_micros(frames * 1_000_000 / self.rate as u64) + Duration::from_millis(15)
    }

    fn stop(&self) {
        unsafe { (self.alsa.drop)(self.pcm) };
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        unsafe { (self.alsa.close)(self.pcm) };
    }
}

// ---------------------------------------------------------------------------
// Audio thread

enum Next {
    /// Play this sound (it arrived while the previous one was playing).
    Play(Choice),
    /// Done with this sound; wait for the next tap.
    Idle,
    /// The `TapSound` is gone.
    Exit,
}

fn audio_thread(rx: Receiver<Choice>) {
    let _session = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    let Some(alsa) = alsa() else {
        warn_once("could not load libasound.so.2");
        // Drain taps silently until idle, so play() stays a cheap send.
        while rx.recv_timeout(IDLE).is_ok() {}
        return;
    };
    let mut dev: Option<Device> = None;
    run(alsa, &mut dev, rx);
    drop(dev);
    trim_heap();
}

/// Plays taps until idle or until the `TapSound` is dropped.
fn run(alsa: &'static Alsa, dev: &mut Option<Device>, rx: Receiver<Choice>) {
    loop {
        let mut choice = match rx.recv_timeout(IDLE) {
            Ok(c) => c,
            Err(RecvTimeoutError::Timeout) => {
                // Stop accepting taps first so a new play() respawns a thread instead
                // of queueing onto this one.
                drop(rx);
                return;
            }
            Err(RecvTimeoutError::Disconnected) => return,
        };
        loop {
            match play_one(alsa, dev, choice, &rx) {
                Next::Play(c) => choice = c,
                Next::Idle => break,
                Next::Exit => return,
            }
        }
    }
}

fn play_one(alsa: &'static Alsa, dev: &mut Option<Device>, choice: Choice, rx: &Receiver<Choice>) -> Next {
    let Some(wav) = parse_wav(choice.wav()) else {
        return Next::Idle;
    };
    // (Re)open if needed, e.g. when sounds differ in format.
    if dev.as_ref().is_none_or(|d| d.channels != wav.channels || d.rate != wav.rate) {
        *dev = None;
        match Device::open(alsa, wav.channels, wav.rate) {
            Ok(d) => *dev = Some(d),
            Err(e) => {
                warn_once(&e);
                return Next::Idle;
            }
        }
    }
    let d = dev.as_ref().expect("device just opened");
    if !d.reset() {
        *dev = None;
        return Next::Idle;
    }
    let frame_bytes = 2 * wav.channels as usize;
    let total = wav.frames();
    let mut done = 0;
    while done < total {
        match rx.try_recv() {
            Ok(c) => return Next::Play(c),
            Err(TryRecvError::Disconnected) => return Next::Exit,
            Err(TryRecvError::Empty) => {}
        }
        let n = d.chunk.min(total - done);
        match d.write(&wav.data[done * frame_bytes..], n) {
            Some(w) => done += w,
            None => {
                *dev = None;
                return Next::Idle;
            }
        }
    }
    d.start();
    // Let the buffered tail play out (still interruptible), then stop the stream so
    // the sound server isn't pulling silence from us until the next tap.
    match rx.recv_timeout(d.remaining()) {
        Ok(c) => Next::Play(c),
        Err(RecvTimeoutError::Timeout) => {
            d.stop();
            Next::Idle
        }
        Err(RecvTimeoutError::Disconnected) => Next::Exit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tap_sound_parses() {
        for c in Choice::ALL {
            let pcm = parse_wav(c.wav()).unwrap_or_else(|| panic!("{} isn't 16-bit PCM", c.name()));
            assert!(pcm.frames() > 100, "{} is empty", c.name());
            assert!((8_000..=96_000).contains(&pcm.rate), "{}: {} Hz", c.name(), pcm.rate);
            assert_eq!(Choice::parse(c.name()), Some(c));
        }
        assert_eq!(Choice::or_default("nope"), Choice::Thock);
    }

    #[test]
    fn rejects_broken_wavs() {
        assert!(parse_wav(b"RIFF\0\0\0\0WAVEdata\0\0\0\0").is_none(), "data before fmt");
        assert!(parse_wav(b"nope").is_none());
    }
}
