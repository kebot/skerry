use crate::audio::Playback;
use crate::clipboard;
use crate::selkies;
use crate::settings::{Key, ScreenChange, ScreenPolicy, Settings};
use crate::video::{self, Codec, Decoder};
use base64::{engine::general_purpose::STANDARD, Engine};
use futures_util::{SinkExt, StreamExt};
use gpui_kit::RenderImage;
use image::{ImageDecoder, ImageFormat, ImageReader};
use reqwest::Url;
use serde_json::{json, Value};
use smallvec::smallvec;
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::{collections::BTreeSet, io::Cursor};
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
    Jpeg(Arc<RenderImage>),
    Video(video::Surface),
}

#[derive(Default)]
struct Canvas {
    width: u16,
    height: u16,
    pixels: image::RgbaImage,
    received_image: bool,
}

impl Canvas {
    fn resize(&mut self, width: u16, height: u16) -> Result<(), &'static str> {
        if !valid_resolution(width, height) {
            return Err("The remote display resolution is outside the supported range.");
        }
        if (self.width, self.height) != (width, height) {
            *self = Self {
                width,
                height,
                pixels: image::RgbaImage::new(width.into(), height.into()),
                received_image: false,
            };
        }
        Ok(())
    }

    fn stripe(&mut self, y: u16, jpeg: &[u8]) -> Option<()> {
        let decoder = ImageReader::with_format(Cursor::new(jpeg), ImageFormat::Jpeg)
            .into_decoder()
            .ok()?;
        let (width, height) = decoder.dimensions();
        let height = u16::try_from(height).ok()?;
        let end = y.checked_add(height)?;
        if width != u32::from(self.width) || height == 0 || end > self.height {
            return None;
        }
        let mut stripe = image::DynamicImage::from_decoder(decoder)
            .ok()?
            .into_rgba8();
        for pixel in stripe.pixels_mut() {
            pixel.0.swap(0, 2);
        } // GPUI expects BGRA.
        let row = usize::from(self.width) * 4;
        let pixels: &mut [u8] = &mut self.pixels;
        pixels[usize::from(y) * row..usize::from(end) * row].copy_from_slice(&stripe);
        self.received_image = true;
        Some(())
    }

    fn snapshot(&self) -> Frame {
        Frame {
            width: self.width,
            height: self.height,
            picture: self.received_image.then(|| {
                Picture::Jpeg(Arc::new(RenderImage::new(smallvec![image::Frame::new(
                    self.pixels.clone()
                )])))
            }),
        }
    }
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
    pub server_encoder: Option<String>,
    pub rtt_ms: Option<f64>,
}

#[derive(Default)]
struct Diagnostics {
    bytes: usize,
    frames: u64,
    packets: u64,
    codec: Option<Codec>,
    dropped: u64,
    decode: Duration,
    server_encoder: Option<String>,
    rtt_ms: Option<f64>,
}

impl Diagnostics {
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
            server_encoder: self.server_encoder.clone(),
            rtt_ms: self.rtt_ms,
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
    AudioUnavailable(&'static str),
    Video(bool),
    Settings(Settings),
    Stats(StreamSample),
    ResizeAllowed(bool),
    ScreenPolicy(ScreenPolicy),
    Cursor(RemoteCursor),
    FingerScroll(bool),
    ClipboardPolicy(clipboard::Policy),
    Clipboard(clipboard::Incoming),
    /// The session's file routes, once the session cookie is in hand (`ARCH-018`).
    Files(selkies::SelkiesClient),
    FilePolicy(selkies::Policy),
    Error(String),
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
    Video(bool),
    Settings(Vec<(Key, u32)>),
    Resolution(DisplayRequest),
    Screen(ScreenChange),
    Appearance(bool),
    /// Write this into the session's clipboard (`ARCH-017`).
    Clipboard(clipboard::Content),
}

pub struct SessionOptions {
    pub preferences: Vec<(Key, u32)>,
    pub dark: bool,
    pub muted: bool,
    pub codec: Codec,
    pub display: Option<DisplayRequest>,
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

pub async fn run(
    url: Url,
    frames: watch::Sender<Frame>,
    mut input: mpsc::UnboundedReceiver<Input>,
    pointer: watch::Receiver<Option<Pointer>>,
    options: SessionOptions,
    events: &mpsc::UnboundedSender<SessionEvent>,
) -> Result<(), &'static str> {
    let (template, access) = socket_request(url).await?;
    if let Some(access) = access {
        let _ = events.send(SessionEvent::Files(access));
    }
    let config = WebSocketConfig::default().max_message_size(Some(16 * 1024 * 1024));
    // A just-launched session answers 404/5xx or refuses connections until Selkies is up, so
    // keep retrying transient failures for a while instead of failing the first attempt.
    let give_up = Instant::now() + SOCKET_STARTUP_GRACE;
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
        )
        .await
        .map_err(|_| "The Selkies WebSocket did not respond.")?;
        match attempt {
            Ok((socket, _)) => break socket,
            Err(error) if transient_socket_error(&error) && Instant::now() < give_up => {
                tokio::time::sleep(Duration::from_millis(1500)).await;
            }
            Err(_) => return Err("Couldn’t connect to the Selkies WebSocket."),
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
    socket
        .send(format!("SETTINGS,{settings}").into())
        .await
        .map_err(|_| disconnected)?;
    let mut frame = Canvas::default();
    let mut jpeg_size = Resolution(1280, 720);
    let mut active_size = jpeg_size;
    let mut decoded = None;
    let mut changed = false;
    let mut last_frame = None;
    let mut seen_video = false;
    let mut unsupported_video = false;
    let mut audio = None;
    let mut ready = false;
    let mut diagnostics = Diagnostics::default();
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
                    match data.first().copied() {
                    Some(3) if data.len() > 6 && hardware.as_ref().is_none_or(|video| !video.decoded) => {
                        frame.resize(jpeg_size.0, jpeg_size.1)?;
                        active_size = jpeg_size;
                        let id = u16::from_be_bytes([data[2], data[3]]);
                        let y = u16::from_be_bytes([data[4], data[5]]);
                        last_frame = Some(id);
                        let started = Instant::now();
                        if frame.stripe(y, &data[6..]).is_some() {
                            diagnostics.packets += 1;
                            diagnostics.codec = Some(Codec::Jpeg);
                            diagnostics.decode += started.elapsed();
                            changed = true;
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
                                active_size = Resolution(picture.width, picture.height);
                                decoded = Some(picture);
                                changed = false;
                                diagnostics.codec = Some(video.codec);
                                diagnostics.packets += 1;
                                diagnostics.decode += started.elapsed();
                                seen_video = true;
                            } else {
                                diagnostics.dropped += 1;
                                if video.recover(Instant::now()) {
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
                        return Err("This server requires WebRTC. SealSkin Native currently uses WebSockets.");
                    }
                    if let Some(finger_scroll) = finger_scroll_capability(&text) {
                        let _ = events.send(SessionEvent::FingerScroll(finger_scroll));
                        continue;
                    }
                    if let Ok(data) = serde_json::from_str::<Value>(text.as_str()) {
                        match data["type"].as_str() {
                            Some("stream_info") => {
                                diagnostics.server_encoder = data["info"]["encoder"].as_str().map(str::to_owned);
                            }
                            Some("stream_resolution") if data["displayId"].as_str().unwrap_or("primary") == "primary" => {
                                let width = dimension(&data["width"])?;
                                let height = dimension(&data["height"])?;
                                if !valid_resolution(width, height) {
                                    return Err("The remote display resolution is outside the supported range.");
                                }
                                jpeg_size = Resolution(width, height);
                                active_size = jpeg_size;
                                if hardware.as_ref().is_none_or(|video| !video.decoded) {
                                    frame.resize(width, height)?;
                                    changed = true;
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
                                        decoded = None;
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
                                    if let Some(legacy) = legacy_settings(&capabilities) {
                                        socket.send(legacy.into()).await.map_err(|_| disconnected)?;
                                    }
                                    if let Some((command, applied)) = settings_command(&capabilities, options.preferences.iter().copied()) {
                                        socket.send(command.into()).await.map_err(|_| disconnected)?;
                                        capabilities.live(&applied);
                                        let _ = events.send(SessionEvent::Settings(capabilities.clone()));
                                    }
                                    if data["settings"]["audio_enabled"]["value"].as_bool() == Some(false) {
                                        let _ = events.send(SessionEvent::AudioUnavailable("Audio is disabled by the server."));
                                    } else {
                                        audio = Playback::new();
                                        match audio_start(&data["settings"], audio.is_some(), options.muted) {
                                            AudioStart::Play => socket.send("START_AUDIO".into()).await.map_err(|_| disconnected)?,
                                            AudioStart::Muted => { let _ = events.send(SessionEvent::Audio(false)); }
                                            AudioStart::Unavailable => { let _ = events.send(SessionEvent::AudioUnavailable("No compatible 48 kHz stereo audio output is available.")); }
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
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return Err(disconnected),
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
                    None => { let _ = socket.close(None).await; return Ok(()); }
                };
                if let Some(key) = text.strip_prefix("kd,").and_then(|key| key.parse::<u32>().ok()) {
                    held.insert(key);
                } else if let Some(key) = text.strip_prefix("ku,").and_then(|key| key.parse::<u32>().ok()) {
                    held.remove(&key);
                }
                socket.send(text.into()).await.map_err(|_| disconnected)?;
            }
            _ = heartbeat.tick(), if !held.is_empty() => {
                let keys = held.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
                socket.send(format!("kh,{keys}").into()).await.map_err(|_| disconnected)?;
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
            _ = present.tick() => {
                if video_playing != Some(false) && hardware.as_ref().is_some_and(|video| video.expired(Instant::now())) {
                    if !capabilities.codecs.contains(&Codec::Jpeg) {
                        return Err("Hardware video decoding failed and the server does not allow JPEG fallback.");
                    }
                    codec = Codec::Jpeg;
                    hardware = None;
                    decoded = None;
                    seen_video = false;
                    unsupported_video = false;
                    video_deadline.as_mut().reset(tokio::time::Instant::now() + WAIT_FOR_VIDEO);
                    socket.send(codec_command(codec).into()).await.map_err(|_| disconnected)?;
                    socket.send("REQUEST_KEYFRAME".into()).await.map_err(|_| disconnected)?;
                }
                if let Some(picture) = decoded.take().or_else(|| changed.then(|| frame.snapshot())) {
                    if picture.picture.is_some() { diagnostics.frames += 1; }
                    frames.send_replace(picture);
                    changed = false;
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
                });
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum AudioStart {
    Play,
    Muted,
    Unavailable,
}

fn audio_start(settings: &Value, output: bool, muted: bool) -> AudioStart {
    if !output {
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
const SOCKET_STARTUP_GRACE: Duration = Duration::from_secs(45);

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

/// The WebSocket handshake for the session, and the session's file routes under the same
/// cookie (`ARCH-018`), or `None` when they cannot be built.
async fn socket_request(
    url: Url,
) -> Result<
    (
        tokio_tungstenite::tungstenite::handshake::client::Request,
        Option<selkies::SelkiesClient>,
    ),
    &'static str,
> {
    let address_error =
        "SealSkin returned an unsupported session address. Open an individual session.";
    let auth_error =
        "Couldn’t authorize the native session. Refresh the launcher and resume again.";
    if !url.query_pairs().any(|(key, _)| key == "access_token") || !url.path().ends_with('/') {
        return Err(address_error);
    }
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|_| auth_error)?;
    let response = http.get(url.clone()).send().await.map_err(|_| auth_error)?;
    if response.status() != reqwest::StatusCode::SEE_OTHER {
        return Err(auth_error);
    }
    let location = response
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .ok_or(address_error)?;
    let redirect = url.join(location).map_err(|_| address_error)?;
    if redirect.origin() != url.origin() || redirect.path() != url.path() {
        return Err(address_error);
    }
    let cookie = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok()?.split(';').next())
        .collect::<Vec<_>>()
        .join("; ");
    if cookie.is_empty() {
        return Err(auth_error);
    }
    let mut websocket = redirect.join("api/websockets").map_err(|_| address_error)?;
    websocket.set_scheme("wss").map_err(|_| address_error)?;
    let mut request = websocket
        .as_str()
        .into_client_request()
        .map_err(|_| address_error)?;
    let mut cookie = cookie
        .parse::<reqwest::header::HeaderValue>()
        .map_err(|_| auth_error)?;
    cookie.set_sensitive(true);
    let mut base = redirect.clone();
    base.set_query(None);
    base.set_fragment(None);
    let access = selkies::SelkiesClient::new(base, cookie.clone()).ok();
    request.headers_mut().insert("cookie", cookie);
    request.headers_mut().insert(
        "origin",
        url.origin()
            .ascii_serialization()
            .parse()
            .map_err(|_| address_error)?,
    );
    Ok((request, access))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let unchanged = *canvas.pixels.get_pixel(0, 0);
        canvas.stripe(1, &blue).unwrap();
        assert_eq!(*canvas.pixels.get_pixel(0, 0), unchanged);
        assert!(unchanged.0[2] > 240); // BGRA red
        assert!(canvas.pixels.get_pixel(0, 1).0[0] > 240); // BGRA blue
        assert!(canvas.stripe(2, &red).is_none());
        assert!(canvas.snapshot().picture.is_some());
        canvas.resize(3, 2).unwrap();
        assert!(canvas.snapshot().picture.is_none());
    }
}
