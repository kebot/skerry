use crate::audio::Playback;
use crate::settings::{Key, Settings};
use futures_util::{SinkExt, StreamExt};
use gpui::RenderImage;
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
    pub fn from_window(width: u16, height: u16) -> Self {
        let scale = (f64::from(MAX_WIDTH) / f64::from(width.max(1)))
            .min(f64::from(MAX_HEIGHT) / f64::from(height.max(1)))
            .min(1.0);
        let even = |size| ((f64::from(size) * scale) as u16 & !1).max(2);
        Self(even(width), even(height))
    }
}

#[derive(Clone, Default)]
pub struct Frame {
    pub width: u16,
    pub height: u16,
    pub image: Option<Arc<RenderImage>>,
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
        if width == 0 || height == 0 || width > MAX_WIDTH || height > MAX_HEIGHT {
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
            image: self.received_image.then(|| {
                Arc::new(RenderImage::new(smallvec![image::Frame::new(
                    self.pixels.clone()
                )]))
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
    pub server_encoder: Option<String>,
    pub rtt_ms: Option<f64>,
}

#[derive(Default)]
struct Diagnostics {
    bytes: usize,
    frames: u64,
    stripes: u64,
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
                / std::mem::take(&mut self.stripes).max(1) as f64,
            dropped: self.dropped,
            server_encoder: self.server_encoder.clone(),
            rtt_ms: self.rtt_ms,
        }
    }
}

pub enum SessionEvent {
    Audio(bool),
    AudioUnavailable(&'static str),
    Video(bool),
    Settings(Settings),
    Stats(StreamSample),
    ResizeAllowed(bool),
    Error(String),
}

pub enum Input {
    Command(String),
    Audio(bool),
    Video(bool),
    Settings(Vec<(Key, u16)>),
    Resolution(Resolution, Option<f32>),
    Appearance(bool),
}

pub struct SessionOptions {
    pub preferences: Vec<(Key, u16)>,
    pub dark: bool,
    pub muted: bool,
}

pub async fn run(
    url: Url,
    frames: watch::Sender<Frame>,
    mut input: mpsc::UnboundedReceiver<Input>,
    pointer: watch::Receiver<Option<String>>,
    options: SessionOptions,
    events: &mpsc::UnboundedSender<SessionEvent>,
) -> Result<(), &'static str> {
    let request = socket_request(url).await?;
    let config = WebSocketConfig::default().max_message_size(Some(16 * 1024 * 1024));
    let (mut socket, _) = tokio::time::timeout(
        Duration::from_secs(10),
        tokio_tungstenite::connect_async_with_config(request, Some(config), false),
    )
    .await
    .map_err(|_| "The Selkies WebSocket did not respond.")?
    .map_err(|_| "Couldn’t connect to the Selkies WebSocket.")?;
    let settings = json!({
        "encoder": "jpeg", "encoderFallback": true,
        "displayId": "primary", "displayScale": 1, "initialClientWidth": 1280,
        "initialClientHeight": 720, "manual_resolution": false,
        "audioRedundancy": false
    });
    let disconnected = "The Selkies stream disconnected. Choose Reconnect to resume.";
    socket
        .send(format!("SETTINGS,{settings}").into())
        .await
        .map_err(|_| disconnected)?;
    let mut frame = Canvas::default();
    frame.resize(1280, 720)?;
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
    let mut heartbeat = tokio::time::interval(Duration::from_secs(1));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            incoming = socket.next() => match incoming {
                Some(Ok(Message::Binary(data))) => {
                    diagnostics.bytes += data.len();
                    match data.first().copied() {
                    Some(3) if data.len() > 6 => {
                        let id = u16::from_be_bytes([data[2], data[3]]);
                        let y = u16::from_be_bytes([data[4], data[5]]);
                        last_frame = Some(id);
                        let started = Instant::now();
                        if frame.stripe(y, &data[6..]).is_some() {
                            diagnostics.stripes += 1;
                            diagnostics.decode += started.elapsed();
                            changed = true;
                            if video_playing.is_none() {
                                video_playing = Some(true);
                                let _ = events.send(SessionEvent::Video(true));
                            }
                            seen_video = true;
                        } else {
                            diagnostics.dropped += 1;
                        }
                    }
                    Some(4) if data.len() > 12 => {
                        last_frame = Some(u16::from_be_bytes([data[2], data[3]]));
                        unsupported_video = true;
                    }
                    Some(1) => { if let Some(audio) = &mut audio { Playback::packet(audio, &data); } }
                    _ => {}
                    }
                },
                Some(Ok(Message::Text(text))) => {
                    if text.starts_with("MODE ") && text != "MODE websockets" {
                        return Err("This server requires WebRTC. SealSkin Native currently uses WebSockets.");
                    }
                    if let Ok(data) = serde_json::from_str::<Value>(text.as_str()) {
                        match data["type"].as_str() {
                            Some("stream_info") => {
                                diagnostics.server_encoder = data["info"]["encoder"].as_str().map(str::to_owned);
                            }
                            Some("stream_resolution") if data["displayId"].as_str().unwrap_or("primary") == "primary" => {
                                let width = dimension(&data["width"])?;
                                let height = dimension(&data["height"])?;
                                frame.resize(width, height)?;
                                changed = true;
                            }
                            Some("server_settings") | Some("display_settings") if primary_display(&data) => {
                                if data["type"] == "display_settings" {
                                    capabilities.live(&data["settings"]);
                                } else {
                                    capabilities.announce(&data["settings"]);
                                    resize_allowed = allows_resize(&data["settings"]);
                                    appearance = data["settings"]["color_scheme"].clone();
                                    let _ = events.send(SessionEvent::ResizeAllowed(resize_allowed));
                                    if !capabilities.jpeg_available {
                                        return Err("This server does not offer JPEG video for the session.");
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
                    Some(Input::Video(enabled)) => {
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
                    Some(Input::Resolution(Resolution(width, height), scale)) => {
                        if resize_allowed {
                            let settings = match scale {
                                None => json!({"manual_resolution": true, "manual_width": width, "manual_height": height, "displayScale": 1}),
                                Some(scale) => json!({"manual_resolution": false, "displayScale": scale}),
                            };
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
                let sample = diagnostics.sample(now.duration_since(sample_start), frame.width, frame.height);
                let _ = events.send(SessionEvent::Stats(sample));
                sample_start = now;
                if ping.is_none() {
                    socket.send(Message::Ping(b"sealskin-rtt".as_slice().into())).await.map_err(|_| disconnected)?;
                    ping = Some(now);
                }
            }
            _ = present.tick() => {
                if changed {
                    diagnostics.frames += 1;
                    frames.send_replace(frame.snapshot());
                    changed = false;
                }
                if let Some(id) = last_frame.take() {
                    socket.send(format!("CLIENT_FRAME_ACK {id} 0").into()).await.map_err(|_| disconnected)?;
                }
                if pointer.has_changed().unwrap_or(false) {
                    let position = pointer.borrow_and_update().clone();
                    if let Some(position) = position {
                        socket.send(position.into()).await.map_err(|_| disconnected)?;
                    }
                }
            }
            _ = &mut video_deadline, if !seen_video && video_playing != Some(false) => {
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
    changes: impl IntoIterator<Item = (Key, u16)>,
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

fn dimension(value: &Value) -> Result<u16, &'static str> {
    value
        .as_u64()
        .and_then(|v| u16::try_from(v).ok())
        .or_else(|| value.as_str().and_then(|v| v.parse().ok()))
        .ok_or("Selkies returned an invalid display resolution.")
}

async fn socket_request(
    url: Url,
) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request, &'static str> {
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
    request.headers_mut().insert("cookie", cookie);
    request.headers_mut().insert(
        "origin",
        url.origin()
            .ascii_serialization()
            .parse()
            .map_err(|_| address_error)?,
    );
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            stripes: 30,
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
        assert!(canvas.snapshot().image.is_some());
        canvas.resize(3, 2).unwrap();
        assert!(canvas.snapshot().image.is_none());
    }
}
