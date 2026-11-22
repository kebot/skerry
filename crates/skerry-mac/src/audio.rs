//! Session audio: Opus decode on the session task, playback on a per-session `skerry-audio`
//! thread (`ARCH-GAP-DC-03`).
//!
//! Every Core Audio device and stream call runs only on that thread, never on GPUI or Tokio:
//! building or dropping a stream blocks. The thread follows the Mac's default output, or a
//! chosen device by its Core Audio UID, and rebuilds the stream on an error, a stall, a
//! default change, or the chosen device coming back.
//!
//! Health rules: the heartbeat is the render-callback count only (counted before the queue
//! lock, so a callback that loses the lock still counts). Muted or stopped audio leaves the
//! queue empty and is healthy, so the stream is never paused on mute. A stall is two seconds
//! without a callback while a stream exists.
//!
//! The stream always opens at 48 kHz f32: Core Audio's output unit converts to the device's
//! own rate. A one-channel device gets a stereo-to-mono downmix; a device with more than two
//! channels opens with two and Core Audio maps them.
//!
//! Dropping `Playback` never blocks: it sends `Shutdown` and detaches the thread.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use opus_decoder::OpusDecoder;
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const RATE: u32 = 48_000;
const CHANNELS: usize = 2;
/// 125 ms of stereo samples; even, so dropping the oldest keeps whole frames.
const QUEUE_LIMIT: usize = RATE as usize * CHANNELS / 8;
/// How often the audio thread checks the stream's health.
const TICK: Duration = Duration::from_millis(500);
/// How often the audio thread lists devices and reads the default.
const ENUMERATE_EVERY: Duration = Duration::from_secs(1);
/// No render callback for this long while a stream exists is a stall.
const STALL: Duration = Duration::from_secs(2);
/// At most one rebuild this often, for every cause but a person's pick.
const REBUILD_LIMIT: Duration = Duration::from_secs(1);
/// A stream that has called back this long clears its device's strikes.
const HEALTHY_AFTER: Duration = Duration::from_secs(5);
const BACKOFF_CAP: Duration = Duration::from_secs(30);

type Queue = Arc<Mutex<VecDeque<f32>>>;

/// An output device as the menus show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputDevice {
    /// The raw Core Audio UID (`kAudioDevicePropertyDeviceUID`).
    pub uid: String,
    /// The Core Audio name, with blanks filled in and repeats numbered.
    pub name: String,
}

/// Why sound isn't playing where the person asked, when it isn't.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputProblem {
    /// The chosen device isn't connected; the default plays, if there is one.
    ChosenMissing,
    /// The chosen device is connected but keeps failing; the default plays meanwhile.
    ChosenFailing,
    /// No output device at all.
    NoDevice,
    /// The output stopped and retries keep failing.
    Failed,
}

/// What the audio thread reports, only when it changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutputStatus {
    /// Connected devices, sorted by name.
    pub devices: Vec<OutputDevice>,
    pub default_uid: Option<String>,
    /// The device playing now.
    pub active: Option<OutputDevice>,
    /// The choice this status answers; `None` is System Default.
    pub chosen: Option<String>,
    pub problem: Option<OutputProblem>,
}

impl OutputStatus {
    pub fn device(&self, uid: &str) -> Option<&OutputDevice> {
        self.devices.iter().find(|device| device.uid == uid)
    }

    pub fn default_device(&self) -> Option<&OutputDevice> {
        self.device(self.default_uid.as_deref()?)
    }
}

enum Control {
    Choose(Option<String>),
    Shutdown,
}

pub struct Playback {
    queue: Queue,
    decoder: OpusDecoder,
    pcm: Vec<f32>,
    control: Sender<Control>,
}

impl Playback {
    /// Starts playback towards `chosen` (`None` follows the Mac's default). `report` gets
    /// each new status from the audio thread. `None` only if the decoder or the thread
    /// can't start; a missing device is a live status, not a failure.
    pub fn new(
        chosen: Option<String>,
        report: impl Fn(OutputStatus) + Send + 'static,
    ) -> Option<Self> {
        let decoder = OpusDecoder::new(RATE, CHANNELS).ok()?;
        let queue: Queue = Arc::new(Mutex::new(VecDeque::with_capacity(QUEUE_LIMIT)));
        let (control, controls) = mpsc::channel();
        let thread_queue = queue.clone();
        std::thread::Builder::new()
            .name("skerry-audio".into())
            .spawn(move || {
                let heartbeat = Arc::new(AtomicU64::new(0));
                let backend = CoreAudio::new(thread_queue, heartbeat.clone());
                run(Engine::new(backend, heartbeat, chosen), &controls, report);
            })
            .ok()?;
        Some(Self {
            queue,
            decoder,
            pcm: vec![0.0; 5760 * CHANNELS],
            control,
        })
    }

    /// Plays through `chosen` from now on (`None` follows the Mac's default). Never blocks.
    pub fn set_output(&self, chosen: Option<String>) {
        let _ = self.control.send(Control::Choose(chosen));
    }

    pub fn packet(&mut self, packet: &[u8]) {
        // We advertise audioRedundancy=false, so only the plain Opus payload is used.
        if packet.len() <= 2 || packet[1] & 0x7f != 0 {
            return;
        }
        let Ok(samples) = self
            .decoder
            .decode_float(&packet[2..], &mut self.pcm, false)
        else {
            return;
        };
        if let Ok(mut queue) = self.queue.lock() {
            push_bounded(&mut queue, &self.pcm[..samples * CHANNELS], QUEUE_LIMIT);
        }
    }

    pub fn clear(&mut self) {
        if let Ok(mut queue) = self.queue.lock() {
            queue.clear();
        }
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        // The thread drops its stream on its own; never join here (the session task and
        // `NativeSession`'s abort watchdog must not wait on Core Audio).
        let _ = self.control.send(Control::Shutdown);
    }
}

/// Appends `pcm`, dropping the oldest samples beyond `limit`.
fn push_bounded(queue: &mut VecDeque<f32>, pcm: &[f32], limit: usize) {
    let dropped = (queue.len() + pcm.len())
        .saturating_sub(limit)
        .min(queue.len());
    queue.drain(..dropped);
    queue.extend(pcm);
}

/// The render callback's body: beat first, then fill from the queue if it is free.
fn render(out: &mut [f32], queue: &Mutex<VecDeque<f32>>, heartbeat: &AtomicU64, channels: u16) {
    heartbeat.fetch_add(1, Ordering::Relaxed);
    match queue.try_lock() {
        Ok(mut queue) => fill(out, &mut queue, channels),
        Err(_) => out.fill(0.0),
    }
}

/// Fills `out` from interleaved stereo; one output channel takes `(L + R) / 2` per frame.
/// What the queue can't cover is silence.
fn fill(out: &mut [f32], queue: &mut VecDeque<f32>, channels: u16) {
    if channels == 1 {
        for sample in out {
            *sample = if queue.len() >= 2 {
                let left = queue.pop_front().unwrap_or(0.0);
                let right = queue.pop_front().unwrap_or(0.0);
                (left + right) * 0.5
            } else {
                0.0
            };
        }
    } else {
        for sample in out {
            *sample = queue.pop_front().unwrap_or(0.0);
        }
    }
}

/// The stream's channel count for a device whose default layout has `device_channels`.
fn stream_channels(device_channels: u16) -> u16 {
    if device_channels == 1 {
        1
    } else {
        2
    }
}

/// Devices as `(uid, name)` labelled for the menus: a blank name reads "Output Device n",
/// a repeated name gets " (2)", " (3)" (both numbered in UID order), and the list is
/// sorted by name, then UID.
fn label_devices(mut raw: Vec<(String, Option<String>)>) -> Vec<OutputDevice> {
    raw.sort_by(|a, b| a.0.cmp(&b.0));
    raw.dedup_by(|a, b| a.0 == b.0);
    let mut blank = 0;
    let named: Vec<(String, String)> = raw
        .into_iter()
        .map(|(uid, name)| {
            let name = name
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| {
                    blank += 1;
                    format!("Output Device {blank}")
                });
            (uid, name)
        })
        .collect();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut devices: Vec<OutputDevice> = named
        .into_iter()
        .map(|(uid, name)| {
            let count = seen.entry(name.clone()).or_insert(0);
            *count += 1;
            let name = if *count == 1 {
                name
            } else {
                format!("{name} ({count})")
            };
            OutputDevice { uid, name }
        })
        .collect();
    devices.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.uid.cmp(&b.uid))
    });
    devices
}

/// What the queued controls amount to: only the last pick counts.
#[derive(Debug, Default, PartialEq, Eq)]
struct Collapsed {
    shutdown: bool,
    choose: Option<Option<String>>,
}

fn collapse(controls: impl IntoIterator<Item = Control>) -> Collapsed {
    let mut collapsed = Collapsed::default();
    for control in controls {
        match control {
            Control::Choose(chosen) => collapsed.choose = Some(chosen),
            Control::Shutdown => collapsed.shutdown = true,
        }
    }
    collapsed
}

/// The Core Audio side, behind a seam so the loop runs in tests without hardware.
trait Backend {
    type Stream;
    /// Connected output devices as `(raw UID, name)`.
    fn devices(&mut self) -> Vec<(String, Option<String>)>;
    fn default_uid(&mut self) -> Option<String>;
    /// Opens and starts a stream on `uid`; its error callback sets `errored`.
    fn open(&mut self, uid: &str, errored: Arc<AtomicBool>) -> Result<Self::Stream, ()>;
}

struct CoreAudio {
    host: cpal::Host,
    queue: Queue,
    heartbeat: Arc<AtomicU64>,
    /// The last enumeration's devices by UID, so `open` needn't enumerate again.
    devices: Vec<(String, cpal::Device)>,
}

impl CoreAudio {
    fn new(queue: Queue, heartbeat: Arc<AtomicU64>) -> Self {
        Self {
            host: cpal::default_host(),
            queue,
            heartbeat,
            devices: Vec::new(),
        }
    }
}

impl Backend for CoreAudio {
    type Stream = cpal::Stream;

    fn devices(&mut self) -> Vec<(String, Option<String>)> {
        self.devices = self
            .host
            .output_devices()
            .map(|devices| {
                devices
                    .filter_map(|device| Some((device.id().ok()?.1, device)))
                    .collect()
            })
            .unwrap_or_default();
        self.devices
            .iter()
            .map(|(uid, device)| {
                let name = device
                    .description()
                    .ok()
                    .map(|description| description.name().to_string());
                (uid.clone(), name)
            })
            .collect()
    }

    fn default_uid(&mut self) -> Option<String> {
        Some(self.host.default_output_device()?.id().ok()?.1)
    }

    fn open(&mut self, uid: &str, errored: Arc<AtomicBool>) -> Result<cpal::Stream, ()> {
        let device = self
            .devices
            .iter()
            .find(|(known, _)| known == uid)
            .map(|(_, device)| device.clone())
            .ok_or(())?;
        let channels = stream_channels(
            device
                .default_output_config()
                .map_or(2, |config| config.channels()),
        );
        let config = cpal::StreamConfig {
            channels,
            sample_rate: RATE,
            buffer_size: cpal::BufferSize::Default,
        };
        let queue = self.queue.clone();
        let heartbeat = self.heartbeat.clone();
        let stream = device
            .build_output_stream(
                &config,
                move |out: &mut [f32], _| render(out, &queue, &heartbeat, channels),
                move |_| errored.store(true, Ordering::Relaxed),
                None,
            )
            .map_err(|_| ())?;
        stream.play().map_err(|_| ())?;
        Ok(stream)
    }
}

struct Active<S> {
    device: OutputDevice,
    _stream: S,
    errored: Arc<AtomicBool>,
    built: Instant,
    /// The default when this stream was built, if it was built on the default device:
    /// such a stream is rebuilt on any default change, so a chosen device stays put.
    built_on_default: Option<String>,
    last_beat: u64,
    last_change: Instant,
    /// Whether any callback has come since the build.
    beating: bool,
}

/// Consecutive failures or stalls of one device.
#[derive(Clone, Copy, Debug)]
struct Strikes {
    count: u32,
    retry_at: Instant,
    /// The last strike was a failed build (not a stall).
    build_failed: bool,
}

/// The wait before the next try after `count` strikes: none after the first, then 2, 4,
/// 8 … seconds, at most 30.
fn backoff(count: u32) -> Duration {
    if count <= 1 {
        Duration::ZERO
    } else {
        Duration::from_secs(1u64 << (count - 1).min(5)).min(BACKOFF_CAP)
    }
}

/// The audio thread's state, driven one tick at a time.
struct Engine<B: Backend> {
    backend: B,
    heartbeat: Arc<AtomicU64>,
    chosen: Option<String>,
    devices: Vec<OutputDevice>,
    default_uid: Option<String>,
    enumerated: Option<Instant>,
    stream: Option<Active<B::Stream>>,
    strikes: BTreeMap<String, Strikes>,
    last_rebuild: Option<Instant>,
    /// A person's pick waiting to apply: it skips the rate limit and the backoff.
    picked: bool,
}

impl<B: Backend> Engine<B> {
    fn new(backend: B, heartbeat: Arc<AtomicU64>, chosen: Option<String>) -> Self {
        Self {
            backend,
            heartbeat,
            chosen,
            devices: Vec::new(),
            default_uid: None,
            enumerated: None,
            stream: None,
            strikes: BTreeMap::new(),
            last_rebuild: None,
            picked: true,
        }
    }

    fn choose(&mut self, chosen: Option<String>) {
        if let Some(uid) = &chosen {
            self.strikes.remove(uid);
        }
        self.chosen = chosen;
        self.picked = true;
    }

    fn strike(&mut self, uid: &str, build_failed: bool, now: Instant) {
        let count = self.strikes.get(uid).map_or(0, |strikes| strikes.count) + 1;
        self.strikes.insert(
            uid.to_string(),
            Strikes {
                count,
                retry_at: now + backoff(count),
                build_failed,
            },
        );
    }

    /// Checks the stream; a stall or an error drops it and strikes its device.
    fn check_health(&mut self, now: Instant) {
        let beat = self.heartbeat.load(Ordering::Relaxed);
        let Some(active) = &mut self.stream else {
            return;
        };
        if beat != active.last_beat {
            active.last_beat = beat;
            active.last_change = now;
            active.beating = true;
        }
        let errored = active.errored.load(Ordering::Relaxed);
        if !errored && now.duration_since(active.last_change) < STALL {
            if active.beating && now.duration_since(active.built) >= HEALTHY_AFTER {
                let uid = active.device.uid.clone();
                self.strikes.remove(&uid);
            }
            return;
        }
        let uid = active.device.uid.clone();
        self.stream = None;
        self.strike(&uid, false, now);
    }

    fn enumerate(&mut self) {
        self.devices = label_devices(self.backend.devices());
        self.default_uid = self
            .backend
            .default_uid()
            .filter(|uid| self.devices.iter().any(|device| &device.uid == uid));
    }

    fn present(&self, uid: &str) -> bool {
        self.devices.iter().any(|device| device.uid == uid)
    }

    /// The device to play through now, and whether the chosen one is set aside for failing.
    fn target(&self, now: Instant) -> (Option<String>, bool) {
        let fallback = self
            .default_uid
            .clone()
            .or_else(|| self.devices.first().map(|device| device.uid.clone()));
        match &self.chosen {
            Some(uid) if self.present(uid) => {
                let resting = self.strikes.get(uid).is_some_and(|strikes| {
                    strikes.count >= 2 && strikes.retry_at > now && !self.picked
                });
                match fallback.filter(|fallback| resting && fallback != uid) {
                    Some(fallback) => (Some(fallback), true),
                    None => (Some(uid.clone()), false),
                }
            }
            _ => (fallback, false),
        }
    }

    fn needs_rebuild(&self, target: Option<&str>) -> bool {
        match (&self.stream, target) {
            (None, Some(_)) => true,
            (Some(_), None) => true,
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

    fn tick(&mut self, now: Instant) -> OutputStatus {
        self.check_health(now);
        let due = self
            .enumerated
            .is_none_or(|last| now.duration_since(last) >= ENUMERATE_EVERY);
        if due || self.picked {
            self.enumerate();
            self.enumerated = Some(now);
        }
        let (target, set_aside) = self.target(now);
        if self.needs_rebuild(target.as_deref()) {
            let limited = self
                .last_rebuild
                .is_some_and(|last| now.duration_since(last) < REBUILD_LIMIT);
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
        self.status(target.as_deref(), set_aside)
    }

    fn rebuild(&mut self, target: Option<&str>, now: Instant) {
        // Drop the old stream before opening the next: one AUHAL at a time.
        self.stream = None;
        let Some(uid) = target else {
            return;
        };
        self.last_rebuild = Some(now);
        let Some(device) = self.devices.iter().find(|d| d.uid == uid).cloned() else {
            return;
        };
        let errored = Arc::new(AtomicBool::new(false));
        match self.backend.open(uid, errored.clone()) {
            Ok(stream) => {
                self.stream = Some(Active {
                    built_on_default: (self.default_uid.as_deref() == Some(uid))
                        .then(|| uid.to_string()),
                    device,
                    _stream: stream,
                    errored,
                    built: now,
                    last_beat: self.heartbeat.load(Ordering::Relaxed),
                    last_change: now,
                    beating: false,
                });
            }
            Err(()) => self.strike(uid, true, now),
        }
    }

    fn status(&self, target: Option<&str>, set_aside: bool) -> OutputStatus {
        let playing = self.stream.as_ref().filter(|active| active.beating);
        let failing = target.is_some_and(|uid| {
            playing.is_none_or(|active| active.device.uid != uid)
                && self
                    .strikes
                    .get(uid)
                    .is_some_and(|strikes| strikes.build_failed || strikes.count >= 2)
        });
        // A failing chosen device with another output to fall back on is set aside, not a
        // stopped output: the next ticks play through the other one.
        let other_output = self
            .devices
            .iter()
            .any(|device| Some(device.uid.as_str()) != target);
        let chosen_target = target.is_some() && target == self.chosen.as_deref();
        let chosen_missing = self.chosen.as_deref().is_some_and(|uid| !self.present(uid));
        let problem = if failing && chosen_target && other_output {
            Some(OutputProblem::ChosenFailing)
        } else if failing {
            Some(OutputProblem::Failed)
        } else if chosen_missing {
            Some(OutputProblem::ChosenMissing)
        } else if target.is_none() {
            Some(OutputProblem::NoDevice)
        } else if set_aside {
            Some(OutputProblem::ChosenFailing)
        } else {
            None
        };
        OutputStatus {
            devices: self.devices.clone(),
            default_uid: self.default_uid.clone(),
            active: self.stream.as_ref().map(|active| active.device.clone()),
            chosen: self.chosen.clone(),
            problem,
        }
    }
}

/// The audio thread: ticks at once, then every `TICK` (sooner on a pick), reports changed
/// statuses, and ends on `Shutdown` or once `Playback` is gone, dropping its stream here.
/// A status sent after the session ended lands in a closed channel and is dropped.
fn run<B: Backend>(
    mut engine: Engine<B>,
    controls: &Receiver<Control>,
    report: impl Fn(OutputStatus),
) {
    let mut last = None;
    let mut wait = Duration::ZERO;
    loop {
        let first = match controls.recv_timeout(wait) {
            Ok(control) => Some(control),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        wait = TICK;
        let collapsed = collapse(first.into_iter().chain(controls.try_iter()));
        if collapsed.shutdown {
            return;
        }
        if let Some(chosen) = collapsed.choose {
            engine.choose(chosen);
        }
        let status = engine.tick(Instant::now());
        if last.as_ref() != Some(&status) {
            report(status.clone());
            last = Some(status);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Default)]
    struct World {
        devices: Vec<(String, Option<String>)>,
        default: Option<String>,
        broken: Vec<String>,
        opens: Vec<String>,
    }

    #[derive(Clone, Default)]
    struct Fake(Rc<RefCell<World>>);

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

        fn break_device(&self, uid: &str) {
            self.0.borrow_mut().broken.push(uid.into());
        }

        fn fix(&self) {
            self.0.borrow_mut().broken.clear();
        }
    }

    impl Backend for Fake {
        type Stream = ();
        fn devices(&mut self) -> Vec<(String, Option<String>)> {
            self.0.borrow().devices.clone()
        }
        fn default_uid(&mut self) -> Option<String> {
            self.0.borrow().default.clone()
        }
        fn open(&mut self, uid: &str, _: Arc<AtomicBool>) -> Result<(), ()> {
            let mut world = self.0.borrow_mut();
            world.opens.push(uid.into());
            if world.broken.iter().any(|broken| broken == uid) {
                Err(())
            } else {
                Ok(())
            }
        }
    }

    const SPEAKERS: (&str, &str) = ("BuiltIn", "MacBook Pro Speakers");
    const AIRPODS: (&str, &str) = ("AirPods", "AirPods Pro");

    struct Rig {
        fake: Fake,
        engine: Engine<Fake>,
        beat: Arc<AtomicU64>,
        start: Instant,
    }

    impl Rig {
        fn new(fake: Fake, chosen: Option<&str>) -> Self {
            let beat = Arc::new(AtomicU64::new(0));
            let engine = Engine::new(fake.clone(), beat.clone(), chosen.map(str::to_string));
            Self {
                fake,
                engine,
                beat,
                start: Instant::now(),
            }
        }

        fn at(&self, ms: u64) -> Instant {
            self.start + Duration::from_millis(ms)
        }

        /// Ticks at `ms`, with the render callback running (`beats`) or not.
        fn tick(&mut self, ms: u64, beats: bool) -> OutputStatus {
            if beats {
                self.beat.fetch_add(1, Ordering::Relaxed);
            }
            let now = self.at(ms);
            self.engine.tick(now)
        }

        /// Ticks every 500 ms from `from` to `to` inclusive.
        fn run(&mut self, from: u64, to: u64, beats: bool) -> OutputStatus {
            let mut status = OutputStatus::default();
            let mut ms = from;
            while ms <= to {
                status = self.tick(ms, beats);
                ms += 500;
            }
            status
        }

        fn active(&self) -> Option<String> {
            self.engine
                .stream
                .as_ref()
                .map(|active| active.device.uid.clone())
        }
    }

    fn devices(list: &[(&str, &str)]) -> Vec<OutputDevice> {
        label_devices(
            list.iter()
                .map(|(uid, name)| (uid.to_string(), Some(name.to_string())))
                .collect(),
        )
    }

    #[test]
    fn output_choice_follows_default_when_nothing_saved() {
        let mut rig = Rig::new(Fake::with(&[SPEAKERS, AIRPODS], Some("BuiltIn")), None);
        let status = rig.tick(0, true);
        assert_eq!(rig.active().as_deref(), Some("BuiltIn"));
        assert_eq!(status.problem, None);
        // AirPods become the default: the stream follows within a tick or two.
        rig.fake.set(&[SPEAKERS, AIRPODS], Some("AirPods"));
        let status = rig.run(500, 1500, true);
        assert_eq!(rig.active().as_deref(), Some("AirPods"));
        assert_eq!(
            status.active.map(|d| d.name).as_deref(),
            Some("AirPods Pro")
        );
    }

    #[test]
    fn output_choice_uses_saved_device_when_present() {
        let mut rig = Rig::new(
            Fake::with(&[SPEAKERS, AIRPODS], Some("BuiltIn")),
            Some("AirPods"),
        );
        let status = rig.tick(0, true);
        assert_eq!(rig.active().as_deref(), Some("AirPods"));
        assert_eq!(status.chosen.as_deref(), Some("AirPods"));
        assert_eq!(status.problem, None);
    }

    #[test]
    fn output_choice_falls_back_to_default_when_saved_device_missing() {
        let mut rig = Rig::new(Fake::with(&[SPEAKERS], Some("BuiltIn")), Some("AirPods"));
        let status = rig.tick(0, true);
        assert_eq!(rig.active().as_deref(), Some("BuiltIn"));
        assert_eq!(status.problem, Some(OutputProblem::ChosenMissing));
    }

    #[test]
    fn output_choice_returns_to_saved_device_when_it_reappears() {
        let mut rig = Rig::new(Fake::with(&[SPEAKERS], Some("BuiltIn")), Some("AirPods"));
        rig.run(0, 2000, true);
        rig.fake.set(&[SPEAKERS, AIRPODS], Some("BuiltIn"));
        let status = rig.run(2500, 4000, true);
        assert_eq!(rig.active().as_deref(), Some("AirPods"));
        assert_eq!(status.problem, None);
    }

    #[test]
    fn output_choice_is_none_without_devices() {
        let mut rig = Rig::new(Fake::with(&[], None), None);
        let status = rig.tick(0, false);
        assert_eq!(rig.active(), None);
        assert_eq!(status.problem, Some(OutputProblem::NoDevice));
        assert!(rig.fake.opens().is_empty());
        let mut rig = Rig::new(Fake::with(&[], None), Some("AirPods"));
        let status = rig.tick(0, false);
        assert_eq!(status.problem, Some(OutputProblem::ChosenMissing));
        assert_eq!(status.active, None);
    }

    #[test]
    fn output_choice_when_chosen_is_also_default() {
        // Speakers chosen while they are the default; headphones then become the default.
        let mut rig = Rig::new(
            Fake::with(&[SPEAKERS, AIRPODS], Some("BuiltIn")),
            Some("BuiltIn"),
        );
        rig.run(0, 1000, true);
        rig.fake.set(&[SPEAKERS, AIRPODS], Some("AirPods"));
        let status = rig.run(1500, 3000, true);
        // Rebuilt once (pinning the speakers whatever the default unit does), never moved.
        assert_eq!(rig.active().as_deref(), Some("BuiltIn"));
        assert_eq!(rig.fake.opens(), ["BuiltIn", "BuiltIn"]);
        assert_eq!(status.problem, None);
    }

    #[test]
    fn device_labels_fill_blank_names_and_number_duplicates() {
        let labelled = label_devices(vec![
            ("uid-c".into(), Some("USB Audio Device".into())),
            ("uid-z".into(), None),
            ("uid-a".into(), Some("USB Audio Device".into())),
            ("uid-b".into(), Some("airPods Pro".into())),
            ("uid-y".into(), Some("  ".into())),
            ("uid-m".into(), Some("MacBook Pro Speakers".into())),
        ]);
        let names: Vec<_> = labelled.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "airPods Pro",
                "MacBook Pro Speakers",
                "Output Device 1",
                "Output Device 2",
                "USB Audio Device",
                "USB Audio Device (2)",
            ]
        );
        // Numbered in UID order.
        assert_eq!(labelled[2].uid, "uid-y");
        assert_eq!(labelled[4].uid, "uid-a");
    }

    #[test]
    fn stall_triggers_rebuild_after_two_seconds_without_callbacks() {
        let mut rig = Rig::new(Fake::with(&[SPEAKERS], Some("BuiltIn")), None);
        rig.run(0, 1000, true);
        assert_eq!(rig.fake.opens().len(), 1);
        rig.run(1500, 2500, false);
        assert_eq!(rig.fake.opens().len(), 1, "not yet two seconds");
        rig.tick(3000, false);
        assert_eq!(rig.fake.opens().len(), 2);
    }

    #[test]
    fn steady_callbacks_never_trigger_rebuild() {
        let mut rig = Rig::new(Fake::with(&[SPEAKERS], Some("BuiltIn")), None);
        rig.run(0, 60_000, true);
        assert_eq!(rig.fake.opens().len(), 1);
    }

    #[test]
    fn muted_silence_still_counts_as_healthy() {
        // Muted: the queue stays empty, but the callback keeps running and beating.
        let queue = Mutex::new(VecDeque::new());
        let beat = AtomicU64::new(0);
        let mut out = [1.0; 64];
        render(&mut out, &queue, &beat, 2);
        assert_eq!(beat.load(Ordering::Relaxed), 1);
        assert!(out.iter().all(|sample| *sample == 0.0));
        let mut rig = Rig::new(Fake::with(&[SPEAKERS], Some("BuiltIn")), None);
        rig.run(0, 10_000, true);
        assert_eq!(rig.fake.opens().len(), 1);
    }

    #[test]
    fn callback_that_misses_lock_still_beats() {
        let queue = Mutex::new(VecDeque::from(vec![0.5; 8]));
        let beat = AtomicU64::new(0);
        let held = queue.lock().unwrap();
        let mut out = [1.0; 4];
        render(&mut out, &queue, &beat, 2);
        drop(held);
        assert_eq!(beat.load(Ordering::Relaxed), 1);
        assert_eq!(out, [0.0; 4]);
    }

    #[test]
    fn no_stall_while_no_stream() {
        let mut rig = Rig::new(Fake::with(&[], None), None);
        rig.run(0, 10_000, false);
        assert!(rig.fake.opens().is_empty());
        assert!(rig.engine.strikes.is_empty());
    }

    #[test]
    fn health_resets_after_rebuild() {
        let mut rig = Rig::new(Fake::with(&[SPEAKERS, AIRPODS], Some("BuiltIn")), None);
        rig.run(0, 1500, true);
        // A planned rebuild at 2 s (new default, seen at the next enumeration): the stall
        // clock starts again, so the silence since the last beat at 1.5 s doesn't count.
        rig.fake.set(&[SPEAKERS, AIRPODS], Some("AirPods"));
        rig.tick(2000, false);
        assert_eq!(rig.active().as_deref(), Some("AirPods"));
        rig.run(2500, 3500, false);
        assert_eq!(rig.fake.opens(), ["BuiltIn", "AirPods"]);
    }

    #[test]
    fn stream_error_triggers_immediate_rebuild() {
        let mut rig = Rig::new(Fake::with(&[SPEAKERS], Some("BuiltIn")), None);
        rig.run(0, 2000, true);
        rig.engine
            .stream
            .as_ref()
            .unwrap()
            .errored
            .store(true, Ordering::Relaxed);
        rig.tick(2500, true);
        assert_eq!(rig.fake.opens().len(), 2);
    }

    #[test]
    fn rebuilds_are_rate_limited_to_one_per_second() {
        let mut rig = Rig::new(Fake::with(&[SPEAKERS], Some("BuiltIn")), None);
        rig.tick(0, true);
        rig.engine
            .stream
            .as_ref()
            .unwrap()
            .errored
            .store(true, Ordering::Relaxed);
        rig.tick(500, true);
        assert_eq!(
            rig.fake.opens().len(),
            1,
            "within a second of the last build"
        );
        rig.tick(1000, true);
        assert_eq!(rig.fake.opens().len(), 2);
    }

    #[test]
    fn flapping_default_rebuilds_at_most_once_per_second() {
        let mut rig = Rig::new(Fake::with(&[SPEAKERS, AIRPODS], Some("BuiltIn")), None);
        rig.tick(0, true);
        for step in 1..=8u64 {
            let default = if step % 2 == 1 { "AirPods" } else { "BuiltIn" };
            rig.fake.set(&[SPEAKERS, AIRPODS], Some(default));
            rig.tick(step * 500, true);
        }
        // Eight changes over four seconds; at most one rebuild per second after the first.
        assert!(rig.fake.opens().len() <= 5, "{:?}", rig.fake.opens());
    }

    #[test]
    fn repeated_stalls_back_off_exponentially() {
        let mut rig = Rig::new(Fake::with(&[SPEAKERS], Some("BuiltIn")), None);
        let mut builds = Vec::new();
        let mut ms = 0;
        while ms <= 60_000 {
            let before = rig.fake.opens().len();
            rig.tick(ms, false);
            if rig.fake.opens().len() > before {
                builds.push(ms);
            }
            ms += 500;
        }
        let gaps: Vec<u64> = builds.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(gaps.len() >= 4, "{builds:?}");
        assert!(gaps.windows(2).all(|w| w[1] >= w[0]), "{gaps:?}");
        assert!(gaps.last().unwrap() > gaps.first().unwrap(), "{gaps:?}");
        assert!(gaps.iter().all(|gap| *gap <= 32_500), "{gaps:?}");
    }

    #[test]
    fn failing_chosen_device_falls_back_to_default() {
        let fake = Fake::with(&[SPEAKERS, AIRPODS], Some("BuiltIn"));
        fake.break_device("AirPods");
        let mut rig = Rig::new(fake, Some("AirPods"));
        let status = rig.run(0, 2000, true);
        assert_eq!(rig.active().as_deref(), Some("BuiltIn"));
        assert_eq!(status.problem, Some(OutputProblem::ChosenFailing));
        // It keeps retrying the chosen device, and goes back once it works.
        rig.fake.fix();
        let status = rig.run(2500, 40_000, true);
        assert_eq!(rig.active().as_deref(), Some("AirPods"));
        assert_eq!(status.problem, None);
    }

    #[test]
    fn first_stall_recovery_stays_silent_but_repeated_stalls_show_failed() {
        let mut rig = Rig::new(Fake::with(&[SPEAKERS], Some("BuiltIn")), None);
        rig.run(0, 1000, true);
        // A route-change hiccup: one stall, then the rebuilt stream plays.
        rig.run(1500, 3000, false);
        assert_eq!(rig.fake.opens().len(), 2);
        let status = rig.tick(3500, true);
        assert_eq!(status.problem, None);
        let mut rig = Rig::new(Fake::with(&[SPEAKERS], Some("BuiltIn")), None);
        // Never calls back: the rebuilt stream stalls too.
        let status = rig.run(0, 6000, false);
        assert_eq!(status.problem, Some(OutputProblem::Failed));
        // The first callback clears it.
        let mut status = status;
        let mut ms = 6500;
        while rig.engine.stream.is_none() && ms < 40_000 {
            status = rig.tick(ms, false);
            ms += 500;
        }
        assert_eq!(status.problem, Some(OutputProblem::Failed));
        let status = rig.tick(ms, true);
        assert_eq!(status.problem, None);
    }

    #[test]
    fn failed_build_shows_failed() {
        let fake = Fake::with(&[SPEAKERS], Some("BuiltIn"));
        fake.break_device("BuiltIn");
        let mut rig = Rig::new(fake, None);
        let status = rig.tick(0, false);
        assert_eq!(status.problem, Some(OutputProblem::Failed));
    }

    #[test]
    fn planned_rebuild_emits_no_transitional_status() {
        let mut rig = Rig::new(Fake::with(&[SPEAKERS, AIRPODS], Some("BuiltIn")), None);
        rig.run(0, 1500, true);
        rig.fake.set(&[SPEAKERS, AIRPODS], Some("AirPods"));
        let status = rig.tick(2000, true);
        assert_eq!(
            status,
            OutputStatus {
                devices: devices(&[SPEAKERS, AIRPODS]),
                default_uid: Some("AirPods".into()),
                active: Some(OutputDevice {
                    uid: "AirPods".into(),
                    name: "AirPods Pro".into()
                }),
                chosen: None,
                problem: None,
            }
        );
    }

    #[test]
    fn a_pick_applies_at_once() {
        let mut rig = Rig::new(Fake::with(&[SPEAKERS, AIRPODS], Some("BuiltIn")), None);
        rig.tick(0, true);
        rig.engine.choose(Some("AirPods".into()));
        rig.tick(100, true);
        assert_eq!(rig.active().as_deref(), Some("AirPods"));
    }

    #[test]
    fn queued_chooses_collapse_to_last() {
        let collapsed = collapse([
            Control::Choose(Some("AirPods".into())),
            Control::Choose(None),
            Control::Choose(Some("BuiltIn".into())),
        ]);
        assert_eq!(
            collapsed,
            Collapsed {
                shutdown: false,
                choose: Some(Some("BuiltIn".into()))
            }
        );
        assert!(collapse([Control::Choose(None), Control::Shutdown]).shutdown);
        assert_eq!(collapse([]), Collapsed::default());
    }

    #[test]
    fn status_sent_only_when_changed() {
        let (tx, rx) = mpsc::channel();
        let (statuses_tx, statuses) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            let fake = Fake::with(&[SPEAKERS], Some("BuiltIn"));
            let engine = Engine::new(fake, Arc::new(AtomicU64::new(0)), None);
            run(engine, &rx, move |status| {
                let _ = statuses_tx.send(status);
            });
        });
        std::thread::sleep(Duration::from_millis(1200));
        tx.send(Control::Shutdown).unwrap();
        handle.join().unwrap();
        // Two ticks or more, one unchanged status.
        assert_eq!(statuses.try_iter().count(), 1);
    }

    #[test]
    fn audio_thread_exits_when_playback_drops() {
        let (tx, rx) = mpsc::channel::<Control>();
        let (done_tx, done) = mpsc::channel();
        std::thread::spawn(move || {
            let fake = Fake::with(&[SPEAKERS], Some("BuiltIn"));
            run(
                Engine::new(fake, Arc::new(AtomicU64::new(0)), None),
                &rx,
                |_| {},
            );
            let _ = done_tx.send(());
        });
        drop(tx);
        assert!(done.recv_timeout(Duration::from_secs(2)).is_ok());
    }

    #[test]
    fn mono_device_gets_downmixed_stereo() {
        assert_eq!(stream_channels(1), 1);
        assert_eq!(stream_channels(2), 2);
        assert_eq!(stream_channels(8), 2);
    }

    #[test]
    fn mono_downmix_averages_frames() {
        let mut queue = VecDeque::from(vec![1.0, 0.0, 0.5, 0.5, -1.0, 1.0]);
        let mut out = [9.0; 3];
        fill(&mut out, &mut queue, 1);
        assert_eq!(out, [0.5, 0.5, 0.0]);
        assert!(queue.is_empty());
    }

    #[test]
    fn mono_underrun_pads_with_silence() {
        let mut queue = VecDeque::from(vec![1.0, 1.0]);
        let mut out = [9.0; 3];
        fill(&mut out, &mut queue, 1);
        assert_eq!(out, [1.0, 0.0, 0.0]);
    }

    #[test]
    fn fill_pads_silence_on_underrun() {
        let mut queue = VecDeque::from(vec![0.25, -0.25]);
        let mut out = [9.0; 4];
        fill(&mut out, &mut queue, 2);
        assert_eq!(out, [0.25, -0.25, 0.0, 0.0]);
    }

    #[test]
    fn queue_stays_bounded_at_125_ms_dropping_oldest() {
        let mut queue = VecDeque::new();
        for round in 0..20 {
            push_bounded(&mut queue, &[round as f32; 960 * CHANNELS], QUEUE_LIMIT);
        }
        assert_eq!(queue.len(), QUEUE_LIMIT);
        assert_eq!(queue.back(), Some(&19.0));
        assert_ne!(queue.front(), Some(&0.0));
    }

    #[test]
    fn queue_length_stays_even_after_drop_oldest() {
        assert!(QUEUE_LIMIT.is_multiple_of(2));
        let mut queue = VecDeque::new();
        for size in [120, 960, 1920, 240, 5760, 480] {
            push_bounded(&mut queue, &vec![0.0; size * CHANNELS], QUEUE_LIMIT);
            assert!(queue.len().is_multiple_of(2));
            assert!(queue.len() <= QUEUE_LIMIT);
        }
    }
}
