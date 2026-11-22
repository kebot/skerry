use crate::audio::{OutputStatus, Playback};
use crate::clipboard;
use crate::selkies;
use crate::settings::{Key, ScreenChange, ScreenPolicy, Settings};
use crate::video::{self, Codec, Decoder};
use base64::{engine::general_purpose::STANDARD, Engine};
use futures_util::{SinkExt, StreamExt};
use gpui_kit::RenderImage;
use image::{ImageFormat, ImageReader};
use reqwest::Url;
use serde_json::{json, Value};
use smallvec::smallvec;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::{
    client::IntoClientRequest, protocol::WebSocketConfig, Message,
};

const MAX_WIDTH: u16 = 4096;
const MAX_HEIGHT: u16 = 2160;
const WAIT_FOR_VIDEO: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resolution(pub u16, pub u16);

impl Resolution {
    pub fn manual(width: &str, height: &str) -> Option<Self> {
        let (width, height): (u16, u16) = (width.trim().parse().ok()?, height.trim().parse().ok()?);
        ((2..=MAX_WIDTH).contains(&width) && (2..=MAX_HEIGHT).contains(&height))
            .then(|| Self::from_window(width, height))
    }

    pub fn from_window(width: u16, height: u16) -> Self {
        let scale = (f64::from(MAX_WIDTH) / f64::from(width.max(1)))
            .min(f64::from(MAX_HEIGHT) / f64::from(height.max(1)))
            .min(1.0);
        let even = |size| ((f64::from(size) * scale) as u16 & !1).max(2);
        Self(even(width), even(height))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DisplayRequest {
    pub resolution: Resolution,
    pub scale: Option<f32>,
}

impl DisplayRequest {
    fn settings(self) -> Value {
        let Resolution(width, height) = self.resolution;
        match self.scale {
            None => {
                json!({"manual_resolution": true, "manual_width": width, "manual_height": height, "displayScale": 1})
            }
            Some(scale) => json!({"manual_resolution": false, "displayScale": scale}),
        }
    }
}

#[derive(Clone, Default)]
pub struct Frame {
    pub width: u16,
    pub height: u16,
    pub picture: Option<Picture>,
}

#[derive(Clone)]
pub enum Picture {
    Jpeg(JpegPicture),
    Video(video::Surface),
}

/// The JPEG stripes on screen, top to bottom (`ARCH-034`). Each stripe is its own
/// `RenderImage`, so a changed stripe uploads only its own rows, and nothing copies the
/// whole canvas. The view retires a replaced stripe with `drop_image` four paints after it
/// leaves the screen.
#[derive(Clone)]
pub struct JpegPicture {
    pub stripes: Arc<[JpegStripe]>,
}

#[derive(Clone)]
pub struct JpegStripe {
    pub y: u16,
    pub height: u16,
    /// BGRA, `height + 2` rows: the first and last rows repeat, so linear sampling at a
    /// non-1:1 scale never reads a neighbouring atlas tile.
    pub image: Arc<RenderImage>,
}

/// Decodes one stripe straight to BGRA into a buffer padded by one repeated row at the top
/// and bottom. Reads the headers before allocating: the width must be `width`, and the
/// height not 0 and at most `rows_left`. The decoder is not strict (as in `image`), so scan
/// data cut short decodes with filler; a stripe cut inside its headers is rejected.
fn decode_stripe(jpeg: &[u8], width: u16, rows_left: u16) -> Option<(u16, Vec<u8>)> {
    use zune_core::{bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions};
    let options = DecoderOptions::default()
        .jpeg_set_out_colorspace(ColorSpace::BGRA)
        .set_max_width(usize::from(width))
        .set_max_height(usize::from(rows_left));
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(jpeg), options);
    decoder.decode_headers().ok()?;
    let (stripe_width, height) = decoder.dimensions()?;
    let height = u16::try_from(height).ok()?;
    if stripe_width != usize::from(width) || height == 0 || height > rows_left {
        return None;
    }
    let row = usize::from(width) * 4;
    let rows = usize::from(height);
    if decoder.output_buffer_size()? != row * rows {
        return None;
    }
    let mut pixels = vec![0u8; row * (rows + 2)];
    decoder
        .decode_into(&mut pixels[row..row * (rows + 1)])
        .ok()?;
    pixels.copy_within(row..2 * row, 0);
    pixels.copy_within(rows * row..(rows + 1) * row, (rows + 1) * row);
    Some((height, pixels))
}

#[derive(Default)]
struct Canvas {
    width: u16,
    height: u16,
    /// Keyed by top row; stored stripes never overlap.
    stripes: BTreeMap<u16, JpegStripe>,
}

impl Canvas {
    /// Sets the remote size; a new size clears every stripe. Returns whether it changed.
    fn resize(&mut self, width: u16, height: u16) -> Result<bool, &'static str> {
        if !valid_resolution(width, height) {
            return Err("The remote display resolution is outside the supported range.");
        }
        let changed = (self.width, self.height) != (width, height);
        if changed {
            *self = Self {
                width,
                height,
                stripes: BTreeMap::new(),
            };
        }
        Ok(changed)
    }

    /// Decodes a stripe for row `y` without touching the canvas.
    fn decode(&self, y: u16, jpeg: &[u8]) -> Option<JpegStripe> {
        let rows_left = self.height.checked_sub(y).filter(|rows| *rows > 0)?;
        let (height, pixels) = decode_stripe(jpeg, self.width, rows_left)?;
        let buffer = image::RgbaImage::from_raw(self.width.into(), u32::from(height) + 2, pixels)?;
        Some(JpegStripe {
            y,
            height,
            image: Arc::new(RenderImage::new(smallvec![image::Frame::new(buffer)])),
        })
    }

    /// Stores a stripe after removing every stored stripe it overlaps, so a change in the
    /// server's stripe count or geometry leaves nothing stale.
    fn insert(&mut self, stripe: JpegStripe) {
        let (top, bottom) = (stripe.y, stripe.y + stripe.height);
        if let Some((&y, above)) = self.stripes.range(..top).next_back() {
            if y + above.height > top {
                self.stripes.remove(&y);
            }
        }
        let covered: Vec<u16> = self.stripes.range(top..bottom).map(|(y, _)| *y).collect();
        for y in covered {
            self.stripes.remove(&y);
        }
        self.stripes.insert(top, stripe);
    }

    fn snapshot(&self) -> Frame {
        Frame {
            width: self.width,
            height: self.height,
            picture: (!self.stripes.is_empty()).then(|| {
                Picture::Jpeg(JpegPicture {
                    stripes: self.stripes.values().cloned().collect(),
                })
            }),
        }
    }
}

/// How long the stream must be quiet before the stripes gathered so far are shown.
const SETTLE: Duration = Duration::from_millis(4);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PresentReason {
    /// A stripe of a new frame id arrived, so the previous frame is complete.
    Boundary,
    /// No stripe arrived for `SETTLE`.
    Settle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Present {
    reason: PresentReason,
    /// Whether this shows a frame id not shown before (`ARCH-021` counts frames per id).
    new_frame: bool,
    /// From the last gathered stripe to this present.
    delay: Duration,
}

/// Presents JPEG stripes at frame-id boundaries, like the Selkies web client, with a
/// `SETTLE` deadline that each stripe pushes back, for the last frame of a burst
/// (`ARCH-034`).
#[derive(Default)]
struct JpegPacer {
    /// The frame id gathered and not yet shown.
    pending: Option<u16>,
    last_stripe: Option<Instant>,
    shown: Option<u16>,
}

impl JpegPacer {
    /// Records a decoded stripe of frame `id`. When it starts a new frame, returns the
    /// present of the previous one, which is due *before* this stripe is applied.
    fn stripe(&mut self, id: u16, now: Instant) -> Option<Present> {
        let boundary = if self.pending.is_some_and(|pending| pending != id) {
            self.present(PresentReason::Boundary, now)
        } else {
            None
        };
        self.pending = Some(id);
        self.last_stripe = Some(now);
        boundary
    }

    /// When the settle present is due; `None` while nothing is pending.
    fn deadline(&self) -> Option<Instant> {
        self.pending.and(self.last_stripe).map(|last| last + SETTLE)
    }

    /// Takes the pending frame for a present.
    fn present(&mut self, reason: PresentReason, now: Instant) -> Option<Present> {
        let id = self.pending.take()?;
        let new_frame = self.shown != Some(id);
        self.shown = Some(id);
        Some(Present {
            reason,
            new_frame,
            delay: self
                .last_stripe
                .map_or(Duration::ZERO, |last| now.saturating_duration_since(last)),
        })
    }

    /// Forgets everything: the stream left JPEG (a hardware frame or codec switch) or
    /// resized, so a pending settle can never show a stale picture.
    fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Logs a JPEG present and its delay from the last stripe when `SKERRY_PRESENT_TRACE=1`.
fn trace_present(present: &Present) {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let enabled = *ENABLED
        .get_or_init(|| std::env::var("SKERRY_PRESENT_TRACE").is_ok_and(|value| value == "1"));
    if enabled {
        eprintln!(
            "skerry present: {:?} delay_ms={:.3}",
            present.reason,
            present.delay.as_secs_f64() * 1000.0
        );
    }
}

/// Shows the gathered stripes; a snapshot is only a list of `Arc`s.
fn present_jpeg(
    frames: &watch::Sender<Frame>,
    canvas: &Canvas,
    diagnostics: &mut Diagnostics,
    present: Present,
) {
    trace_present(&present);
    let picture = canvas.snapshot();
    if picture.picture.is_some() && present.new_frame {
        diagnostics.frames += 1;
    }
    frames.send_replace(picture);
}

#[derive(Clone, Debug)]
pub struct StreamSample {
    pub width: u16,
    pub height: u16,
    pub fps: f64,
    pub wire_mbps: f64,
    pub decode_ms: f64,
    pub dropped: u64,
    pub client_codec: Option<Codec>,
    pub rtt_ms: Option<f64>,
    /// The server's `stream_stats` figures, when fresh (`ARCH-019`).
    pub server: Option<ServerFigures>,
    /// Stream bytes received since the stats opened.
    pub received_bytes: u64,
    /// Key frames requested since the stats opened.
    pub keyframe_requests: u64,
}

/// Server figures older than this are left out of a sample (the web's `SERVER_FRESH_MS`).
const SERVER_FRESH: Duration = Duration::from_secs(3);

/// One `stream_stats` message: its numeric figures by key, and whether the
/// server is holding frames back.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ServerFigures {
    pub numbers: BTreeMap<String, f64>,
    pub throttled: bool,
}

impl ServerFigures {
    pub fn parse(stats: &Value) -> Self {
        let numbers = stats
            .as_object()
            .map(|object| {
                object
                    .iter()
                    .filter_map(|(key, value)| Some((key.clone(), value.as_f64()?)))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            numbers,
            throttled: stats["throttled"].as_bool().unwrap_or(false),
        }
    }

    pub fn get(&self, key: &str) -> Option<f64> {
        self.numbers.get(key).copied()
    }
}

/// The server's description of the stream (`stream_info`), as Selkies'
/// `ScreenCapture::stream_info` sends it.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct StreamInfo {
    pub backend: String,
    pub capture: String,
    pub zero_copy: bool,
    pub zero_copy_available: bool,
    pub capture_reason: String,
    pub encoder: String,
    pub hardware: bool,
    pub gpu_present: Option<bool>,
    pub hardware_expected: bool,
    pub encoder_reason: String,
    pub codec: String,
    pub fullcolor: bool,
    pub bit_depth: Option<u8>,
    pub striped: bool,
    pub gpu: String,
    pub driver: String,
    pub encode_node: String,
    pub renderer: String,
    pub render_node: String,
    pub render_gpu: String,
    pub renderer_reason: String,
}

#[derive(Default)]
struct Diagnostics {
    bytes: usize,
    frames: u64,
    packets: u64,
    codec: Option<Codec>,
    dropped: u64,
    decode: Duration,
    rtt_ms: Option<f64>,
    stats_on: bool,
    received_bytes: u64,
    keyframe_requests: u64,
    server: Option<(Instant, ServerFigures)>,
}

impl Diagnostics {
    /// The stats opened or shut: counts start again from the opening.
    fn set_stats(&mut self, on: bool) {
        self.stats_on = on;
        self.received_bytes = 0;
        self.keyframe_requests = 0;
        self.server = None;
    }

    fn sample(&mut self, elapsed: Duration, width: u16, height: u16) -> StreamSample {
        let seconds = elapsed.as_secs_f64();
        StreamSample {
            width,
            height,
            fps: std::mem::take(&mut self.frames) as f64 / seconds,
            wire_mbps: std::mem::take(&mut self.bytes) as f64 * 8.0 / 1_000_000.0 / seconds,
            decode_ms: std::mem::take(&mut self.decode).as_secs_f64() * 1000.0
                / std::mem::take(&mut self.packets).max(1) as f64,
            client_codec: self.codec,
            dropped: self.dropped,
            rtt_ms: self.rtt_ms,
            server: self
                .server
                .as_ref()
                .filter(|(at, _)| at.elapsed() <= SERVER_FRESH)
                .map(|(_, figures)| figures.clone()),
            received_bytes: self.received_bytes,
            keyframe_requests: self.keyframe_requests,
        }
    }
}

#[derive(Clone)]
pub enum RemoteCursor {
    Hidden,
    Image {
        image: Arc<RenderImage>,
        hotspot: (u16, u16),
        size: (u16, u16),
    },
}

#[derive(serde::Deserialize)]
struct CursorMessage<'a> {
    #[serde(borrow)]
    curdata: &'a str,
    #[serde(default)]
    handle: Option<CursorNumber>,
    #[serde(default)]
    hotx: Option<CursorNumber>,
    #[serde(default)]
    hoty: Option<CursorNumber>,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum CursorNumber {
    Integer(u32),
    Text(String),
}

impl CursorNumber {
    fn value(self) -> Option<u32> {
        match self {
            Self::Integer(value) => Some(value),
            Self::Text(value) => value.parse().ok(),
        }
    }
}

impl RemoteCursor {
    fn parse(text: &str) -> Option<Self> {
        if text.len() > 512 * 1024 {
            return None;
        }
        let message: CursorMessage<'_> = serde_json::from_str(text).ok()?;
        if message.handle.and_then(CursorNumber::value) == Some(0) || message.curdata.is_empty() {
            return Some(Self::Hidden);
        }
        let png = STANDARD.decode(message.curdata).ok()?;
        let mut reader = ImageReader::with_format(Cursor::new(png), ImageFormat::Png);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(256);
        limits.max_image_height = Some(256);
        limits.max_alloc = Some(2 * 1024 * 1024);
        reader.limits(limits);
        let mut pixels = reader.decode().ok()?.into_rgba8();
        let (width, height) = (pixels.width() as u16, pixels.height() as u16);
        for pixel in pixels.pixels_mut() {
            pixel.0.swap(0, 2);
        }
        let image = Arc::new(RenderImage::new(smallvec![image::Frame::new(pixels)]));
        let hotspot = |value: Option<CursorNumber>, extent: u16| {
            value
                .and_then(CursorNumber::value)
                .unwrap_or_default()
                .min(u32::from(extent.saturating_sub(1))) as u16
        };
        Some(Self::Image {
            image,
            hotspot: (hotspot(message.hotx, width), hotspot(message.hoty, height)),
            size: (width, height),
        })
    }
}

pub enum SessionEvent {
    Audio(bool),
    /// No audio this session: the server turned it off, or it couldn't start on this Mac.
    AudioUnavailable(&'static str),
    /// The output devices and where sound plays (`ARCH-GAP-DC-03`).
    AudioOutput(OutputStatus),
    Video(bool),
    Settings(Settings),
    Stats(StreamSample),
    StreamInfo(Box<StreamInfo>),
    ResizeAllowed(bool),
    ScreenPolicy(ScreenPolicy),
    Cursor(RemoteCursor),
    FingerScroll(bool),
    ClipboardPolicy(clipboard::Policy),
    Clipboard(clipboard::Incoming),
    /// The session's file routes, once the session cookie is in hand (`ARCH-023`).
    Files(selkies::SelkiesClient),
    FilePolicy(selkies::Policy),
    Error(String),
    /// The start-up grace ran out with the session still answering like one starting.
    StartupTimeout,
}

#[derive(Clone, Copy)]
pub struct Pointer {
    pub display: Resolution,
    pub position: (u16, u16),
    pub buttons: u8,
}

impl Pointer {
    fn command_for(self, display: Resolution) -> Option<String> {
        (self.display == display).then(|| {
            let (x, y) = self.position;
            format!("m,{x},{y},{},0", self.buttons)
        })
    }
}

fn zoom_commands(
    notches: i8,
    position: Pointer,
    display: Resolution,
    control_held: bool,
) -> Vec<String> {
    let Some(move_pointer) = position.command_for(display).filter(|_| notches != 0) else {
        return Vec::new();
    };
    let mask = position.buttons | if notches > 0 { 16 } else { 8 };
    let mut commands = vec![move_pointer];
    if !control_held {
        commands.push("kd,65507".into());
    }
    commands.push(format!("m2,0,0,{mask},{}", notches.unsigned_abs()));
    commands.push(format!("m2,0,0,{},0", position.buttons));
    if !control_held {
        commands.push("ku,65507".into());
    }
    commands
}

pub enum Input {
    Command(String),
    Pointer(Pointer),
    Zoom(i8, Pointer),
    Audio(bool),
    /// Play through this output device's Core Audio UID; `None` follows the Mac's default.
    AudioOutput(Option<String>),
    /// Ask the server for this audio bitrate (bps).
    AudioBitrate(u32),
    Video(bool),
    Settings(Vec<(Key, u32)>),
    Resolution(DisplayRequest),
    Screen(ScreenChange),
    Appearance(bool),
    /// Write this into the session's clipboard (`ARCH-017`).
    Clipboard(clipboard::Content),
    /// Subscribe to (or drop) the server's `stream_stats` (`_stats,1` / `_stats,0`).
    Stats(bool),
    /// End the stream: release every held key, then send a close frame (`ARCH-GAP-DC-09`).
    Close,
}

/// What a closing stream sends before its close frame: a `ku` for every key the session
/// still holds. No `kr`: on a clean close the server already resets the keyboard of the
/// departing input owner, and every reset injects Super, Meta and Escape releases.
fn closing_commands(held: &BTreeSet<u32>) -> Vec<String> {
    held.iter().map(|key| format!("ku,{key}")).collect()
}

/// Whether `SKERRY_INPUT_TRACE=1` asks for a trace of key traffic on stderr. Read once.
fn input_trace_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("SKERRY_INPUT_TRACE").is_ok_and(|value| value == "1"))
}

/// Logs one key command (`kd`, `ku`, `kh`, `kr`) the transport writes, when tracing.
fn trace_input(text: &str) {
    if input_trace_enabled() {
        if let Some(line) = redact_key_command(text) {
            eprintln!("skerry input: {line}");
        }
    }
}

/// Logs a stream event that matters to key traffic (close frame, watchdog abort).
pub fn trace_input_event(event: &str) {
    if input_trace_enabled() {
        eprintln!("skerry input: {event}");
    }
}

/// A key command with every printable keysym hidden, so the trace never records typed
/// text. Modifiers, function and navigation keys (0xff00 to 0xffff) keep their numbers.
/// Anything other than `kd`, `ku`, `kh` and `kr` gives `None`.
fn redact_key_command(text: &str) -> Option<String> {
    if text == "kr" {
        return Some(text.into());
    }
    let (verb, keys) = text.split_once(',')?;
    if !matches!(verb, "kd" | "ku" | "kh") {
        return None;
    }
    let keys: Vec<String> = keys
        .split(',')
        .map(|key| match key.parse::<u32>() {
            // Keypad digits and operators (0xff80 to 0xffbd) are typed text too.
            Ok(keysym)
                if (0xff00..=0xffff).contains(&keysym) && !(0xff80..=0xffbd).contains(&keysym) =>
            {
                keysym.to_string()
            }
            _ => "<char>".into(),
        })
        .collect();
    Some(format!("{verb},{}", keys.join(",")))
}

pub struct SessionOptions {
    pub preferences: Vec<(Key, u32)>,
    pub dark: bool,
    pub muted: bool,
    pub codec: Codec,
    pub display: Option<DisplayRequest>,
    /// Whether the Stats section is open, so the session subscribes once ready.
    pub stats: bool,
    /// How long start-up errors are retried before [`RunError::StartupTimeout`]: 45 s, or
    /// [`KEEP_WAITING_GRACE`] after the person chose Keep Waiting (`ARCH-027`).
    pub startup_grace: Duration,
    /// The waiting overlay's text until the first frame.
    pub waiting: String,
    /// The saved output device's Core Audio UID; `None` follows the Mac's default.
    pub audio_output: Option<String>,
    /// The saved audio bitrate (bps), requested once ready if the server offers it.
    pub audio_bitrate: Option<u32>,
}

/// Why a stream ended with an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunError {
    /// The session kept answering like one still starting until the grace ran out: it
    /// may still be booting, so the window offers Retry and Keep Waiting.
    StartupTimeout,
    Other(&'static str),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StartupTimeout => f.write_str("The session hasn’t answered yet."),
            Self::Other(message) => f.write_str(message),
        }
    }
}

impl From<&'static str> for RunError {
    fn from(message: &'static str) -> Self {
        Self::Other(message)
    }
}

/// Whether a failed WebSocket attempt looks like a session still starting: `None` is an
/// attempt that timed out (the session accepts TCP but hangs, R7), like a refusal or a
/// proxy's 404/5xx.
fn attempt_transient(failure: Result<(), Option<&tokio_tungstenite::tungstenite::Error>>) -> bool {
    match failure {
        Ok(()) => false,
        Err(None) => true,
        Err(Some(error)) => transient_socket_error(error),
    }
}

/// What to do after one failed WebSocket attempt during start-up.
#[derive(Debug, PartialEq, Eq)]
enum Retry {
    Again,
    Timeout,
    Fail,
}

/// A start-up error (a proxy's 404/5xx, a refused connection, or an attempt that timed
/// out while the session accepts TCP but hangs) is retried until `give_up`, then becomes a
/// [`RunError::StartupTimeout`]. Anything else fails at once.
fn retry_decision(transient: bool, now: Instant, give_up: Instant) -> Retry {
    match (transient, now < give_up) {
        (true, true) => Retry::Again,
        (true, false) => Retry::Timeout,
        (false, _) => Retry::Fail,
    }
}

struct HardwareVideo {
    codec: Codec,
    decoder: Decoder,
    decoded: bool,
    deadline: Option<Instant>,
    next_keyframe_request: Instant,
}

impl HardwareVideo {
    fn new(codec: Codec, now: Instant) -> Self {
        Self {
            codec,
            decoder: Decoder::default(),
            decoded: false,
            deadline: Some(now + WAIT_FOR_VIDEO),
            next_keyframe_request: now,
        }
    }

    fn decode(&mut self, packet: &video::Packet<'_>) -> Option<Frame> {
        let (width, height) = packet.size;
        if !valid_resolution(width, height) {
            return None;
        }
        let surface = self.decoder.decode(packet).ok()??;
        self.decoded = true;
        self.deadline = None;
        Some(Frame {
            width,
            height,
            picture: Some(Picture::Video(surface)),
        })
    }

    fn expired(&self, now: Instant) -> bool {
        self.deadline.is_some_and(|deadline| now >= deadline)
    }

    fn recover(&mut self, now: Instant) -> bool {
        self.deadline.get_or_insert(now + WAIT_FOR_VIDEO);
        if now < self.next_keyframe_request {
            return false;
        }
        self.next_keyframe_request = now + Duration::from_secs(1);
        true
    }
}

fn finger_scroll_capability(text: &str) -> Option<bool> {
    let payload = text.strip_prefix("DISPLAY_CONFIG_UPDATE,").unwrap_or(text);
    let data: Value = serde_json::from_str(payload).ok()?;
    if data["type"] != "display_config_update" {
        return None;
    }
    Some(data["finger_scroll"].as_bool().unwrap_or(false))
}

/// Runs `work` unless the stream is cancelled first (`None`). A dropped sender counts as
/// cancelled: the session that owned it is gone.
async fn unless_cancelled<T>(
    cancel: &mut watch::Receiver<bool>,
    work: impl std::future::Future<Output = T>,
) -> Option<T> {
    tokio::select! {
        biased;
        _ = cancel.wait_for(|cancelled| *cancelled) => None,
        out = work => Some(out),
    }
}

/// Runs one Selkies stream. `cancel` turns true when the session drops this stream: until
/// the main loop reads `Input::Close`, the transport checks it so a stream dropped while
/// still connecting never opens its socket or sends SETTINGS (`ARCH-GAP-DC-09`).
pub async fn run(
    url: Url,
    frames: watch::Sender<Frame>,
    mut input: mpsc::UnboundedReceiver<Input>,
    pointer: watch::Receiver<Option<Pointer>>,
    options: SessionOptions,
    events: &mpsc::UnboundedSender<SessionEvent>,
    mut cancel: watch::Receiver<bool>,
) -> Result<(), RunError> {
    let Some(requested) = unless_cancelled(&mut cancel, socket_request(url)).await else {
        return Ok(());
    };
    let (template, access) = requested?;
    if let Some(access) = access {
        let _ = events.send(SessionEvent::Files(access));
    }
    let config = WebSocketConfig::default().max_message_size(Some(16 * 1024 * 1024));
    // A just-launched session answers 404/5xx or refuses connections until Selkies is up, so
    // keep retrying transient failures for a while instead of failing the first attempt.
    let give_up = Instant::now() + options.startup_grace;
    let mut socket = loop {
        let mut request = template
            .uri()
            .clone()
            .into_client_request()
            .map_err(|_| "Couldn’t connect to the Selkies WebSocket.")?;
        request.headers_mut().extend(template.headers().clone());
        let attempt = tokio::time::timeout(
            Duration::from_secs(10),
            tokio_tungstenite::connect_async_with_config(request, Some(config), false),
        );
        let Some(attempt) = unless_cancelled(&mut cancel, attempt).await else {
            return Ok(());
        };
        let transient = match attempt {
            Ok(Ok((socket, _))) => break socket,
            Ok(Err(error)) => attempt_transient(Err(Some(&error))),
            Err(_) => attempt_transient(Err(None)),
        };
        match retry_decision(transient, Instant::now(), give_up) {
            Retry::Again => {
                let wait = tokio::time::sleep(Duration::from_millis(1500));
                if unless_cancelled(&mut cancel, wait).await.is_none() {
                    return Ok(());
                }
            }
            Retry::Timeout => return Err(RunError::StartupTimeout),
            Retry::Fail => return Err("Couldn’t connect to the Selkies WebSocket.".into()),
        }
    };
    let mut codec = if options.codec.supported() {
        options.codec
    } else {
        Codec::Jpeg
    };
    let mut hardware = (codec != Codec::Jpeg).then(|| HardwareVideo::new(codec, Instant::now()));
    let mut settings = options.display.map_or_else(
        || json!({"manual_resolution": false, "displayScale": 1}),
        DisplayRequest::settings,
    );
    if let Some(display) = options.display {
        let Resolution(width, height) = display.resolution;
        settings["initialClientWidth"] = json!(width);
        settings["initialClientHeight"] = json!(height);
    }
    settings["encoder"] = json!(codec.wire());
    settings["encoderFallback"] = json!(true);
    settings["video_10bit"] = json!(false);
    settings["video_fullcolor"] = json!(false);
    settings["displayId"] = json!("primary");
    settings["audioRedundancy"] = json!(false);
    let disconnected = "The Selkies stream disconnected. Choose Reconnect to resume.";
    if *cancel.borrow() {
        // Dropped while the handshake finished: leave without sending the stale SETTINGS.
        trace_input_event("close frame (cancelled before SETTINGS)");
        let _ = socket.close(None).await;
        return Ok(());
    }
    socket
        .send(format!("SETTINGS,{settings}").into())
        .await
        .map_err(|_| disconnected)?;
    let mut frame = Canvas::default();
    let mut jpeg_size = Resolution(1280, 720);
    let mut active_size = jpeg_size;
    let mut pacer = JpegPacer::default();
    let mut last_frame = None;
    let mut seen_video = false;
    let mut unsupported_video = false;
    let mut audio: Option<Playback> = None;
    // The last output picked during this stream, so a pick made before audio starts counts.
    let mut audio_pick: Option<Option<String>> = None;
    let mut ready = false;
    let mut diagnostics = Diagnostics::default();
    diagnostics.set_stats(options.stats);
    let mut sample_start = Instant::now();
    let mut ping = None;
    let mut sample_tick = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(1),
        Duration::from_secs(1),
    );
    sample_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut capabilities = Settings::default();
    let mut resize_allowed = false;
    let mut screen_policy = ScreenPolicy::default();
    let mut dark = options.dark;
    let mut video_playing = None;
    let mut appearance = Value::Null;
    let mut sent_scheme = None;
    let video_deadline = tokio::time::sleep(WAIT_FOR_VIDEO);
    tokio::pin!(video_deadline);
    let mut present = tokio::time::interval(Duration::from_millis(33));
    present.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Shows the last frame of a burst; each stripe pushes it back, and it is polled only
    // while a frame is pending, so a static desktop never wakes for it (`ARCH-034`).
    let settle = tokio::time::sleep(SETTLE);
    tokio::pin!(settle);
    let mut pointer = pointer;
    let mut held = BTreeSet::new();
    let mut clipboard_in = clipboard::Receiver::default();
    let mut clipboard_transfer = 0u64;
    let mut heartbeat = tokio::time::interval(Duration::from_secs(1));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            incoming = socket.next() => match incoming {
                Some(Ok(Message::Binary(data))) => {
                    diagnostics.bytes += data.len();
                    diagnostics.received_bytes += data.len() as u64;
                    match data.first().copied() {
                    Some(3) if data.len() > 6 && hardware.as_ref().is_none_or(|video| !video.decoded) => {
                        if frame.resize(jpeg_size.0, jpeg_size.1)? {
                            pacer.reset();
                        }
                        active_size = jpeg_size;
                        let id = u16::from_be_bytes([data[2], data[3]]);
                        let y = u16::from_be_bytes([data[4], data[5]]);
                        last_frame = Some(id);
                        let started = Instant::now();
                        if let Some(stripe) = frame.decode(y, &data[6..]) {
                            diagnostics.packets += 1;
                            diagnostics.codec = Some(Codec::Jpeg);
                            diagnostics.decode += started.elapsed();
                            let now = Instant::now();
                            if let Some(present) = pacer.stripe(id, now) {
                                present_jpeg(&frames, &frame, &mut diagnostics, present);
                            }
                            frame.insert(stripe);
                            if let Some(deadline) = pacer.deadline() {
                                settle.as_mut().reset(deadline.into());
                            }
                            seen_video = true;
                        } else {
                            diagnostics.dropped += 1;
                        }
                    }
                    Some(4) if data.len() > 12 => {
                        last_frame = Some(u16::from_be_bytes([data[2], data[3]]));
                        if let Some(video) = &mut hardware {
                            let packet = video::Packet::parse(&data);
                            if packet.as_ref().is_some_and(|packet| packet.codec != video.codec) {
                                continue;
                            }
                            let started = Instant::now();
                            if let Some(picture) = packet.as_ref().and_then(|packet| video.decode(packet)) {
                                frame = Canvas::default();
                                pacer.reset();
                                active_size = Resolution(picture.width, picture.height);
                                // A hardware frame is a whole picture: show it now rather
                                // than on the next tick, which capped video at 30 fps (`ARCH-021`).
                                diagnostics.frames += 1;
                                frames.send_replace(picture);
                                diagnostics.codec = Some(video.codec);
                                diagnostics.packets += 1;
                                diagnostics.decode += started.elapsed();
                                seen_video = true;
                            } else {
                                diagnostics.dropped += 1;
                                if video.recover(Instant::now()) {
                                    diagnostics.keyframe_requests += 1;
                                    socket.send("REQUEST_KEYFRAME".into()).await.map_err(|_| disconnected)?;
                                }
                            }
                        } else {
                            unsupported_video = true;
                        }
                    }
                    Some(1) => { if let Some(audio) = &mut audio { Playback::packet(audio, &data); } }
                    _ => {}
                    }
                    if seen_video && video_playing.is_none() {
                        video_playing = Some(true);
                        let _ = events.send(SessionEvent::Video(true));
                    }
                },
                Some(Ok(Message::Text(text))) => {
                    if let Some(cursor) = text.strip_prefix("cursor,") {
                        if let Some(cursor) = RemoteCursor::parse(cursor) {
                            let _ = events.send(SessionEvent::Cursor(cursor));
                        }
                        continue;
                    }
                    match clipboard_in.on_text(&text) {
                        clipboard::Frame::NotClipboard => {}
                        clipboard::Frame::Pending => continue,
                        clipboard::Frame::Payload(payload) => {
                            let _ = events.send(SessionEvent::Clipboard(payload));
                            continue;
                        }
                    }
                    if text.starts_with("MODE ") && text != "MODE websockets" {
                        return Err("This server requires WebRTC. SealSkin Native currently uses WebSockets.".into());
                    }
                    if let Some(finger_scroll) = finger_scroll_capability(&text) {
                        let _ = events.send(SessionEvent::FingerScroll(finger_scroll));
                        continue;
                    }
                    if let Ok(data) = serde_json::from_str::<Value>(text.as_str()) {
                        match data["type"].as_str() {
                            Some("stream_info") => {
                                if let Ok(info) = serde_json::from_value::<StreamInfo>(data["info"].clone()) {
                                    let _ = events.send(SessionEvent::StreamInfo(Box::new(info)));
                                }
                            }
                            Some("stream_stats") if data["displayId"].as_str().unwrap_or("primary") == "primary" => {
                                diagnostics.server = Some((Instant::now(), ServerFigures::parse(&data["stats"])));
                            }
                            Some("stream_resolution") if data["displayId"].as_str().unwrap_or("primary") == "primary" => {
                                let width = dimension(&data["width"])?;
                                let height = dimension(&data["height"])?;
                                if !valid_resolution(width, height) {
                                    return Err("The remote display resolution is outside the supported range.".into());
                                }
                                jpeg_size = Resolution(width, height);
                                active_size = jpeg_size;
                                // A new size presents at once (empty), so the view retires
                                // the old-size stripes.
                                if hardware.as_ref().is_none_or(|video| !video.decoded)
                                    && frame.resize(width, height)?
                                {
                                    pacer.reset();
                                    frames.send_replace(frame.snapshot());
                                }
                            }
                            Some("server_settings") | Some("display_settings") if primary_display(&data) => {
                                if data["type"] == "display_settings" {
                                    capabilities.live(&data["settings"]);
                                } else {
                                    capabilities.announce(&data["settings"]);
                                    let _ = events.send(SessionEvent::ClipboardPolicy(clipboard::Policy::from_server(&data["settings"])));
                                    let _ = events.send(SessionEvent::FilePolicy(selkies::Policy::from_server(&data["settings"])));
                                    resize_allowed = allows_resize(&data["settings"]);
                                    screen_policy = ScreenPolicy::from_server(&data["settings"]);
                                    let _ = events.send(SessionEvent::ScreenPolicy(screen_policy.clone()));
                                    appearance = data["settings"]["color_scheme"].clone();
                                    let _ = events.send(SessionEvent::ResizeAllowed(resize_allowed));
                                    if !capabilities.codecs.contains(&codec) {
                                        codec = capabilities.codecs.first().copied().ok_or("This server offers no video codec this Mac can play.")?;
                                        hardware = (codec != Codec::Jpeg).then(|| HardwareVideo::new(codec, Instant::now()));
                                        pacer.reset();
                                        seen_video = false;
                                        let command = codec_command(codec);
                                        socket.send(command.into()).await.map_err(|_| disconnected)?;
                                        socket.send("REQUEST_KEYFRAME".into()).await.map_err(|_| disconnected)?;
                                        video_deadline.as_mut().reset(tokio::time::Instant::now() + WAIT_FOR_VIDEO);
                                    }
                                }
                                let _ = events.send(SessionEvent::Settings(capabilities.clone()));
                                if data["type"] == "server_settings" {
                                    if let Some(scheme) = requested_scheme(&appearance, dark).filter(|scheme| Some(*scheme) != sent_scheme) {
                                        socket.send(color_command(scheme).into()).await.map_err(|_| disconnected)?;
                                        sent_scheme = Some(scheme);
                                    }
                                }
                                if data["type"] == "server_settings" && !ready {
                                    ready = true;
                                    if diagnostics.stats_on {
                                        socket.send("_stats,1".into()).await.map_err(|_| disconnected)?;
                                    }
                                    if let Some(legacy) = legacy_settings(&capabilities) {
                                        socket.send(legacy.into()).await.map_err(|_| disconnected)?;
                                    }
                                    if let Some((command, applied)) = settings_command(&capabilities, options.preferences.iter().copied()) {
                                        socket.send(command.into()).await.map_err(|_| disconnected)?;
                                        capabilities.live(&applied);
                                        let _ = events.send(SessionEvent::Settings(capabilities.clone()));
                                    }
                                    if let Some((command, applied)) = options.audio_bitrate.and_then(|bps| audio_bitrate_command(&capabilities, bps)) {
                                        socket.send(command.into()).await.map_err(|_| disconnected)?;
                                        capabilities.live(&applied);
                                        let _ = events.send(SessionEvent::Settings(capabilities.clone()));
                                    }
                                    if data["settings"]["audio_enabled"]["value"].as_bool() == Some(false) {
                                        let _ = events.send(SessionEvent::AudioUnavailable("Audio turned off by the server"));
                                    } else {
                                        let statuses = events.clone();
                                        audio = Playback::new(starting_output(options.audio_output.as_ref(), audio_pick.as_ref()), move |status| {
                                            let _ = statuses.send(SessionEvent::AudioOutput(status));
                                        });
                                        match audio_start(&data["settings"], audio.is_some(), options.muted) {
                                            AudioStart::Play => socket.send("START_AUDIO".into()).await.map_err(|_| disconnected)?,
                                            AudioStart::Muted => { let _ = events.send(SessionEvent::Audio(false)); }
                                            AudioStart::Unavailable => { let _ = events.send(SessionEvent::AudioUnavailable("Audio couldn’t start on this Mac.")); }
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    } else if text == "VIDEO_STARTED" || text == "VIDEO_STOPPED" {
                        let playing = text == "VIDEO_STARTED";
                        video_playing = Some(playing);
                        let _ = events.send(SessionEvent::Video(playing));
                    } else if text == "AUDIO_STARTED" && audio.is_some() {
                        let _ = events.send(SessionEvent::Audio(true));
                    } else if text == "AUDIO_STOPPED" && audio.is_some() {
                        let _ = events.send(SessionEvent::Audio(false));
                    }
                }
                Some(Ok(Message::Pong(data))) if data.as_ref() == b"sealskin-rtt" => {
                    if let Some(sent) = ping.take() {
                        diagnostics.rtt_ms = Some(Instant::now().duration_since(sent).as_secs_f64() * 1000.0);
                    }
                }
                Some(Ok(Message::Ping(_))) => { socket.flush().await.map_err(|_| disconnected)?; }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return Err(disconnected.into()),
                _ => {}
            },
            command = input.recv() => {
                let text = match command {
                    Some(Input::Command(text)) => text,
                    Some(Input::Pointer(position)) => {
                        let Some(text) = position.command_for(active_size) else { continue; };
                        text
                    }
                    Some(Input::Zoom(notches, position)) => {
                        let control_held = held.contains(&0xffe3) || held.contains(&0xffe4);
                        for command in zoom_commands(notches, position, active_size, control_held) {
                            trace_input(&command);
                            socket.send(command.into()).await.map_err(|_| disconnected)?;
                        }
                        continue;
                    }
                    Some(Input::Video(enabled)) => {
                        if let Some(video) = &mut hardware {
                            video.decoder = Decoder::default();
                            video.deadline = enabled.then(|| Instant::now() + WAIT_FOR_VIDEO);
                        }
                        if enabled { "START_VIDEO" } else { "STOP_VIDEO" }.to_string()
                    }
                    Some(Input::Audio(enabled)) => {
                        if let Some(audio) = &mut audio { audio.clear(); }
                        if enabled { "START_AUDIO" } else { "STOP_AUDIO" }.to_string()
                    }
                    Some(Input::AudioOutput(chosen)) => {
                        if let Some(audio) = &audio { audio.set_output(chosen.clone()); }
                        audio_pick = Some(chosen);
                        continue;
                    }
                    Some(Input::AudioBitrate(bps)) => {
                        let Some((command, applied)) = audio_bitrate_command(&capabilities, bps) else { continue; };
                        socket.send(command.into()).await.map_err(|_| disconnected)?;
                        // Selkies may not echo it to the controller that asked.
                        capabilities.live(&applied);
                        let _ = events.send(SessionEvent::Settings(capabilities.clone()));
                        continue;
                    }
                    Some(Input::Settings(changes)) => {
                        let Some((command, applied)) = settings_command(&capabilities, changes) else { continue; };
                        socket.send(command.into()).await.map_err(|_| disconnected)?;
                        // Some Selkies versions only echo changes to other controllers.
                        capabilities.live(&applied);
                        let _ = events.send(SessionEvent::Settings(capabilities.clone()));
                        continue;
                    }
                    Some(Input::Screen(change)) => {
                        if let Some(settings) = screen_policy.apply(change) {
                            socket.send(format!("SETTINGS,{settings}").into()).await.map_err(|_| disconnected)?;
                            let _ = events.send(SessionEvent::ScreenPolicy(screen_policy.clone()));
                        }
                        continue;
                    }
                    Some(Input::Resolution(request)) => {
                        if resize_allowed {
                            let Resolution(width, height) = request.resolution;
                            let settings = request.settings();
                            socket.send(format!("SETTINGS,{settings}").into()).await.map_err(|_| disconnected)?;
                            socket.send(format!("r,{width}x{height},primary").into()).await.map_err(|_| disconnected)?;
                        }
                        continue;
                    }
                    Some(Input::Appearance(next)) => {
                        dark = next;
                        if let Some(scheme) = requested_scheme(&appearance, dark).filter(|scheme| Some(*scheme) != sent_scheme) {
                            socket.send(color_command(scheme).into()).await.map_err(|_| disconnected)?;
                            sent_scheme = Some(scheme);
                        }
                        continue;
                    }
                    Some(Input::Clipboard(content)) => {
                        clipboard_transfer += 1;
                        for frame in clipboard::encode(&content, clipboard_transfer) {
                            socket.send(frame.into()).await.map_err(|_| disconnected)?;
                        }
                        continue;
                    }
                    Some(Input::Stats(on)) => {
                        if on == diagnostics.stats_on { continue; }
                        diagnostics.set_stats(on);
                        if !ready { continue; }
                        format!("_stats,{}", u8::from(on))
                    }
                    // Every sender gone (rare: `Desktop` and the pinch monitor hold clones) or
                    // an explicit close: release held keys, then close cleanly.
                    Some(Input::Close) | None => {
                        for command in closing_commands(&held) {
                            trace_input(&command);
                            let _ = socket.send(command.into()).await;
                        }
                        trace_input_event("close frame");
                        let _ = socket.close(None).await;
                        return Ok(());
                    }
                };
                if let Some(key) = text.strip_prefix("kd,").and_then(|key| key.parse::<u32>().ok()) {
                    held.insert(key);
                } else if let Some(key) = text.strip_prefix("ku,").and_then(|key| key.parse::<u32>().ok()) {
                    held.remove(&key);
                }
                trace_input(&text);
                socket.send(text.into()).await.map_err(|_| disconnected)?;
            }
            _ = heartbeat.tick(), if !held.is_empty() => {
                let keys = held.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
                let text = format!("kh,{keys}");
                trace_input(&text);
                socket.send(text.into()).await.map_err(|_| disconnected)?;
            }
            _ = sample_tick.tick() => {
                let now = Instant::now();
                let sample = diagnostics.sample(now.duration_since(sample_start), active_size.0, active_size.1);
                let _ = events.send(SessionEvent::Stats(sample));
                sample_start = now;
                if ping.is_none() {
                    socket.send(Message::Ping(b"sealskin-rtt".as_slice().into())).await.map_err(|_| disconnected)?;
                    ping = Some(now);
                }
            }
            _ = &mut settle, if pacer.deadline().is_some() => {
                if let Some(present) = pacer.present(PresentReason::Settle, Instant::now()) {
                    present_jpeg(&frames, &frame, &mut diagnostics, present);
                }
            }
            _ = present.tick() => {
                if video_playing != Some(false) && hardware.as_ref().is_some_and(|video| video.expired(Instant::now())) {
                    if !capabilities.codecs.contains(&Codec::Jpeg) {
                        return Err("Hardware video decoding failed and the server does not allow JPEG fallback.".into());
                    }
                    codec = Codec::Jpeg;
                    hardware = None;
                    pacer.reset();
                    seen_video = false;
                    unsupported_video = false;
                    video_deadline.as_mut().reset(tokio::time::Instant::now() + WAIT_FOR_VIDEO);
                    socket.send(codec_command(codec).into()).await.map_err(|_| disconnected)?;
                    socket.send("REQUEST_KEYFRAME".into()).await.map_err(|_| disconnected)?;
                }
                if let Some(id) = last_frame.take() {
                    socket.send(format!("CLIENT_FRAME_ACK {id} 0").into()).await.map_err(|_| disconnected)?;
                }
                if pointer.has_changed().unwrap_or(false) {
                    let position = *pointer.borrow_and_update();
                    if let Some(command) = position.and_then(|p| p.command_for(active_size)) {
                        socket.send(command.into()).await.map_err(|_| disconnected)?;
                    }
                }
            }
            _ = &mut video_deadline, if hardware.is_none() && !seen_video && video_playing != Some(false) => {
                return Err(if unsupported_video {
                    "The server did not switch to JPEG. Enable JPEG for this session and reconnect."
                } else {
                    "No video arrived from Selkies. Choose Reconnect to try again."
                }
                .into());
            }
        }
    }
}

/// The output a new `Playback` starts with: the last pick this stream saw (System Default
/// included), else the saved device.
fn starting_output(saved: Option<&String>, picked: Option<&Option<String>>) -> Option<String> {
    picked.map_or_else(|| saved.cloned(), Clone::clone)
}

/// The web client's audio bitrate message, `SETTINGS,{"audio_bitrate":<bps>,"picked":[…]}`
/// (`picked` makes a non-owner controller's change count), and the value to apply locally.
/// `None` unless the server offers `bps` and it differs from the current value.
fn audio_bitrate_command(settings: &Settings, bps: u32) -> Option<(String, Value)> {
    let bps = settings.audio_bitrate.as_ref()?.request(bps)?;
    Some((
        format!(
            "SETTINGS,{}",
            json!({"audio_bitrate": bps, "picked": ["audio_bitrate"]})
        ),
        json!({"audio_bitrate": bps}),
    ))
}

#[derive(Debug, PartialEq, Eq)]
enum AudioStart {
    Play,
    Muted,
    Unavailable,
}

/// `decoder`: whether `Playback` started (it fails only when the Opus decoder can't).
fn audio_start(settings: &Value, decoder: bool, muted: bool) -> AudioStart {
    if !decoder {
        return AudioStart::Unavailable;
    }
    if muted || settings["audio_on_start"]["value"].as_bool() == Some(false) {
        AudioStart::Muted
    } else {
        AudioStart::Play
    }
}

fn requested_scheme(policy: &Value, dark: bool) -> Option<&'static str> {
    let scheme = if dark { "dark" } else { "light" };
    (policy["locked"].as_bool() != Some(true)
        && policy["allowed"]
            .as_array()?
            .iter()
            .any(|option| option == scheme))
    .then_some(scheme)
}

fn color_command(scheme: &str) -> String {
    format!("SETTINGS,{}", json!({"color_scheme": scheme}))
}

fn allows_resize(settings: &Value) -> bool {
    settings["enable_resize"]["value"].as_bool().unwrap_or(true)
        && settings["manual_resolution"]["locked"].as_bool() != Some(true)
}

fn primary_display(data: &Value) -> bool {
    data["displayId"].as_str().unwrap_or("primary") == "primary"
}

fn legacy_settings(settings: &Settings) -> Option<String> {
    let missing: serde_json::Map<String, Value> = [(Key::Fps, 30), (Key::Jpeg, 75)]
        .into_iter()
        .filter(|(key, _)| settings.get(*key).is_none())
        .map(|(key, value)| (key.wire().into(), key.encode(value)))
        .collect();
    (!missing.is_empty()).then(|| format!("SETTINGS,{}", Value::Object(missing)))
}

fn settings_command(
    settings: &Settings,
    changes: impl IntoIterator<Item = (Key, u32)>,
) -> Option<(String, Value)> {
    let values: serde_json::Map<String, Value> = changes
        .into_iter()
        .filter_map(|(key, value)| {
            settings
                .request(key, value)
                .map(|value| (key.wire().into(), key.encode(value)))
        })
        .collect();
    if values.is_empty() {
        return None;
    }
    let mut command = values.clone();
    command.insert("picked".into(), json!(values.keys().collect::<Vec<_>>()));
    Some((
        format!("SETTINGS,{}", Value::Object(command)),
        Value::Object(values),
    ))
}

fn codec_command(codec: Codec) -> String {
    format!(
        "SETTINGS,{}",
        json!({"encoder": codec.wire(), "encoderFallback": true, "video_10bit": false, "video_fullcolor": false})
    )
}

fn valid_resolution(width: u16, height: u16) -> bool {
    width > 0 && width <= MAX_WIDTH && height > 0 && height <= MAX_HEIGHT
}

fn dimension(value: &Value) -> Result<u16, &'static str> {
    value
        .as_u64()
        .and_then(|v| u16::try_from(v).ok())
        .or_else(|| value.as_str().and_then(|v| v.parse().ok()))
        .ok_or("Selkies returned an invalid display resolution.")
}

/// How long to keep retrying the WebSocket while a newly launched session starts.
pub const SOCKET_STARTUP_GRACE: Duration = Duration::from_secs(45);
/// How long Keep Waiting keeps retrying a session that hasn't answered yet.
pub const KEEP_WAITING_GRACE: Duration = Duration::from_secs(300);

fn transient_socket_error(error: &tokio_tungstenite::tungstenite::Error) -> bool {
    use tokio_tungstenite::tungstenite::Error;
    match error {
        Error::Io(_) => true,
        Error::Http(response) => transient_status(response.status().as_u16()),
        _ => false,
    }
}

/// Statuses a session proxy returns while the container behind it is still starting.
fn transient_status(status: u16) -> bool {
    matches!(status, 404 | 502 | 503 | 504)
}

/// A session's cookie, from SealSkin's `GET /<id>/?access_token=…` → 303 exchange
/// (`sessions.py::initial_session_auth`). The token is compared and copied into the cookie,
/// not used up, so the exchange can repeat while a stream is open (`ARCH-028`).
pub struct SessionAccess {
    /// Where the 303 sent us: the session's own path, still with its query.
    pub redirect: Url,
    /// `redirect` without query or fragment: the base of the session's routes.
    pub base: Url,
    /// Every cookie the exchange set, marked sensitive.
    pub cookie: reqwest::header::HeaderValue,
}

const ADDRESS_ERROR: &str =
    "SealSkin returned an unsupported session address. Open an individual session.";
const AUTH_ERROR: &str =
    "Couldn’t authorize the native session. Refresh the launcher and resume again.";

/// The cookie exchange for an individual session URL, on its own client with no
/// credentials: the application key never goes to a session origin. A collaboration room
/// (`/room/<id>?token=`) has no `access_token` and is refused.
pub async fn session_access(url: Url) -> Result<SessionAccess, &'static str> {
    if !url.query_pairs().any(|(key, _)| key == "access_token") || !url.path().ends_with('/') {
        return Err(ADDRESS_ERROR);
    }
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|_| AUTH_ERROR)?;
    let response = http.get(url.clone()).send().await.map_err(|_| AUTH_ERROR)?;
    if response.status() != reqwest::StatusCode::SEE_OTHER {
        return Err(AUTH_ERROR);
    }
    let location = response
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .ok_or(ADDRESS_ERROR)?;
    let redirect = url.join(location).map_err(|_| ADDRESS_ERROR)?;
    if redirect.origin() != url.origin() || redirect.path() != url.path() {
        return Err(ADDRESS_ERROR);
    }
    let cookie = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok()?.split(';').next())
        .collect::<Vec<_>>()
        .join("; ");
    if cookie.is_empty() {
        return Err(AUTH_ERROR);
    }
    let mut cookie = cookie
        .parse::<reqwest::header::HeaderValue>()
        .map_err(|_| AUTH_ERROR)?;
    cookie.set_sensitive(true);
    let mut base = redirect.clone();
    base.set_query(None);
    base.set_fragment(None);
    Ok(SessionAccess {
        redirect,
        base,
        cookie,
    })
}

/// The session's Selkies file routes without a stream: the cookie exchange, then a
/// `SelkiesClient` under that cookie (the Library's Files button, `ARCH-028`).
pub async fn selkies_for_session(url: Url) -> Result<selkies::SelkiesClient, &'static str> {
    let access = session_access(url).await?;
    selkies::SelkiesClient::new(access.base, access.cookie).map_err(|_| ADDRESS_ERROR)
}

/// The WebSocket handshake for the session, and the session's file routes under the same
/// cookie (`ARCH-023`), or `None` when they cannot be built.
async fn socket_request(
    url: Url,
) -> Result<
    (
        tokio_tungstenite::tungstenite::handshake::client::Request,
        Option<selkies::SelkiesClient>,
    ),
    &'static str,
> {
    let SessionAccess {
        redirect,
        base,
        cookie,
    } = session_access(url.clone()).await?;
    let mut websocket = redirect.join("api/websockets").map_err(|_| ADDRESS_ERROR)?;
    websocket.set_scheme("wss").map_err(|_| ADDRESS_ERROR)?;
    let mut request = websocket
        .as_str()
        .into_client_request()
        .map_err(|_| ADDRESS_ERROR)?;
    let access = selkies::SelkiesClient::new(base, cookie.clone()).ok();
    request.headers_mut().insert("cookie", cookie);
    request.headers_mut().insert(
        "origin",
        url.origin()
            .ascii_serialization()
            .parse()
            .map_err(|_| ADDRESS_ERROR)?,
    );
    Ok((request, access))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_options() -> SessionOptions {
        SessionOptions {
            preferences: Vec::new(),
            dark: false,
            muted: true,
            codec: Codec::Jpeg,
            display: None,
            stats: false,
            startup_grace: SOCKET_STARTUP_GRACE,
            waiting: String::new(),
            audio_output: None,
            audio_bitrate: None,
        }
    }

    #[test]
    fn an_attempt_timeout_is_retried_then_becomes_a_startup_timeout() {
        use tokio_tungstenite::tungstenite::Error;
        // The attempt's own 10 s timeout maps to transient, as a refused connection does.
        assert!(attempt_transient(Err(None)));
        let refused = Error::Io(std::io::Error::from(std::io::ErrorKind::ConnectionRefused));
        assert!(attempt_transient(Err(Some(&refused))));
        assert!(!attempt_transient(Err(Some(&Error::Utf8(String::new())))));
        let start = Instant::now();
        let give_up = start + SOCKET_STARTUP_GRACE;
        assert_eq!(
            retry_decision(attempt_transient(Err(None)), start, give_up),
            Retry::Again
        );
        assert_eq!(
            retry_decision(attempt_transient(Err(None)), give_up, give_up),
            Retry::Timeout
        );
    }

    #[test]
    fn startup_grace_expiry_is_a_startup_timeout_not_a_generic_error() {
        let start = Instant::now();
        let give_up = start + SOCKET_STARTUP_GRACE;
        assert_eq!(retry_decision(true, start, give_up), Retry::Again);
        assert_eq!(retry_decision(true, give_up, give_up), Retry::Timeout);
        assert_eq!(retry_decision(false, start, give_up), Retry::Fail);
        assert_eq!(retry_decision(false, give_up, give_up), Retry::Fail);
        // An attempt that timed out counts as transient (the caller passes `true`), so a
        // session that accepts TCP but hangs is retried, then times out the same way.
        assert_eq!(
            retry_decision(true, start + Duration::from_secs(44), give_up),
            Retry::Again
        );
        assert_eq!(
            RunError::from("x").to_string(),
            "x",
            "other errors keep their message"
        );
    }

    fn selkies_audio_settings(value: &str) -> Settings {
        let mut settings = Settings::default();
        settings.announce(&json!({"audio_bitrate": {
            "value": value,
            "allowed": ["32000", "64000", "96000", "128000", "320000"]
        }}));
        settings
    }

    #[test]
    fn audio_bitrate_command_matches_web_settings_message() {
        let settings = selkies_audio_settings("128000");
        let (command, applied) = audio_bitrate_command(&settings, 96_000).unwrap();
        assert_eq!(
            command,
            r#"SETTINGS,{"audio_bitrate":96000,"picked":["audio_bitrate"]}"#
        );
        assert_eq!(applied, json!({"audio_bitrate": 96000}));
    }

    #[test]
    fn saved_audio_bitrate_is_sent_on_ready_only_when_allowed_and_different() {
        let settings = selkies_audio_settings("128000");
        assert!(audio_bitrate_command(&settings, 320_000).is_some());
        assert!(audio_bitrate_command(&settings, 128_000).is_none());
        assert!(audio_bitrate_command(&settings, 100_000).is_none());
        assert!(audio_bitrate_command(&Settings::default(), 96_000).is_none());
    }

    #[test]
    fn saved_bitrate_skipped_when_locked() {
        let mut settings = Settings::default();
        settings.announce(&json!({"audio_bitrate": {
            "value": "128000", "locked": true, "allowed": ["64000", "128000"]
        }}));
        assert!(audio_bitrate_command(&settings, 64_000).is_none());
    }

    #[test]
    fn a_pick_before_audio_starts_wins_over_the_saved_device() {
        let saved = Some("AirPods".to_string());
        assert_eq!(starting_output(saved.as_ref(), None), saved);
        assert_eq!(starting_output(saved.as_ref(), Some(&None)), None);
        let usb = Some("USB".to_string());
        assert_eq!(starting_output(saved.as_ref(), Some(&usb)), usb);
        assert_eq!(starting_output(None, None), None);
    }

    /// Runs a stream against `listener` with `cancel`, and returns how it ended and every
    /// connection the listener saw (each one's first bytes) within `wait`.
    async fn run_against(
        listener: tokio::net::TcpListener,
        cancel: watch::Receiver<bool>,
        wait: Duration,
    ) -> (Result<(), RunError>, Vec<String>) {
        use tokio::io::AsyncReadExt as _;
        let address = listener.local_addr().unwrap();
        let url = Url::parse(&format!("http://{address}/s/?access_token=t")).unwrap();
        let seen = tokio::spawn(async move {
            let mut seen = Vec::new();
            // Accept everything, answer nothing: the HTTP step hangs as on a slow server.
            let mut held = Vec::new();
            while let Ok(Ok((mut stream, _))) =
                tokio::time::timeout(Duration::from_millis(300), listener.accept()).await
            {
                let mut buf = [0u8; 256];
                let read = tokio::time::timeout(Duration::from_millis(100), stream.read(&mut buf))
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .unwrap_or(0);
                seen.push(String::from_utf8_lossy(&buf[..read]).to_string());
                held.push(stream);
            }
            seen
        });
        let (frames, _) = watch::channel(Frame::default());
        let (_input_tx, input) = mpsc::unbounded_channel();
        let (_pointer_tx, pointer) = watch::channel(None);
        let (events, _events_rx) = mpsc::unbounded_channel();
        let ended = tokio::time::timeout(
            wait,
            run(url, frames, input, pointer, test_options(), &events, cancel),
        )
        .await
        .expect("a cancelled stream ends at once");
        (ended, seen.await.unwrap())
    }

    #[tokio::test]
    async fn a_stream_cancelled_before_it_starts_never_connects() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (cancel_tx, cancel) = watch::channel(true);
        let (ended, seen) = run_against(listener, cancel, Duration::from_millis(200)).await;
        drop(cancel_tx);
        assert_eq!(ended, Ok(()));
        assert!(seen.is_empty(), "{seen:?}");
    }

    #[tokio::test]
    async fn a_stream_dropped_while_connecting_stops_without_settings() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (cancel_tx, cancel) = watch::channel(false);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            // What `NativeSession::drop` does: set the flag, then drop the sender.
            let _ = cancel_tx.send(true);
        });
        let (ended, seen) = run_against(listener, cancel, Duration::from_secs(2)).await;
        assert_eq!(ended, Ok(()));
        // Only the hung HTTP request ever reached the server: no WebSocket, no SETTINGS.
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert!(seen[0].starts_with("GET /s/?access_token=t"), "{seen:?}");
        assert!(!seen.iter().any(|request| request.contains("SETTINGS")));
    }

    #[test]
    fn closing_releases_every_held_key_then_closes() {
        let held = BTreeSet::from([0x61, 0xffe3]);
        assert_eq!(closing_commands(&held), ["ku,97", "ku,65507"]);
    }

    #[test]
    fn closing_with_nothing_held_sends_nothing() {
        assert!(closing_commands(&BTreeSet::new()).is_empty());
    }

    #[test]
    fn the_input_trace_hides_typed_characters() {
        assert_eq!(redact_key_command("kd,97").as_deref(), Some("kd,<char>"));
        assert_eq!(
            redact_key_command("ku,16777729").as_deref(),
            Some("ku,<char>")
        );
        assert_eq!(redact_key_command("kd,65507").as_deref(), Some("kd,65507"));
        assert_eq!(redact_key_command("ku,65307").as_deref(), Some("ku,65307"));
        assert_eq!(
            redact_key_command("kh,97,65505,32").as_deref(),
            Some("kh,<char>,65505,<char>")
        );
        assert_eq!(redact_key_command("kd,65457").as_deref(), Some("kd,<char>"));
        assert_eq!(redact_key_command("kr").as_deref(), Some("kr"));
        assert_eq!(redact_key_command("m2,0,0,1,0"), None);
        assert_eq!(redact_key_command("SETTINGS,{}"), None);
    }

    #[test]
    fn reads_selkies_prefixed_display_capabilities() {
        assert_eq!(
            finger_scroll_capability(
                r#"DISPLAY_CONFIG_UPDATE,{"type":"display_config_update","finger_scroll":true}"#
            ),
            Some(true)
        );
        assert_eq!(
            finger_scroll_capability(r#"{"type":"display_config_update"}"#),
            Some(false)
        );
        assert_eq!(finger_scroll_capability(r#"{"type":"stream_info"}"#), None);
    }

    fn jpeg(color: [u8; 3]) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut bytes)
            .encode(
                &[color, color].concat(),
                2,
                1,
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
        bytes
    }

    #[test]
    fn discards_pointer_events_from_before_remote_resize() {
        let pointer = Pointer {
            display: Resolution(1280, 720),
            position: (960, 360),
            buttons: 1,
        };
        assert_eq!(
            pointer.command_for(Resolution(1280, 720)).as_deref(),
            Some("m,960,360,1,0")
        );
        assert_eq!(pointer.command_for(Resolution(2560, 1440)), None);
    }

    #[test]
    fn playback_and_display_follow_server_policy() {
        assert_eq!(
            audio_start(&json!({"audio_on_start": {"value": false}}), true, false),
            AudioStart::Muted
        );
        assert_eq!(audio_start(&json!({}), true, true), AudioStart::Muted);
        assert_eq!(
            audio_start(&json!({}), false, false),
            AudioStart::Unavailable
        );
        assert_eq!(audio_start(&json!({}), true, false), AudioStart::Play);
        assert_eq!(
            requested_scheme(&json!({"allowed": ["light", "dark"]}), true),
            Some("dark")
        );
        assert_eq!(
            requested_scheme(&json!({"allowed": ["light", "dark"], "locked": true}), true),
            None
        );
        assert!(!allows_resize(&json!({"enable_resize": {"value": false}})));
        assert!(!allows_resize(
            &json!({"manual_resolution": {"locked": true}})
        ));
    }

    #[test]
    fn pinch_preserves_buttons_and_preexisting_control() {
        let display = Resolution(1280, 720);
        let position = Pointer {
            display,
            position: (32, 48),
            buttons: 1,
        };
        assert_eq!(
            zoom_commands(2, position, display, false),
            [
                "m,32,48,1,0",
                "kd,65507",
                "m2,0,0,17,2",
                "m2,0,0,1,0",
                "ku,65507"
            ]
        );
        assert_eq!(
            zoom_commands(-1, position, display, true),
            ["m,32,48,1,0", "m2,0,0,9,1", "m2,0,0,1,0"]
        );
        assert!(zoom_commands(2, position, Resolution(1920, 1080), false).is_empty());
    }

    #[test]
    fn cursor_decode_preserves_the_hotspot_and_handles_hidden_cursors() {
        assert!(matches!(
            RemoteCursor::parse(r#"{"curdata":"","handle":0}"#),
            Some(RemoteCursor::Hidden)
        ));
        let mut png = Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(2, 3, image::Rgba([255, 100, 10, 255]))
            .write_to(&mut png, ImageFormat::Png)
            .unwrap();
        let payload = json!({"curdata": STANDARD.encode(png.into_inner()), "handle": "1", "hotx": "1", "hoty": 2});
        assert!(matches!(
            RemoteCursor::parse(&payload.to_string()),
            Some(RemoteCursor::Image {
                hotspot: (1, 2),
                size: (2, 3),
                ..
            })
        ));
        assert!(RemoteCursor::parse(r#"{"curdata":"garbage","handle":1}"#).is_none());
    }

    #[test]
    fn hardware_recovery_throttles_requests_without_postponing_fallback() {
        let start = Instant::now();
        let mut video = HardwareVideo::new(Codec::H264, start);
        assert!(video.recover(start));
        assert!(!video.recover(start + Duration::from_millis(500)));
        assert!(video.recover(start + Duration::from_secs(1)));
        assert!(!video.expired(start + Duration::from_secs(14)));
        assert!(video.expired(start + WAIT_FOR_VIDEO));
    }

    #[test]
    fn only_startup_statuses_are_retried() {
        for status in [404, 502, 503, 504] {
            assert!(transient_status(status), "{status}");
        }
        for status in [200, 400, 401, 403, 500] {
            assert!(!transient_status(status), "{status}");
        }
    }

    #[test]
    fn manual_resolution_rejects_dimensions_outside_client_limits() {
        assert_eq!(
            Resolution::manual("1367", "769"),
            Some(Resolution(1366, 768))
        );
        assert_eq!(Resolution::manual("", "768"), None);
        assert_eq!(Resolution::manual("4097", "2160"), None);
        assert_eq!(Resolution::manual("1920", "2161"), None);
    }

    #[test]
    fn resolution_fits_within_native_limits_and_uses_even_sizes() {
        assert_eq!(Resolution::from_window(1281, 721), Resolution(1280, 720));
        assert_eq!(Resolution::from_window(5120, 2880), Resolution(3840, 2160));
        assert_eq!(Resolution::from_window(1, 1), Resolution(2, 2));
    }

    #[test]
    fn reports_wire_bytes_and_frames_within_the_sample_period() {
        let mut stats = Diagnostics {
            bytes: 500_000,
            frames: 15,
            decode: Duration::from_millis(30),
            packets: 30,
            dropped: 2,
            ..Default::default()
        };
        let sample = stats.sample(Duration::from_millis(500), 1280, 720);
        assert_eq!(sample.wire_mbps, 8.0);
        assert_eq!(sample.fps, 30.0);
        assert_eq!(sample.decode_ms, 1.0);
        assert_eq!(sample.dropped, 2);
        assert_eq!(stats.sample(Duration::from_secs(1), 1280, 720).fps, 0.0);
    }

    #[test]
    fn composites_stripes_and_preserves_unchanged_rows() {
        let mut canvas = Canvas::default();
        canvas.resize(2, 2).unwrap();
        let red = jpeg([255, 0, 0]);
        let blue = jpeg([0, 0, 255]);
        canvas.stripe(0, &red).unwrap();
        canvas.stripe(1, &blue).unwrap();
        let before = stripes(&canvas.snapshot());
        canvas.stripe(1, &red).unwrap();
        let after = stripes(&canvas.snapshot());
        assert_eq!(after.len(), 2);
        assert!(Arc::ptr_eq(&before[0].image, &after[0].image));
        assert!(!Arc::ptr_eq(&before[1].image, &after[1].image));
        assert!(pixel(&before[0], 1)[2] > 240); // BGRA red
        assert!(pixel(&before[1], 1)[0] > 240); // BGRA blue
        assert!(canvas.stripe(2, &red).is_none());
    }

    impl Canvas {
        fn stripe(&mut self, y: u16, jpeg: &[u8]) -> Option<()> {
            let stripe = self.decode(y, jpeg)?;
            self.insert(stripe);
            Some(())
        }
    }

    fn stripes(frame: &Frame) -> Vec<JpegStripe> {
        match &frame.picture {
            Some(Picture::Jpeg(picture)) => picture.stripes.to_vec(),
            _ => Vec::new(),
        }
    }

    /// The first pixel of padded row `row` of a stripe, BGRA.
    fn pixel(stripe: &JpegStripe, row: usize) -> [u8; 4] {
        let bytes = stripe.image.as_bytes(0).unwrap();
        let start = row * usize::from(stripe.image.size(0).width.0 as u16) * 4;
        bytes[start..start + 4].try_into().unwrap()
    }

    fn jpeg_sized(
        width: u16,
        height: u16,
        quality: u8,
        pixel: impl Fn(u32, u32) -> [u8; 3],
    ) -> Vec<u8> {
        let mut rgb = Vec::with_capacity(usize::from(width) * usize::from(height) * 3);
        for y in 0..u32::from(height) {
            for x in 0..u32::from(width) {
                rgb.extend_from_slice(&pixel(x, y));
            }
        }
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality)
            .encode(
                &rgb,
                width.into(),
                height.into(),
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
        bytes
    }

    #[test]
    fn a_stripe_replaces_every_overlapping_stripe() {
        let mut canvas = Canvas::default();
        canvas.resize(2, 16).unwrap();
        let red = |height| jpeg_sized(2, height, 90, |_, _| [255, 0, 0]);
        for y in [0, 4, 8, 12] {
            canvas.stripe(y, &red(4)).unwrap();
        }
        // Eight stripes of two rows: 2..4 overlaps the end of the stripe at 0.
        canvas.stripe(2, &red(2)).unwrap();
        let ys: Vec<u16> = stripes(&canvas.snapshot()).iter().map(|s| s.y).collect();
        assert_eq!(ys, [2, 4, 8, 12]);
        for y in (0..16).step_by(2) {
            canvas.stripe(y, &red(2)).unwrap();
        }
        let geometry: Vec<(u16, u16)> = stripes(&canvas.snapshot())
            .iter()
            .map(|s| (s.y, s.height))
            .collect();
        assert_eq!(
            geometry,
            (0..16).step_by(2).map(|y| (y, 2)).collect::<Vec<_>>()
        );
        // One tall stripe covers several.
        canvas.stripe(3, &red(6)).unwrap();
        let ys: Vec<u16> = stripes(&canvas.snapshot()).iter().map(|s| s.y).collect();
        assert_eq!(ys, [0, 3, 10, 12, 14]);
    }

    #[test]
    fn resize_clears_stripes() {
        let mut canvas = Canvas::default();
        assert!(canvas.resize(2, 2).unwrap());
        canvas.stripe(0, &jpeg([255, 0, 0])).unwrap();
        assert!(!canvas.resize(2, 2).unwrap());
        assert!(canvas.snapshot().picture.is_some());
        assert!(canvas.resize(3, 2).unwrap());
        let frame = canvas.snapshot();
        assert!(frame.picture.is_none());
        assert_eq!((frame.width, frame.height), (3, 2));
    }

    #[test]
    fn rejects_wrong_width_overflow_zero_height_and_headers_cut_short() {
        let mut canvas = Canvas::default();
        canvas.resize(2, 4).unwrap();
        let two_rows = jpeg_sized(2, 2, 90, |_, _| [0, 255, 0]);
        assert!(canvas
            .decode(0, &jpeg_sized(4, 2, 90, |_, _| [0; 3]))
            .is_none());
        assert!(canvas.decode(3, &two_rows).is_none()); // runs past the bottom
        assert!(canvas.decode(4, &two_rows).is_none());
        assert!(canvas.decode(u16::MAX, &two_rows).is_none());
        assert!(canvas.decode(0, &[]).is_none());
        // Cut inside the headers (before the start of scan) fails; scan data cut short
        // decodes with filler, since the decoder is not strict.
        let scan = two_rows.windows(2).position(|w| w == [0xff, 0xda]).unwrap();
        assert!(canvas.decode(0, &two_rows[..scan]).is_none());
        assert!(canvas.decode(0, &two_rows[..scan / 2]).is_none());
        assert!(canvas.decode(2, &two_rows).is_some());
    }

    #[test]
    fn decoded_stripe_is_bgra_with_repeated_edge_rows() {
        let mut canvas = Canvas::default();
        canvas.resize(8, 16).unwrap();
        let bands = jpeg_sized(
            8,
            8,
            95,
            |_, y| if y < 4 { [255, 0, 0] } else { [0, 0, 255] },
        );
        let stripe = canvas.decode(8, &bands).unwrap();
        assert_eq!((stripe.y, stripe.height), (8, 8));
        assert_eq!(stripe.image.size(0).height.0, 10);
        assert!(pixel(&stripe, 1)[2] > 240); // red, BGRA
        assert!(pixel(&stripe, 8)[0] > 240); // blue, BGRA
        let bytes = stripe.image.as_bytes(0).unwrap();
        let row = 8 * 4;
        assert_eq!(bytes[..row], bytes[row..2 * row]);
        assert_eq!(bytes[9 * row..], bytes[8 * row..9 * row]);
    }

    #[test]
    fn zune_bgra_matches_image_crate_decode() {
        let gradient = jpeg_sized(64, 24, 75, |x, y| {
            [(x * 4) as u8, (y * 10) as u8, ((x + y) * 3) as u8]
        });
        let (_, padded) = decode_stripe(&gradient, 64, 24).unwrap();
        let reference = image::load_from_memory_with_format(&gradient, ImageFormat::Jpeg)
            .unwrap()
            .into_rgba8();
        let row = 64 * 4;
        for (bgra, rgba) in padded[row..row * 25].chunks(4).zip(reference.pixels()) {
            for (ours, theirs) in [(bgra[2], rgba[0]), (bgra[1], rgba[1]), (bgra[0], rgba[2])] {
                assert!(ours.abs_diff(theirs) <= 1, "{bgra:?} vs {rgba:?}");
            }
            assert_eq!(bgra[3], 255);
        }
    }

    #[test]
    fn a_new_frame_id_presents_the_previous_frame_once() {
        let start = Instant::now();
        let mut pacer = JpegPacer::default();
        assert_eq!(pacer.stripe(1, start), None);
        assert_eq!(pacer.stripe(1, start + Duration::from_millis(1)), None);
        let present = pacer.stripe(2, start + Duration::from_millis(3)).unwrap();
        assert_eq!(present.reason, PresentReason::Boundary);
        assert!(present.new_frame);
        assert_eq!(present.delay, Duration::from_millis(2));
        assert_eq!(pacer.stripe(2, start + Duration::from_millis(4)), None);
    }

    #[test]
    fn settle_deadline_follows_the_last_stripe() {
        let start = Instant::now();
        let mut pacer = JpegPacer::default();
        assert_eq!(pacer.deadline(), None);
        pacer.stripe(1, start);
        assert_eq!(pacer.deadline(), Some(start + SETTLE));
        // Stripes less than SETTLE apart keep pushing it back.
        for ms in [2, 5, 8] {
            pacer.stripe(1, start + Duration::from_millis(ms));
            assert_eq!(
                pacer.deadline(),
                Some(start + Duration::from_millis(ms) + SETTLE)
            );
        }
        let present = pacer
            .present(PresentReason::Settle, start + Duration::from_millis(12))
            .unwrap();
        assert_eq!(present.reason, PresentReason::Settle);
        assert_eq!(present.delay, SETTLE);
        assert_eq!(pacer.deadline(), None, "nothing to wake for while static");
        assert_eq!(pacer.present(PresentReason::Settle, start), None);
    }

    #[test]
    fn a_frame_split_over_a_settle_is_counted_once() {
        let start = Instant::now();
        let mut pacer = JpegPacer::default();
        pacer.stripe(7, start);
        let first = pacer
            .present(PresentReason::Settle, start + SETTLE)
            .unwrap();
        assert!(first.new_frame);
        pacer.stripe(7, start + Duration::from_millis(9));
        let rest = pacer
            .present(PresentReason::Settle, start + Duration::from_millis(13))
            .unwrap();
        assert!(!rest.new_frame);
        // The first stripe of the next frame after a settle presents nothing extra.
        assert_eq!(pacer.stripe(8, start + Duration::from_millis(20)), None);
    }

    #[test]
    fn leaving_jpeg_resets_the_pacer_and_canvas() {
        let start = Instant::now();
        let mut pacer = JpegPacer::default();
        let mut canvas = Canvas::default();
        canvas.resize(2, 2).unwrap();
        canvas.stripe(0, &jpeg([255, 0, 0])).unwrap();
        pacer.stripe(3, start);
        // A hardware frame (or codec switch) clears both: no settle can send a stale picture.
        canvas = Canvas::default();
        pacer.reset();
        assert_eq!(pacer.deadline(), None);
        assert_eq!(pacer.present(PresentReason::Settle, start + SETTLE), None);
        assert!(canvas.snapshot().picture.is_none());
        // After a reset the same id counts as a new frame again.
        pacer.stripe(3, start);
        assert!(
            pacer
                .present(PresentReason::Settle, start + SETTLE)
                .unwrap()
                .new_frame
        );
    }

    /// The old path: `image` to RGB, to RGBA, swap R and B, copy into a full canvas, then one
    /// full clone per flush.
    fn old_decode(stripes: &[(u16, Vec<u8>)], canvas: &mut image::RgbaImage) -> image::RgbaImage {
        use image::ImageDecoder as _;
        let row = canvas.width() as usize * 4;
        for (y, jpeg) in stripes {
            let decoder = ImageReader::with_format(Cursor::new(jpeg), ImageFormat::Jpeg)
                .into_decoder()
                .unwrap();
            let height = decoder.dimensions().1 as usize;
            let mut stripe = image::DynamicImage::from_decoder(decoder)
                .unwrap()
                .into_rgba8();
            for pixel in stripe.pixels_mut() {
                pixel.0.swap(0, 2);
            }
            let start = usize::from(*y) * row;
            let pixels: &mut [u8] = canvas;
            pixels[start..start + height * row].copy_from_slice(&stripe);
        }
        canvas.clone()
    }

    /// Real stripes from `SKERRY_JPEG_BENCH_STRIPES`: 1492-wide files named `*_<y>_<h>.jpg`.
    fn bench_stripes_from_disk() -> Option<Vec<(u16, Vec<u8>)>> {
        let dir = std::env::var_os("SKERRY_JPEG_BENCH_STRIPES")?;
        let mut stripes: Vec<(u16, Vec<u8>)> = std::fs::read_dir(dir)
            .ok()?
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                let name = path.file_stem()?.to_str()?.to_owned();
                let y = name.rsplit('_').nth(1)?.parse().ok()?;
                Some((y, std::fs::read(path).ok()?))
            })
            .collect();
        stripes.sort_by_key(|(y, _)| *y);
        Some(stripes)
    }

    /// `cargo test --release -p skerry-mac jpeg_stripe_decode_cost -- --ignored --nocapture`
    /// Prints CPU ms per frame for every stripe changing (motion) and for one (typing).
    #[test]
    #[ignore = "benchmark"]
    fn jpeg_stripe_decode_cost() {
        let (width, height, count) = (1492u16, 1868u16, 8u16);
        let rows = height / count;
        let synthetic = || -> Vec<(u16, Vec<u8>)> {
            (0..count)
                .map(|i| {
                    let y = i * rows;
                    let h = if i == count - 1 { height - y } else { rows };
                    let jpeg = jpeg_sized(width, h, 75, |x, row| {
                        let y = u32::from(y) + row;
                        let noise = (x.wrapping_mul(2654435761) ^ y.wrapping_mul(40503)) >> 27;
                        [
                            (x / 6) as u8,
                            (y / 8) as u8,
                            ((x ^ y) as u8 & 0xf0) | noise as u8,
                        ]
                    });
                    (y, jpeg)
                })
                .collect()
        };
        let (source, stripes) = match bench_stripes_from_disk() {
            Some(stripes) => ("disk", stripes),
            None => ("synthetic", synthetic()),
        };
        let mut canvas = Canvas::default();
        canvas.resize(width, height).unwrap();
        let mut full = image::RgbaImage::new(width.into(), height.into());
        let frames = 120;
        let time = |f: &mut dyn FnMut()| {
            f();
            let started = Instant::now();
            for _ in 0..frames {
                f();
            }
            started.elapsed().as_secs_f64() * 1000.0 / f64::from(frames)
        };
        for (workload, changed) in [("motion", &stripes[..]), ("typing", &stripes[3..4])] {
            let old = time(&mut || {
                std::hint::black_box(old_decode(changed, &mut full));
            });
            let new = time(&mut || {
                for (y, jpeg) in changed {
                    canvas.insert(canvas.decode(*y, jpeg).unwrap());
                }
                std::hint::black_box(canvas.snapshot());
            });
            println!(
                "jpeg {source} {workload}: old {old:.3} ms/frame, new {new:.3} ms/frame ({:.0}% less)",
                (1.0 - new / old) * 100.0
            );
        }
    }

    // ---- Session access without a stream (ARCH-028) --------------------------------------

    /// Answers one request per response, in order, and returns each request's head.
    async fn answer_heads(
        responses: Vec<String>,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let task = tokio::spawn(async move {
            let mut seen = Vec::new();
            for response in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut chunk = [0u8; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let read = stream.read(&mut chunk).await.unwrap();
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&chunk[..read]);
                }
                stream.write_all(response.as_bytes()).await.unwrap();
                seen.push(String::from_utf8_lossy(&request).to_lowercase());
            }
            seen
        });
        (address, task)
    }

    fn see_other(location: &str) -> String {
        format!(
            "HTTP/1.1 303 See Other\r\nlocation: {location}\r\nset-cookie: sealskin_session_s1=tok; Path=/s1; HttpOnly\r\nset-cookie: other=2; Path=/\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
        )
    }

    #[tokio::test]
    async fn session_access_exchanges_the_token_for_a_cookie() {
        let (address, seen) = answer_heads(vec![see_other("/s1/")]).await;
        let url = Url::parse(&format!("http://{address}/s1/?access_token=tok")).unwrap();
        let access = session_access(url).await.unwrap();
        assert_eq!(access.base.as_str(), format!("http://{address}/s1/"));
        assert_eq!(access.base.query(), None);
        assert_eq!(
            access.cookie.to_str().unwrap(),
            "sealskin_session_s1=tok; other=2"
        );
        assert!(access.cookie.is_sensitive());
        assert_eq!(seen.await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn session_access_refuses_what_socket_request_refuses() {
        let ok = "HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_string();
        let (address, _) = answer_heads(vec![ok]).await;
        let url = Url::parse(&format!("http://{address}/s1/?access_token=t")).unwrap();
        assert_eq!(session_access(url).await.err(), Some(AUTH_ERROR));

        let (address, _) = answer_heads(vec![see_other("https://evil.test/s1/")]).await;
        let url = Url::parse(&format!("http://{address}/s1/?access_token=t")).unwrap();
        assert_eq!(session_access(url).await.err(), Some(ADDRESS_ERROR));

        let room = Url::parse("https://seal.example/room/abc?token=t").unwrap();
        assert_eq!(session_access(room).await.err(), Some(ADDRESS_ERROR));
    }

    #[tokio::test]
    async fn session_access_and_selkies_requests_carry_no_authorization() {
        let listing = r#"{"path":"","entries":[]}"#;
        let browse = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{listing}",
            listing.len()
        );
        let (address, seen) = answer_heads(vec![see_other("/s1/"), browse]).await;
        let url = Url::parse(&format!("http://{address}/s1/?access_token=tok")).unwrap();
        let files = selkies_for_session(url).await.unwrap();
        files.browse("").await.unwrap();
        let seen = seen.await.unwrap();
        assert_eq!(seen.len(), 2);
        assert!(seen[0].starts_with("get /s1/?access_token=tok "));
        assert!(
            seen[1].starts_with("get /s1/api/file-manager/browse"),
            "{}",
            seen[1]
        );
        assert!(
            seen[1].contains("cookie: sealskin_session_s1=tok"),
            "{}",
            seen[1]
        );
        for request in &seen {
            assert!(!request.contains("authorization:"), "{request}");
            assert!(!request.contains("x-sealskin-app"), "{request}");
        }
    }
}
