//! Microphone uplink (`ARCH-038`): capture on a per-session `skerry-mic` thread, Opus through
//! AudioToolbox's own encoder, and `0x02` frames towards the session's socket.
//!
//! The thread model is `ARCH-030`'s: every cpal and AudioToolbox call runs on this thread,
//! never on GPUI or Tokio. The IO callback only appends to a 200 ms queue that drops the
//! oldest samples. The thread drains it every 10 ms while capturing, downmixes to mono
//! (channel 0, or the mean of the first two), and feeds an `AudioConverter` that resamples to
//! 24 kHz and encodes 20 ms Opus packets at 32 kbps.
//!
//! It lists devices from the start (the picker works with the microphone off) and opens no
//! stream until it is told to capture. A stream is rebuilt on an error, a stall, a default
//! change, or the chosen device coming back, at most once a second and with backoff.
//!
//! It stops its own stream as soon as the session's frame channel closes, so the device goes
//! off even if the window misses the stream's end. Dropping `Microphone` never blocks: it
//! sends `Shutdown` and detaches the thread.

use crate::audio::{self, OutputDevice};
use crate::capture;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::error::TrySendError;

/// An input device as the menu shows it.
pub type InputDevice = OutputDevice;

/// How often the thread drains the queue while capturing.
const CAPTURE_TICK: Duration = Duration::from_millis(10);
/// How often it wakes while not capturing.
const IDLE_TICK: Duration = Duration::from_millis(500);
const ENUMERATE_EVERY: Duration = Duration::from_secs(1);
/// The level meter's window.
const LEVEL_EVERY: Duration = Duration::from_millis(100);
/// A stream that has called back this long clears its device's strikes.
const HEALTHY_AFTER: Duration = Duration::from_secs(5);
/// A failed build waits at least this long.
const BUILD_RETRY: Duration = Duration::from_secs(2);
/// The rate the encoder is probed at when the thread starts.
const PROBE_RATE: u32 = 48_000;

/// Why the microphone isn't capturing where the person asked, when it isn't.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MicProblem {
    /// The chosen device isn't connected; the default is used, if there is one.
    ChosenMissing,
    /// No input device at all.
    NoDevice,
    /// The stream stopped and retries keep failing.
    Failed,
    /// This Mac's AudioToolbox has no Opus encoder.
    EncoderUnavailable,
}

/// What the microphone thread reports, only when it changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MicStatus {
    /// Connected input devices, sorted by name.
    pub devices: Vec<InputDevice>,
    pub default_uid: Option<String>,
    /// The device capturing now.
    pub active: Option<InputDevice>,
    /// The choice this status answers; `None` is System Default.
    pub chosen: Option<String>,
    pub problem: Option<MicProblem>,
    /// The input level, 0..=5, over the last 100 ms.
    pub level: u8,
    /// A stream is open: the device is live, whether or not packets reach the session. The
    /// badge, the card's dot and its header follow this.
    pub live: bool,
}

impl MicStatus {
    pub fn device(&self, uid: &str) -> Option<&InputDevice> {
        self.devices.iter().find(|device| device.uid == uid)
    }

    pub fn default_device(&self) -> Option<&InputDevice> {
        self.device(self.default_uid.as_deref()?)
    }
}

enum MicControl {
    Choose(Option<String>),
    Capture(bool),
    Shutdown,
}

/// The window's handle on its `skerry-mic` thread.
pub struct Microphone {
    control: Sender<MicControl>,
}

impl Microphone {
    /// Starts the thread, listing devices only. `sink` takes `0x02` frames; `report` gets each
    /// new status. `None` only if the thread can't start.
    pub fn new(
        chosen: Option<String>,
        sink: tokio::sync::mpsc::Sender<Vec<u8>>,
        report: impl Fn(MicStatus) + Send + 'static,
    ) -> Option<Self> {
        let (control, controls) = mpsc::channel();
        std::thread::Builder::new()
            .name("skerry-mic".into())
            .spawn(move || {
                let engine = Engine::new(CpalInput::new(), AudioToolboxOpus::new, chosen, sink);
                run(engine, &controls, report);
            })
            .ok()?;
        Some(Self { control })
    }

    /// Captures from `chosen` from now on (`None` follows the Mac's default). Never blocks.
    pub fn set_input(&self, chosen: Option<String>) {
        let _ = self.control.send(MicControl::Choose(chosen));
    }

    /// Opens or closes the device. Never blocks.
    pub fn set_capture(&self, on: bool) {
        let _ = self.control.send(MicControl::Capture(on));
    }
}

impl Drop for Microphone {
    fn drop(&mut self) {
        // The thread drops its stream on its own; never join here.
        let _ = self.control.send(MicControl::Shutdown);
    }
}

/// What a stream's callbacks share with the thread, one set per stream.
#[derive(Clone, Default)]
struct InputHooks {
    beats: Arc<AtomicU64>,
    errored: Arc<AtomicBool>,
    /// Interleaved samples at the device's rate and channel count.
    queue: Arc<Mutex<VecDeque<f32>>>,
}

/// The IO callback's body: beat, then append if the queue is free (never wait in the
/// callback).
fn deliver(data: &[f32], hooks: &InputHooks, limit: usize) {
    hooks.beats.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut queue) = hooks.queue.try_lock() {
        audio::push_bounded(&mut queue, data, limit);
    }
}

/// 200 ms of interleaved samples, a whole number of frames.
fn queue_limit(rate: u32, channels: u16) -> usize {
    (rate as usize / 5).max(1) * usize::from(channels.max(1))
}

/// Interleaved to mono: channel 0 alone, or the mean of the first two. More channels are
/// ignored (an audio interface's first pair is its main input).
fn downmix(interleaved: &[f32], channels: u16, out: &mut Vec<f32>) {
    out.clear();
    let channels = usize::from(channels.max(1));
    if channels == 1 {
        out.extend_from_slice(interleaved);
        return;
    }
    out.extend(
        interleaved
            .chunks_exact(channels)
            .map(|frame| (frame[0] + frame[1]) * 0.5),
    );
}

/// The meter's level for a peak amplitude: one segment per 10 dB from −50 dBFS.
fn peak_level(peak: f32) -> u8 {
    if peak <= 0.0 {
        return 0;
    }
    let db = 20.0 * peak.log10();
    [-50.0, -40.0, -30.0, -20.0, -10.0]
        .iter()
        .filter(|threshold| db >= **threshold)
        .count() as u8
}

/// The meter's level for some mono samples.
#[cfg(test)]
fn level(mono: &[f32]) -> u8 {
    peak_level(mono.iter().fold(0.0f32, |peak, s| peak.max(s.abs())))
}

/// The cpal side, behind a seam so the loop runs in tests without hardware.
trait MicBackend {
    type Stream;
    /// Connected input devices as `(raw UID, name)`; `None` when listing failed.
    fn devices(&mut self) -> Option<Vec<(String, Option<String>)>>;
    fn default_uid(&mut self) -> Option<String>;
    /// Opens and starts a stream on `uid`; returns it with its rate and channel count.
    fn open(&mut self, uid: &str, hooks: InputHooks) -> Result<(Self::Stream, u32, u16), ()>;
}

/// Mono f32 in, Opus packets out.
trait OpusEncode {
    fn encode(&mut self, mono: &[f32], out: &mut Vec<Vec<u8>>) -> Result<(), ()>;
}

struct CpalInput {
    host: cpal::Host,
    devices: Vec<(String, cpal::Device)>,
}

impl CpalInput {
    fn new() -> Self {
        Self {
            host: cpal::default_host(),
            devices: Vec::new(),
        }
    }
}

impl MicBackend for CpalInput {
    type Stream = cpal::Stream;

    fn devices(&mut self) -> Option<Vec<(String, Option<String>)>> {
        self.devices = self
            .host
            .input_devices()
            .ok()?
            .filter_map(|device| Some((device.id().ok()?.1, device)))
            .collect();
        Some(
            self.devices
                .iter()
                .map(|(uid, device)| {
                    let name = device
                        .description()
                        .ok()
                        .map(|description| description.name().to_string());
                    (uid.clone(), name)
                })
                .collect(),
        )
    }

    fn default_uid(&mut self) -> Option<String> {
        Some(self.host.default_input_device()?.id().ok()?.1)
    }

    fn open(&mut self, uid: &str, hooks: InputHooks) -> Result<(cpal::Stream, u32, u16), ()> {
        let device = self
            .devices
            .iter()
            .find(|(known, _)| known == uid)
            .map(|(_, device)| device.clone())
            .ok_or(())?;
        let default = device.default_input_config().map_err(|_| ())?;
        let (rate, channels) = (default.sample_rate(), default.channels().max(1));
        let config = cpal::StreamConfig {
            channels,
            sample_rate: rate,
            buffer_size: cpal::BufferSize::Default,
        };
        let limit = queue_limit(rate, channels);
        let errored = hooks.errored.clone();
        let stream = device
            .build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| deliver(data, &hooks, limit),
                move |_| errored.store(true, Ordering::Relaxed),
                None,
            )
            .map_err(|_| ())?;
        stream.play().map_err(|_| ())?;
        Ok((stream, rate, channels))
    }
}

struct Active<S> {
    device: InputDevice,
    _stream: S,
    hooks: InputHooks,
    rate: u32,
    channels: u16,
    /// The default when this stream was built on it: rebuilt on any default change.
    built_on_default: Option<String>,
    last_beat: u64,
    last_change: Instant,
    first_beat: Option<Instant>,
}

impl<S> Active<S> {
    /// `true` when the stream errored or stalled.
    fn observe(&mut self, now: Instant) -> bool {
        let beat = self.hooks.beats.load(Ordering::Relaxed);
        if beat != self.last_beat {
            self.last_beat = beat;
            self.last_change = now;
            self.first_beat.get_or_insert(now);
        }
        self.hooks.errored.load(Ordering::Relaxed)
            || now.duration_since(self.last_change) >= audio::STALL
    }

    fn steady(&self) -> bool {
        self.first_beat
            .is_some_and(|first| self.last_change.duration_since(first) >= HEALTHY_AFTER)
    }
}

/// Consecutive failures of one device.
#[derive(Clone, Copy, Debug)]
struct Strikes {
    count: u32,
    retry_at: Instant,
}

/// The microphone thread's state, driven one tick at a time.
struct Engine<B: MicBackend, E: OpusEncode, F: FnMut(u32) -> Result<E, ()>> {
    backend: B,
    make_encoder: F,
    encoder: Option<(u32, E)>,
    /// The probe at start (and every later build) found an encoder.
    encoder_ok: bool,
    sink: tokio::sync::mpsc::Sender<Vec<u8>>,
    /// The session's frame channel closed: the stream ended, so this thread captures no more.
    sink_closed: bool,
    capture: bool,
    chosen: Option<String>,
    devices: Vec<InputDevice>,
    default_uid: Option<String>,
    enumerated: Option<Instant>,
    stream: Option<Active<B::Stream>>,
    strikes: BTreeMap<String, Strikes>,
    last_rebuild: Option<Instant>,
    /// A pick or a start waiting to apply: it skips the rate limit and the backoff.
    picked: bool,
    interleaved: Vec<f32>,
    mono: Vec<f32>,
    packets: Vec<Vec<u8>>,
    peak: f32,
    level: u8,
    level_at: Option<Instant>,
}

impl<B: MicBackend, E: OpusEncode, F: FnMut(u32) -> Result<E, ()>> Engine<B, E, F> {
    fn new(
        backend: B,
        mut make_encoder: F,
        chosen: Option<String>,
        sink: tokio::sync::mpsc::Sender<Vec<u8>>,
    ) -> Self {
        let encoder_ok = make_encoder(PROBE_RATE).is_ok();
        Self {
            backend,
            make_encoder,
            encoder: None,
            encoder_ok,
            sink,
            sink_closed: false,
            capture: false,
            chosen,
            devices: Vec::new(),
            default_uid: None,
            enumerated: None,
            stream: None,
            strikes: BTreeMap::new(),
            last_rebuild: None,
            picked: true,
            interleaved: Vec::new(),
            mono: Vec::new(),
            packets: Vec::new(),
            peak: 0.0,
            level: 0,
            level_at: None,
        }
    }

    fn choose(&mut self, chosen: Option<String>) {
        if let Some(uid) = &chosen {
            self.strikes.remove(uid);
        }
        self.chosen = chosen;
        self.picked = true;
    }

    fn set_capture(&mut self, on: bool) {
        self.capture = on;
        self.picked = true;
    }

    fn capturing(&self) -> bool {
        self.capture && self.stream.is_some()
    }

    fn strike(&mut self, uid: &str, build_failed: bool, now: Instant) {
        let count = self.strikes.get(uid).map_or(0, |strikes| strikes.count) + 1;
        let wait = if build_failed {
            audio::backoff(count).max(BUILD_RETRY)
        } else {
            audio::backoff(count)
        };
        self.strikes.insert(
            uid.to_string(),
            Strikes {
                count,
                retry_at: now + wait,
            },
        );
    }

    fn enumerate(&mut self) {
        let Some(devices) = self.backend.devices() else {
            return;
        };
        self.devices = audio::label_devices_as(devices, "Microphone");
        self.default_uid = self
            .backend
            .default_uid()
            .filter(|uid| self.devices.iter().any(|device| &device.uid == uid));
    }

    fn present(&self, uid: &str) -> bool {
        self.devices.iter().any(|device| device.uid == uid)
    }

    /// The chosen device when connected, else the default, else the first device.
    fn target(&self) -> Option<String> {
        match &self.chosen {
            Some(uid) if self.present(uid) => Some(uid.clone()),
            _ => self
                .default_uid
                .clone()
                .or_else(|| self.devices.first().map(|device| device.uid.clone())),
        }
    }

    fn needs_rebuild(&self, target: Option<&str>) -> bool {
        match (&self.stream, target) {
            (None, Some(_)) | (Some(_), None) => true,
            (Some(active), Some(target)) => {
                active.device.uid != target
                    || active
                        .built_on_default
                        .as_ref()
                        .is_some_and(|built| Some(built) != self.default_uid.as_ref())
            }
            (None, None) => false,
        }
    }

    /// Drops the stream, its queue and the meter.
    fn close(&mut self) {
        self.stream = None;
        self.peak = 0.0;
        self.level = 0;
        self.level_at = None;
    }

    fn tick(&mut self, now: Instant) -> MicStatus {
        let due = self
            .enumerated
            .is_none_or(|last| now.duration_since(last) >= ENUMERATE_EVERY);
        if due || self.picked {
            self.enumerate();
            self.enumerated = Some(now);
        }
        let target = self.target();
        if !self.capture || !self.encoder_ok || self.sink_closed {
            self.close();
            self.encoder = None;
            self.picked = false;
            return self.status(target.as_deref());
        }
        let health = self.stream.as_mut().map(|active| {
            let failed = active.observe(now);
            (active.device.uid.clone(), failed, active.steady())
        });
        match health {
            Some((uid, true, _)) => {
                self.close();
                self.strike(&uid, false, now);
            }
            Some((uid, false, true)) => {
                self.strikes.remove(&uid);
            }
            _ => {}
        }
        if self.needs_rebuild(target.as_deref()) {
            let limited = self
                .last_rebuild
                .is_some_and(|last| now.duration_since(last) < audio::REBUILD_LIMIT);
            let waiting = target.as_ref().is_some_and(|uid| {
                self.strikes
                    .get(uid)
                    .is_some_and(|strikes| strikes.retry_at > now)
            });
            if self.picked || (!limited && !waiting) {
                self.rebuild(target.as_deref(), now);
            }
        }
        self.picked = false;
        self.drain(now);
        self.status(target.as_deref())
    }

    fn rebuild(&mut self, target: Option<&str>, now: Instant) {
        self.close();
        let Some(uid) = target else {
            return;
        };
        self.last_rebuild = Some(now);
        let Some(device) = self.devices.iter().find(|d| d.uid == uid).cloned() else {
            return;
        };
        let hooks = InputHooks::default();
        match self.backend.open(uid, hooks.clone()) {
            Ok((stream, rate, channels)) => {
                self.stream = Some(Active {
                    built_on_default: (self.default_uid.as_deref() == Some(uid))
                        .then(|| uid.to_string()),
                    device,
                    _stream: stream,
                    last_beat: hooks.beats.load(Ordering::Relaxed),
                    hooks,
                    rate,
                    channels,
                    last_change: now,
                    first_beat: None,
                });
            }
            Err(()) => self.strike(uid, true, now),
        }
    }

    /// Encodes what the callback queued and hands the packets to the session.
    fn drain(&mut self, now: Instant) {
        let Some(active) = &self.stream else {
            return;
        };
        let (rate, channels) = (active.rate, active.channels);
        let (uid, queue) = (active.device.uid.clone(), active.hooks.queue.clone());
        self.interleaved.clear();
        if let Ok(mut queue) = queue.lock() {
            self.interleaved.extend(queue.drain(..));
        }
        downmix(&self.interleaved, channels, &mut self.mono);
        self.peak = self
            .mono
            .iter()
            .fold(self.peak, |peak, sample| peak.max(sample.abs()));
        match self.level_at {
            Some(at) if now.duration_since(at) < LEVEL_EVERY => {}
            _ => {
                self.level = peak_level(self.peak);
                self.peak = 0.0;
                self.level_at = Some(now);
            }
        }
        if self
            .encoder
            .as_ref()
            .is_none_or(|(built, _)| *built != rate)
        {
            match (self.make_encoder)(rate) {
                Ok(encoder) => self.encoder = Some((rate, encoder)),
                Err(()) => {
                    self.encoder_ok = false;
                    self.encoder = None;
                    self.close();
                    return;
                }
            }
        }
        if self.mono.is_empty() {
            return;
        }
        self.packets.clear();
        let encoded = match &mut self.encoder {
            Some((_, encoder)) => encoder.encode(&self.mono, &mut self.packets),
            None => Ok(()),
        };
        if encoded.is_err() {
            // A converter in a bad state: start over with a fresh one on the next build.
            self.encoder = None;
            self.close();
            self.strike(&uid, false, now);
            return;
        }
        let packets = std::mem::take(&mut self.packets);
        for packet in &packets {
            match self.sink.try_send(capture::mic_frame(packet)) {
                Ok(()) | Err(TrySendError::Full(_)) => {}
                Err(TrySendError::Closed(_)) => {
                    // The stream that owned this thread ended: stop now, whatever the window
                    // does (`ARCH-038`).
                    self.sink_closed = true;
                    self.capture = false;
                    self.encoder = None;
                    self.close();
                    break;
                }
            }
        }
        self.packets = packets;
        self.packets.clear();
    }

    fn status(&self, target: Option<&str>) -> MicStatus {
        let struck = target.is_some_and(|uid| {
            self.strikes
                .get(uid)
                .is_some_and(|strikes| strikes.count >= 2)
        });
        let problem = if !self.encoder_ok {
            Some(MicProblem::EncoderUnavailable)
        } else if self.chosen.as_deref().is_some_and(|uid| !self.present(uid)) {
            Some(MicProblem::ChosenMissing)
        } else if target.is_none() {
            Some(MicProblem::NoDevice)
        } else if self.capture && self.stream.is_none() && struck {
            Some(MicProblem::Failed)
        } else {
            None
        };
        MicStatus {
            devices: self.devices.clone(),
            default_uid: self.default_uid.clone(),
            active: self.stream.as_ref().map(|active| active.device.clone()),
            chosen: self.chosen.clone(),
            problem,
            level: if self.stream.is_some() { self.level } else { 0 },
            live: self.stream.is_some(),
        }
    }
}

/// What the queued controls amount to: only the last pick and the last capture count.
#[derive(Debug, Default, PartialEq, Eq)]
struct Collapsed {
    shutdown: bool,
    choose: Option<Option<String>>,
    capture: Option<bool>,
}

fn collapse(controls: impl IntoIterator<Item = MicControl>) -> Collapsed {
    let mut collapsed = Collapsed::default();
    for control in controls {
        match control {
            MicControl::Choose(chosen) => collapsed.choose = Some(chosen),
            MicControl::Capture(on) => collapsed.capture = Some(on),
            MicControl::Shutdown => collapsed.shutdown = true,
        }
    }
    collapsed
}

/// The microphone thread: ticks at once, then every 10 ms while capturing (500 ms
/// otherwise), reports changed statuses, and ends on `Shutdown` or once `Microphone` is gone,
/// dropping its stream here.
fn run<B: MicBackend, E: OpusEncode, F: FnMut(u32) -> Result<E, ()>>(
    mut engine: Engine<B, E, F>,
    controls: &Receiver<MicControl>,
    report: impl Fn(MicStatus),
) {
    let mut last = None;
    let mut wait = Duration::ZERO;
    loop {
        let first = match controls.recv_timeout(wait) {
            Ok(control) => Some(control),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let collapsed = collapse(first.into_iter().chain(controls.try_iter()));
        if collapsed.shutdown {
            return;
        }
        if let Some(chosen) = collapsed.choose {
            engine.choose(chosen);
        }
        if let Some(on) = collapsed.capture {
            engine.set_capture(on);
        }
        let status = engine.tick(Instant::now());
        wait = if engine.capturing() {
            CAPTURE_TICK
        } else {
            IDLE_TICK
        };
        if last.as_ref() != Some(&status) {
            report(status.clone());
            last = Some(status);
        }
    }
}

/// The bitrate to ask for: `want` when an applicable range holds it, else the nearest end.
fn choose_bitrate(ranges: &[(f64, f64)], want: u32) -> u32 {
    let want_f = f64::from(want);
    if ranges.is_empty() || ranges.iter().any(|(lo, hi)| *lo <= want_f && want_f <= *hi) {
        return want;
    }
    ranges
        .iter()
        .flat_map(|(lo, hi)| [*lo, *hi])
        .min_by(|a, b| (a - want_f).abs().total_cmp(&(b - want_f).abs()))
        .map_or(want, |rate| rate.round() as u32)
}

pub use opus::AudioToolboxOpus;

/// AudioToolbox's Opus encoder, streamed through `AudioConverterFillComplexBuffer`.
mod opus {
    use super::OpusEncode;
    use objc2_audio_toolbox::{
        kAudioConverterApplicableEncodeBitRates, kAudioConverterEncodeBitRate,
        kAudioConverterPropertyMaximumOutputPacketSize, AudioConverterDispose,
        AudioConverterFillComplexBuffer, AudioConverterGetProperty, AudioConverterNew,
        AudioConverterRef, AudioConverterSetProperty,
    };
    use objc2_core_audio_types::{
        kAudioFormatFlagsNativeFloatPacked, kAudioFormatLinearPCM, kAudioFormatOpus, AudioBuffer,
        AudioBufferList, AudioStreamBasicDescription, AudioStreamPacketDescription,
        AudioValueRange,
    };
    use std::ffi::c_void;
    use std::ptr::{null_mut, NonNull};

    /// Selkies plays the microphone at 24 kHz mono (`audio/microphone.rs`).
    pub const OPUS_RATE: u32 = 24_000;
    /// 20 ms at 24 kHz, as the web client sends.
    pub const FRAMES_PER_PACKET: u32 = 480;
    /// The web client's bitrate.
    const BITRATE: u32 = 32_000;
    /// Packets asked for per `FillComplexBuffer` call.
    const BATCH: usize = 8;
    /// The input proc's answer when its queue is short: a private non-zero status with 0
    /// packets. `noErr` with 0 packets would mean end of stream, which needs a reset.
    const NEED_MORE: i32 = i32::from_be_bytes(*b"skmn");

    /// The samples waiting for the converter, and the block last handed to it (which must
    /// stay valid until the next input proc call).
    #[derive(Default)]
    struct Feed {
        pending: Vec<f32>,
        handed: Vec<f32>,
    }

    pub struct AudioToolboxOpus {
        converter: AudioConverterRef,
        feed: Box<Feed>,
        out: Vec<u8>,
        descriptions: Vec<AudioStreamPacketDescription>,
    }

    /// The converter's input proc: hands over what is queued, up to what it asks for.
    unsafe extern "C-unwind" fn feed_input(
        _converter: AudioConverterRef,
        packets: NonNull<u32>,
        data: NonNull<AudioBufferList>,
        descriptions: *mut *mut AudioStreamPacketDescription,
        user: *mut c_void,
    ) -> i32 {
        // SAFETY: `user` is the encoder's boxed `Feed`, alive and not otherwise borrowed for
        // the whole `FillComplexBuffer` call; the converter's pointers are valid for this call.
        unsafe {
            let feed = &mut *user.cast::<Feed>();
            let wanted = *packets.as_ptr() as usize;
            let count = wanted.min(feed.pending.len());
            if !descriptions.is_null() {
                *descriptions = null_mut();
            }
            let list = data.as_ptr();
            (*list).mNumberBuffers = 1;
            if count == 0 {
                *packets.as_ptr() = 0;
                (*list).mBuffers[0] = AudioBuffer {
                    mNumberChannels: 1,
                    mDataByteSize: 0,
                    mData: null_mut(),
                };
                return NEED_MORE;
            }
            feed.handed.clear();
            feed.handed.extend(feed.pending.drain(..count));
            (*list).mBuffers[0] = AudioBuffer {
                mNumberChannels: 1,
                mDataByteSize: (count * size_of::<f32>()) as u32,
                mData: feed.handed.as_mut_ptr().cast(),
            };
            *packets.as_ptr() = count as u32;
        }
        0
    }

    impl AudioToolboxOpus {
        /// An encoder for mono f32 at `input_rate`, resampled to 24 kHz. `Err` when this Mac
        /// has no Opus encoder.
        pub fn new(input_rate: u32) -> Result<Self, ()> {
            let input = AudioStreamBasicDescription {
                mSampleRate: f64::from(input_rate),
                mFormatID: kAudioFormatLinearPCM,
                mFormatFlags: kAudioFormatFlagsNativeFloatPacked,
                mBytesPerPacket: 4,
                mFramesPerPacket: 1,
                mBytesPerFrame: 4,
                mChannelsPerFrame: 1,
                mBitsPerChannel: 32,
                mReserved: 0,
            };
            let output = AudioStreamBasicDescription {
                mSampleRate: f64::from(OPUS_RATE),
                mFormatID: kAudioFormatOpus,
                mFormatFlags: 0,
                mBytesPerPacket: 0,
                mFramesPerPacket: FRAMES_PER_PACKET,
                mBytesPerFrame: 0,
                mChannelsPerFrame: 1,
                mBitsPerChannel: 0,
                mReserved: 0,
            };
            let mut converter: AudioConverterRef = null_mut();
            // SAFETY: both descriptions and the out pointer are valid locals.
            let status = unsafe {
                AudioConverterNew(
                    NonNull::from(&input),
                    NonNull::from(&output),
                    NonNull::from(&mut converter),
                )
            };
            if status != 0 || converter.is_null() {
                return Err(());
            }
            // Disposed by `Drop` from here on.
            let mut encoder = Self {
                converter,
                feed: Box::default(),
                out: Vec::new(),
                descriptions: Vec::new(),
            };
            let bitrate = super::choose_bitrate(&encoder.applicable_bitrates(), BITRATE);
            // SAFETY: a live converter and a u32 property value.
            let status = unsafe {
                AudioConverterSetProperty(
                    encoder.converter,
                    kAudioConverterEncodeBitRate,
                    size_of::<u32>() as u32,
                    NonNull::from(&bitrate).cast(),
                )
            };
            if status != 0 {
                return Err(());
            }
            let mut max_packet: u32 = 0;
            let mut size = size_of::<u32>() as u32;
            // SAFETY: a live converter, and a u32 to write into.
            let status = unsafe {
                AudioConverterGetProperty(
                    encoder.converter,
                    kAudioConverterPropertyMaximumOutputPacketSize,
                    NonNull::from(&mut size),
                    NonNull::from(&mut max_packet).cast(),
                )
            };
            if status != 0 || max_packet == 0 {
                return Err(());
            }
            encoder.out = vec![0; max_packet as usize * BATCH];
            encoder.descriptions = vec![
                AudioStreamPacketDescription {
                    mStartOffset: 0,
                    mVariableFramesInPacket: 0,
                    mDataByteSize: 0,
                };
                BATCH
            ];
            Ok(encoder)
        }

        /// `kAudioConverterApplicableEncodeBitRates` as `(min, max)` ranges; empty if the
        /// converter doesn't say.
        fn applicable_bitrates(&self) -> Vec<(f64, f64)> {
            let mut ranges = [AudioValueRange {
                mMinimum: 0.0,
                mMaximum: 0.0,
            }; 64];
            let mut size = size_of_val(&ranges) as u32;
            // SAFETY: a live converter and a buffer of `size` bytes.
            let status = unsafe {
                AudioConverterGetProperty(
                    self.converter,
                    kAudioConverterApplicableEncodeBitRates,
                    NonNull::from(&mut size),
                    NonNull::from(&mut ranges).cast(),
                )
            };
            if status != 0 {
                return Vec::new();
            }
            let count = (size as usize / size_of::<AudioValueRange>()).min(ranges.len());
            ranges[..count]
                .iter()
                .map(|range| (range.mMinimum, range.mMaximum))
                .collect()
        }
    }

    impl OpusEncode for AudioToolboxOpus {
        fn encode(&mut self, mono: &[f32], out: &mut Vec<Vec<u8>>) -> Result<(), ()> {
            self.feed.pending.extend_from_slice(mono);
            loop {
                let mut packets = BATCH as u32;
                let mut list = AudioBufferList {
                    mNumberBuffers: 1,
                    mBuffers: [AudioBuffer {
                        mNumberChannels: 1,
                        mDataByteSize: self.out.len() as u32,
                        mData: self.out.as_mut_ptr().cast(),
                    }],
                };
                let feed: *mut Feed = &mut *self.feed;
                // SAFETY: a live converter; the output buffer holds `BATCH` packets of the
                // maximum size and `descriptions` has `BATCH` entries; `feed` outlives the call.
                let status = unsafe {
                    AudioConverterFillComplexBuffer(
                        self.converter,
                        Some(feed_input),
                        feed.cast(),
                        NonNull::from(&mut packets),
                        NonNull::from(&mut list),
                        self.descriptions.as_mut_ptr(),
                    )
                };
                let produced = (packets as usize).min(BATCH);
                for description in &self.descriptions[..produced] {
                    let start = usize::try_from(description.mStartOffset).unwrap_or(usize::MAX);
                    let bytes = start
                        .checked_add(description.mDataByteSize as usize)
                        .and_then(|end| self.out.get(start..end));
                    if let Some(bytes) = bytes.filter(|bytes| !bytes.is_empty()) {
                        out.push(bytes.to_vec());
                    }
                }
                match status {
                    0 if produced == BATCH => continue,
                    0 | NEED_MORE => return Ok(()),
                    _ => return Err(()),
                }
            }
        }
    }

    impl Drop for AudioToolboxOpus {
        fn drop(&mut self) {
            // SAFETY: created by `AudioConverterNew` and disposed once, here.
            unsafe {
                AudioConverterDispose(self.converter);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    struct Opened {
        hooks: InputHooks,
        alive: Rc<Cell<bool>>,
        channels: u16,
    }

    #[derive(Default)]
    struct World {
        devices: Vec<(String, Option<String>)>,
        default: Option<String>,
        opens: Vec<String>,
        streams: Vec<Opened>,
        /// Every open reports this rate and channel count.
        format: Option<(u32, u16)>,
    }

    #[derive(Clone, Default)]
    struct Fake(Rc<RefCell<World>>);

    struct FakeStream(Rc<Cell<bool>>);

    impl Drop for FakeStream {
        fn drop(&mut self) {
            self.0.set(false);
        }
    }

    impl Fake {
        fn with(devices: &[(&str, &str)], default: Option<&str>) -> Self {
            let fake = Self::default();
            fake.set(devices, default);
            fake
        }

        fn set(&self, devices: &[(&str, &str)], default: Option<&str>) {
            let mut world = self.0.borrow_mut();
            world.devices = devices
                .iter()
                .map(|(uid, name)| (uid.to_string(), Some(name.to_string())))
                .collect();
            world.default = default.map(str::to_string);
        }

        fn opens(&self) -> Vec<String> {
            self.0.borrow().opens.clone()
        }

        fn live(&self) -> usize {
            self.0
                .borrow()
                .streams
                .iter()
                .filter(|opened| opened.alive.get())
                .count()
        }

        /// Runs every live stream's callback once with `frames` frames of `value`.
        fn callback(&self, frames: usize, value: f32) {
            let mut world = self.0.borrow_mut();
            world.streams.retain(|opened| opened.alive.get());
            for opened in &world.streams {
                let data = vec![value; frames * usize::from(opened.channels)];
                deliver(&data, &opened.hooks, 1 << 20);
            }
        }
    }

    impl MicBackend for Fake {
        type Stream = FakeStream;
        fn devices(&mut self) -> Option<Vec<(String, Option<String>)>> {
            Some(self.0.borrow().devices.clone())
        }
        fn default_uid(&mut self) -> Option<String> {
            self.0.borrow().default.clone()
        }
        fn open(&mut self, uid: &str, hooks: InputHooks) -> Result<(FakeStream, u32, u16), ()> {
            let mut world = self.0.borrow_mut();
            world.opens.push(uid.into());
            let (rate, channels) = world.format.unwrap_or((48_000, 1));
            let alive = Rc::new(Cell::new(true));
            world.streams.push(Opened {
                hooks,
                alive: alive.clone(),
                channels,
            });
            Ok((FakeStream(alive), rate, channels))
        }
    }

    /// One "packet" per 480 mono samples, its first byte the packet's index.
    #[derive(Default)]
    struct FakeEncoder {
        carry: usize,
        made: u8,
    }

    impl OpusEncode for FakeEncoder {
        fn encode(&mut self, mono: &[f32], out: &mut Vec<Vec<u8>>) -> Result<(), ()> {
            self.carry += mono.len();
            while self.carry >= 480 {
                self.carry -= 480;
                out.push(vec![self.made, 0xAA]);
                self.made = self.made.wrapping_add(1);
            }
            Ok(())
        }
    }

    const BUILTIN: (&str, &str) = ("BuiltIn", "MacBook Pro Microphone");
    const USB: (&str, &str) = ("USB", "Shure MV7");

    type TestEngine = Engine<Fake, FakeEncoder, fn(u32) -> Result<FakeEncoder, ()>>;

    struct Rig {
        fake: Fake,
        engine: TestEngine,
        frames: tokio::sync::mpsc::Receiver<Vec<u8>>,
        start: Instant,
    }

    fn working(_: u32) -> Result<FakeEncoder, ()> {
        Ok(FakeEncoder::default())
    }

    fn broken(_: u32) -> Result<FakeEncoder, ()> {
        Err(())
    }

    impl Rig {
        fn new(fake: Fake, queue: usize) -> Self {
            Self::with_encoder(fake, queue, working)
        }

        fn with_encoder(
            fake: Fake,
            queue: usize,
            make: fn(u32) -> Result<FakeEncoder, ()>,
        ) -> Self {
            let (sink, frames) = tokio::sync::mpsc::channel(queue);
            Self {
                engine: Engine::new(fake.clone(), make, None, sink),
                fake,
                frames,
                start: Instant::now(),
            }
        }

        fn tick(&mut self, ms: u64) -> MicStatus {
            self.engine.tick(self.start + Duration::from_millis(ms))
        }

        fn active(&self) -> Option<String> {
            self.engine.stream.as_ref().map(|a| a.device.uid.clone())
        }

        fn received(&mut self) -> Vec<Vec<u8>> {
            std::iter::from_fn(|| self.frames.try_recv().ok()).collect()
        }
    }

    #[test]
    fn downmix_mono_passthrough() {
        let mut out = Vec::new();
        downmix(&[0.1, -0.2, 0.3], 1, &mut out);
        assert_eq!(out, [0.1, -0.2, 0.3]);
    }

    #[test]
    fn downmix_stereo_averages() {
        let mut out = vec![9.0];
        downmix(&[1.0, 0.0, 0.5, 0.5, -1.0, 1.0], 2, &mut out);
        assert_eq!(out, [0.5, 0.5, 0.0]);
    }

    #[test]
    fn downmix_many_channels_uses_first_two() {
        let mut out = Vec::new();
        downmix(&[1.0, 0.0, 9.0, 9.0, 0.2, 0.4, -9.0, 9.0], 4, &mut out);
        assert_eq!(out.len(), 2);
        assert!((out[0] - 0.5).abs() < 1e-6 && (out[1] - 0.3).abs() < 1e-6);
    }

    #[test]
    fn level_buckets_by_peak_dbfs() {
        assert_eq!(level(&[]), 0);
        assert_eq!(level(&[0.0, 0.0]), 0);
        // −60 dBFS, −45, −35, −25, −15, −5.
        assert_eq!(level(&[0.001]), 0);
        assert_eq!(level(&[0.0056]), 1);
        assert_eq!(level(&[-0.0178]), 2);
        assert_eq!(level(&[0.0562]), 3);
        assert_eq!(level(&[0.178]), 4);
        assert_eq!(level(&[0.0, -0.56]), 5);
        assert_eq!(level(&[1.0]), 5);
    }

    #[test]
    fn queue_limit_holds_whole_frames() {
        assert_eq!(queue_limit(48_000, 1), 9_600);
        assert_eq!(queue_limit(44_100, 2), 17_640);
        assert!(queue_limit(44_100, 3).is_multiple_of(3));
    }

    #[test]
    fn engine_lists_devices_without_opening_stream() {
        let mut rig = Rig::new(Fake::with(&[BUILTIN, USB], Some("BuiltIn")), 10);
        let status = rig.tick(0);
        assert_eq!(status.devices.len(), 2);
        assert_eq!(
            status.default_device().map(|d| d.name.as_str()),
            Some("MacBook Pro Microphone")
        );
        assert!(!status.live);
        assert_eq!(status.problem, None);
        assert!(rig.fake.opens().is_empty());
        for ms in (500..5000).step_by(500) {
            rig.tick(ms);
        }
        assert!(rig.fake.opens().is_empty());
    }

    #[test]
    fn engine_opens_on_capture_and_closes_on_stop() {
        let mut rig = Rig::new(Fake::with(&[BUILTIN], Some("BuiltIn")), 10);
        rig.tick(0);
        rig.engine.set_capture(true);
        let status = rig.tick(10);
        // Live as soon as the device is open, before any packet.
        assert!(status.live);
        assert_eq!(rig.active().as_deref(), Some("BuiltIn"));
        rig.fake.callback(960, 0.5);
        rig.tick(20);
        assert_eq!(rig.received(), [vec![0x02, 0, 0xAA], vec![0x02, 1, 0xAA]]);
        // The meter moves once per 100 ms window.
        assert_eq!(rig.tick(130).level, 5);
        assert_eq!(rig.tick(240).level, 0);
        rig.engine.set_capture(false);
        let status = rig.tick(30);
        assert!(!status.live && status.level == 0);
        assert_eq!(rig.fake.live(), 0);
    }

    #[test]
    fn engine_follows_default_change() {
        let mut rig = Rig::new(Fake::with(&[BUILTIN, USB], Some("BuiltIn")), 10);
        rig.engine.set_capture(true);
        rig.tick(0);
        assert_eq!(rig.active().as_deref(), Some("BuiltIn"));
        rig.fake.set(&[BUILTIN, USB], Some("USB"));
        for ms in (10..=2000).step_by(10) {
            rig.fake.callback(10, 0.0);
            rig.tick(ms);
        }
        assert_eq!(rig.active().as_deref(), Some("USB"));
        assert_eq!(rig.fake.opens(), ["BuiltIn", "USB"]);
    }

    #[test]
    fn engine_uses_chosen_and_reports_it_missing() {
        let fake = Fake::with(&[BUILTIN], Some("BuiltIn"));
        let (sink, _frames) = tokio::sync::mpsc::channel(10);
        let mut engine: TestEngine = Engine::new(fake.clone(), working, Some("USB".into()), sink);
        engine.set_capture(true);
        let status = engine.tick(Instant::now());
        assert_eq!(status.problem, Some(MicProblem::ChosenMissing));
        assert_eq!(status.active.map(|d| d.uid).as_deref(), Some("BuiltIn"));
        // It comes back: capture moves to it.
        fake.set(&[BUILTIN, USB], Some("BuiltIn"));
        engine.choose(Some("USB".into()));
        let status = engine.tick(Instant::now());
        assert_eq!(status.problem, None);
        assert_eq!(status.active.map(|d| d.uid).as_deref(), Some("USB"));
    }

    #[test]
    fn engine_reports_no_device() {
        let mut rig = Rig::new(Fake::with(&[], None), 10);
        rig.engine.set_capture(true);
        let status = rig.tick(0);
        assert_eq!(status.problem, Some(MicProblem::NoDevice));
        assert!(!status.live);
    }

    #[test]
    fn engine_rebuilds_on_stall_with_backoff() {
        let mut rig = Rig::new(Fake::with(&[BUILTIN], Some("BuiltIn")), 10);
        rig.engine.set_capture(true);
        let mut builds = Vec::new();
        let mut failed = false;
        let mut ms = 0;
        while ms <= 60_000 {
            let before = rig.fake.opens().len();
            // Never calls back: every stream stalls.
            failed |= rig.tick(ms).problem == Some(MicProblem::Failed);
            if rig.fake.opens().len() > before {
                builds.push(ms);
            }
            ms += 100;
        }
        let gaps: Vec<u64> = builds.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(gaps.len() >= 4, "{builds:?}");
        assert!(gaps.iter().all(|gap| *gap >= 2000), "{gaps:?}");
        assert!(gaps.last().unwrap() > gaps.first().unwrap(), "{gaps:?}");
        assert!(failed);
    }

    #[test]
    fn engine_drops_packets_when_sink_full() {
        let mut rig = Rig::new(Fake::with(&[BUILTIN], Some("BuiltIn")), 2);
        rig.engine.set_capture(true);
        rig.tick(0);
        rig.fake.callback(480 * 5, 0.1);
        rig.tick(10);
        // Two fit; the rest are dropped, and capture carries on.
        let received = rig.received();
        assert_eq!(received.len(), 2);
        assert_eq!(received[0][1], 0);
        rig.fake.callback(480, 0.1);
        let status = rig.tick(20);
        assert!(status.live);
        assert_eq!(rig.received(), [vec![0x02, 5, 0xAA]]);
    }

    #[test]
    fn engine_reports_encoder_unavailable() {
        let mut rig = Rig::with_encoder(Fake::with(&[BUILTIN], Some("BuiltIn")), 10, broken);
        let status = rig.tick(0);
        assert_eq!(status.problem, Some(MicProblem::EncoderUnavailable));
        assert_eq!(status.devices.len(), 1);
        rig.engine.set_capture(true);
        let status = rig.tick(10);
        assert!(!status.live);
        assert!(rig.fake.opens().is_empty(), "never opens the device");
    }

    #[test]
    fn engine_stops_when_sink_closed() {
        let mut rig = Rig::new(Fake::with(&[BUILTIN], Some("BuiltIn")), 10);
        rig.engine.set_capture(true);
        rig.tick(0);
        assert_eq!(rig.fake.live(), 1);
        // The stream ended: the transport's receiver is gone.
        let Rig {
            fake,
            mut engine,
            frames,
            start,
        } = rig;
        drop(frames);
        fake.callback(480, 0.1);
        let status = engine.tick(start + Duration::from_millis(10));
        assert!(!status.live);
        assert_eq!(fake.live(), 0);
        // It stays off even while still asked to capture.
        engine.set_capture(true);
        engine.tick(start + Duration::from_millis(5000));
        assert_eq!(fake.live(), 0);
        assert_eq!(fake.opens().len(), 1);
    }

    #[test]
    fn engine_downmixes_stereo_devices() {
        let fake = Fake::with(&[BUILTIN], Some("BuiltIn"));
        fake.0.borrow_mut().format = Some((44_100, 2));
        let mut rig = Rig::new(fake, 10);
        rig.engine.set_capture(true);
        rig.tick(0);
        rig.fake.callback(480, 0.2);
        rig.tick(10);
        assert_eq!(
            rig.received().len(),
            1,
            "480 stereo frames make 480 mono samples"
        );
    }

    #[test]
    fn queued_controls_collapse_to_last() {
        let collapsed = collapse([
            MicControl::Capture(true),
            MicControl::Choose(Some("USB".into())),
            MicControl::Capture(false),
        ]);
        assert_eq!(
            collapsed,
            Collapsed {
                shutdown: false,
                choose: Some(Some("USB".into())),
                capture: Some(false),
            }
        );
        assert!(collapse([MicControl::Capture(true), MicControl::Shutdown]).shutdown);
    }

    #[test]
    fn mic_thread_exits_when_microphone_drops() {
        let (tx, rx) = mpsc::channel::<MicControl>();
        let (done_tx, done) = mpsc::channel();
        std::thread::spawn(move || {
            let (sink, _frames) = tokio::sync::mpsc::channel(10);
            let engine: TestEngine =
                Engine::new(Fake::with(&[BUILTIN], Some("BuiltIn")), working, None, sink);
            run(engine, &rx, |_| {});
            let _ = done_tx.send(());
        });
        tx.send(MicControl::Capture(true)).unwrap();
        tx.send(MicControl::Shutdown).unwrap();
        assert!(done.recv_timeout(Duration::from_secs(2)).is_ok());
        let (tx, rx) = mpsc::channel::<MicControl>();
        let (done_tx, done) = mpsc::channel();
        std::thread::spawn(move || {
            let (sink, _frames) = tokio::sync::mpsc::channel(10);
            let engine: TestEngine = Engine::new(Fake::default(), working, None, sink);
            run(engine, &rx, |_| {});
            let _ = done_tx.send(());
        });
        drop(tx);
        assert!(done.recv_timeout(Duration::from_secs(2)).is_ok());
    }

    #[test]
    fn bitrate_is_checked_against_applicable_ranges() {
        assert_eq!(choose_bitrate(&[], 32_000), 32_000);
        assert_eq!(choose_bitrate(&[(6_000.0, 510_000.0)], 32_000), 32_000);
        assert_eq!(
            choose_bitrate(&[(48_000.0, 64_000.0), (96_000.0, 128_000.0)], 32_000),
            48_000
        );
    }

    /// Streams 1 s of a 44.1 kHz sine in 10 ms chunks through the real encoder and decodes
    /// every packet with `opus-decoder`.
    #[test]
    fn audiotoolbox_opus_encodes_20ms_packets() {
        let mut encoder = AudioToolboxOpus::new(44_100).expect("this Mac has an Opus encoder");
        let mut packets = Vec::new();
        let mut packets_after = Vec::new();
        let tone =
            |i: usize| (i as f32 * 2.0 * std::f32::consts::PI * 440.0 / 44_100.0).sin() * 0.3;
        for chunk in 0..100 {
            let samples: Vec<f32> = (chunk * 441..(chunk + 1) * 441).map(tone).collect();
            let before = packets.len();
            encoder.encode(&samples, &mut packets).expect("encodes");
            if chunk >= 10 {
                packets_after.push(packets.len() - before);
            }
        }
        // Packets keep flowing: every 20 ms of input yields one, never a stall needing a reset.
        assert!(
            (46..=51).contains(&packets.len()),
            "{} packets",
            packets.len()
        );
        assert!(
            packets_after
                .windows(4)
                .all(|w| w.iter().sum::<usize>() >= 1),
            "{packets_after:?}"
        );
        let mut decoder = opus_decoder::OpusDecoder::new(24_000, 1).unwrap();
        let mut pcm = vec![0.0f32; 5760];
        let mut decoded = Vec::new();
        for packet in &packets {
            assert!((3..=200).contains(&packet.len()), "{} bytes", packet.len());
            // TOC: 20 ms frames.
            let samples = decoder.decode_float(packet, &mut pcm, false).unwrap();
            assert_eq!(samples, 480);
            decoded.extend_from_slice(&pcm[..samples]);
        }
        assert!(packets[5..].iter().all(|p| p.len() >= 20));
        // The sine comes out continuous: steady level, and 440 Hz worth of zero crossings.
        let settled = &decoded[2400..decoded.len() - 480];
        for window in settled.as_chunks::<480>().0 {
            let rms = (window.iter().map(|s| s * s).sum::<f32>() / 480.0).sqrt();
            assert!((0.12..=0.32).contains(&rms), "rms {rms}");
        }
        let span = &settled[..4800];
        let crossings = span
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        // 0.2 s of 440 Hz: 176 crossings.
        assert!((165..=187).contains(&crossings), "{crossings} crossings");
    }

    #[test]
    fn audiotoolbox_opus_follows_other_input_rates() {
        for rate in [16_000, 24_000, 48_000, 96_000] {
            let mut encoder = AudioToolboxOpus::new(rate).expect("encoder");
            let mut packets = Vec::new();
            let chunk = rate as usize / 100;
            for _ in 0..50 {
                encoder.encode(&vec![0.1; chunk], &mut packets).unwrap();
            }
            assert!(
                (22..=26).contains(&packets.len()),
                "{rate}: {}",
                packets.len()
            );
        }
    }
}
