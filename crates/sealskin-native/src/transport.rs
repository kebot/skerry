use crate::audio::Playback;
use futures_util::{SinkExt, StreamExt};
use gpui::RenderImage;
use image::{ImageDecoder, ImageFormat, ImageReader};
use reqwest::Url;
use serde_json::{json, Value};
use smallvec::smallvec;
use std::sync::Arc;
use std::time::Duration;
use std::{collections::BTreeSet, io::Cursor};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::{
    client::IntoClientRequest, protocol::WebSocketConfig, Message,
};

const MAX_WIDTH: u16 = 4096;
const MAX_HEIGHT: u16 = 2160;
const WAIT_FOR_VIDEO: Duration = Duration::from_secs(15);

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

pub enum SessionEvent {
    Audio(bool),
    Error(String),
}

pub enum Input {
    Command(String),
    Audio(bool),
}

pub async fn run(
    url: Url,
    frames: watch::Sender<Frame>,
    mut input: mpsc::UnboundedReceiver<Input>,
    pointer: watch::Receiver<Option<String>>,
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
        "framerate": 30, "jpeg_quality": 75, "audioRedundancy": false
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
                Some(Ok(Message::Binary(data))) => match data.first().copied() {
                    Some(3) if data.len() > 6 => {
                        let id = u16::from_be_bytes([data[2], data[3]]);
                        let y = u16::from_be_bytes([data[4], data[5]]);
                        last_frame = Some(id);
                        if frame.stripe(y, &data[6..]).is_some() {
                            changed = true;
                            seen_video = true;
                        }
                    }
                    Some(4) if data.len() > 12 => {
                        last_frame = Some(u16::from_be_bytes([data[2], data[3]]));
                        unsupported_video = true;
                    }
                    Some(1) => { if let Some(audio) = &mut audio { Playback::packet(audio, &data); } }
                    _ => {}
                },
                Some(Ok(Message::Text(text))) => {
                    if text.starts_with("MODE ") && text != "MODE websockets" {
                        return Err("This server requires WebRTC. SealSkin Native currently uses WebSockets.");
                    }
                    if let Ok(data) = serde_json::from_str::<Value>(text.as_str()) {
                        match data["type"].as_str() {
                            Some("stream_resolution") if data["displayId"].as_str().unwrap_or("primary") == "primary" => {
                                let width = dimension(&data["width"])?;
                                let height = dimension(&data["height"])?;
                                frame.resize(width, height)?;
                                changed = true;
                            }
                            Some("server_settings") if !ready => {
                                ready = true;
                                if data["settings"]["audio_enabled"]["value"].as_bool().unwrap_or(true) {
                                    audio = Playback::new();
                                    if audio.is_some() {
                                        socket.send("START_AUDIO".into()).await.map_err(|_| disconnected)?;
                                    }
                                }
                            }
                            _ => {}
                        }
                    } else if text == "AUDIO_STARTED" && audio.is_some() {
                        let _ = events.send(SessionEvent::Audio(true));
                    } else if text == "AUDIO_STOPPED" && audio.is_some() {
                        let _ = events.send(SessionEvent::Audio(false));
                    }
                }
                Some(Ok(Message::Ping(_))) => { socket.flush().await.map_err(|_| disconnected)?; }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return Err(disconnected),
                _ => {}
            },
            command = input.recv() => {
                let text = match command {
                    Some(Input::Command(text)) => text,
                    Some(Input::Audio(enabled)) => {
                        if let Some(audio) = &mut audio { audio.clear(); }
                        if enabled { "START_AUDIO" } else { "STOP_AUDIO" }.to_string()
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
            _ = present.tick() => {
                if changed {
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
            _ = &mut video_deadline, if !seen_video => {
                return Err(if unsupported_video {
                    "The server did not switch to JPEG. Enable JPEG for this session and reconnect."
                } else {
                    "No video arrived from Selkies. Choose Reconnect to try again."
                });
            }
        }
    }
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
