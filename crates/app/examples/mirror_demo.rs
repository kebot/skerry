//! Mirror demo without a real browser.
//!
//! Starts the fake CDP server, attaches a Session and streams synthetic JPEG
//! Frames (a moving test pattern with a grid, a frame counter and a marker at
//! the last mouse position the fake page received). The window is deliberately
//! not the page's aspect ratio so the letterbox bars are visible. Mapped
//! remote coordinates of every input event are printed to stdout.
//!
//! Run: `cargo run -p rb-app --example mirror_demo`
//! Env: RB_DEMO_FPS (default 10), RB_DEMO_FRAMES (exit after N Frames).

use std::io::Cursor;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use gpui::{
    px, size, App, AppContext, Application, Bounds, Focusable, KeyBinding, WindowBounds,
    WindowOptions,
};
use image::{codecs::jpeg::JpegEncoder, ExtendedColorType, ImageEncoder, RgbImage};
use rb_app::mirror::{Mirror, MirrorBackend};
use rb_app::state::Quit;
use rb_cdp::testing::FakeCdpServer;
use rb_cdp::{Client, ScreencastOptions};
use serde_json::{json, Value};

const W: u32 = 1000;
const H: u32 = 700;

/// 3x5 digit glyphs, one row per entry (bit 2 = left pixel).
const DIGITS: [[u8; 5]; 10] = [
    [7, 5, 5, 5, 7],
    [2, 6, 2, 2, 7],
    [7, 1, 7, 4, 7],
    [7, 1, 7, 1, 7],
    [5, 5, 7, 1, 1],
    [7, 4, 7, 1, 7],
    [7, 4, 7, 5, 7],
    [7, 1, 1, 1, 1],
    [7, 5, 7, 5, 7],
    [7, 5, 7, 1, 7],
];

fn put(img: &mut RgbImage, x: i64, y: i64, c: [u8; 3]) {
    if x >= 0 && y >= 0 && (x as u32) < W && (y as u32) < H {
        img.put_pixel(x as u32, y as u32, image::Rgb(c));
    }
}

fn rect(img: &mut RgbImage, x: i64, y: i64, w: i64, h: i64, c: [u8; 3]) {
    for yy in y..y + h {
        for xx in x..x + w {
            put(img, xx, yy, c);
        }
    }
}

fn number(img: &mut RgbImage, mut x: i64, y: i64, n: u64, scale: i64, c: [u8; 3]) {
    for ch in n.to_string().chars() {
        let g = DIGITS[ch.to_digit(10).unwrap() as usize];
        for (row, bits) in g.iter().enumerate() {
            for col in 0..3 {
                if bits & (4 >> col) != 0 {
                    rect(
                        img,
                        x + col * scale,
                        y + row as i64 * scale,
                        scale,
                        scale,
                        c,
                    );
                }
            }
        }
        x += 4 * scale;
    }
}

/// Fake page: grid every 100 px with coordinate labels, a border (so the
/// letterbox edge is obvious), a moving box, the frame counter, and a marker
/// at the last mouse position the server received.
fn render_page(n: u64, mouse: Option<(f64, f64, bool)>) -> Vec<u8> {
    let mut img = RgbImage::from_pixel(W, H, image::Rgb([24, 40, 64]));
    for gx in (0..W as i64).step_by(100) {
        rect(&mut img, gx, 0, 1, H as i64, [90, 120, 160]);
        number(&mut img, gx + 4, 4, gx as u64, 2, [200, 210, 230]);
    }
    for gy in (0..H as i64).step_by(100) {
        rect(&mut img, 0, gy, W as i64, 1, [90, 120, 160]);
        if gy > 0 {
            number(&mut img, 4, gy + 4, gy as u64, 2, [200, 210, 230]);
        }
    }
    // border
    for (x, y, w, h) in [
        (0, 0, W as i64, 6),
        (0, H as i64 - 6, W as i64, 6),
        (0, 0, 6, H as i64),
        (W as i64 - 6, 0, 6, H as i64),
    ] {
        rect(&mut img, x, y, w, h, [255, 200, 0]);
    }
    // moving box
    let t = n as i64;
    let bx = 100 + (t * 7) % (W as i64 - 300);
    let by = 250 + ((t * 3) % 200);
    rect(&mut img, bx, by, 80, 80, [230, 80, 80]);
    // big frame counter
    number(&mut img, 380, 560, n, 10, [255, 255, 255]);
    if let Some((mx, my, down)) = mouse {
        let c = if down { [255, 60, 255] } else { [60, 255, 120] };
        let (mx, my) = (mx as i64, my as i64);
        rect(&mut img, mx - 20, my - 1, 41, 3, c);
        rect(&mut img, mx - 1, my - 20, 3, 41, c);
        for k in -9..=9i64 {
            put(&mut img, mx + k, my - 9, c);
            put(&mut img, mx + k, my + 9, c);
            put(&mut img, mx - 9, my + k, c);
            put(&mut img, mx + 9, my + k, c);
        }
    }
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(Cursor::new(&mut out), 60)
        .write_image(img.as_raw(), W, H, ExtendedColorType::Rgb8)
        .unwrap();
    out
}

fn describe(method: &str, p: &Value) -> String {
    match method {
        "Input.dispatchMouseEvent" => format!(
            "mouse {} x={:.1} y={:.1} button={} buttons={} clicks={} dx={} dy={} mods={}",
            p["type"].as_str().unwrap_or("?"),
            p["x"].as_f64().unwrap_or(0.0),
            p["y"].as_f64().unwrap_or(0.0),
            p["button"].as_str().unwrap_or("?"),
            p["buttons"],
            p["clickCount"],
            p.get("deltaX").unwrap_or(&Value::Null),
            p.get("deltaY").unwrap_or(&Value::Null),
            p["modifiers"],
        ),
        _ => format!("{method} {p}"),
    }
}

/// Fake page loop: emit a Frame, wait for its ack (like Chrome), repeat.
async fn run_page(server: FakeCdpServer, fps: f64, max_frames: Option<u64>) {
    let mut seen = 0usize;
    let mut mouse: Option<(f64, f64, bool)> = None;
    let mut acks = 0usize;
    let mut n = 0u64;
    let period = Duration::from_secs_f64(1.0 / fps);
    loop {
        let started = Instant::now();
        for c in server.commands().iter().skip(seen) {
            seen += 1;
            if c.method.starts_with("Input.") {
                println!("[fake page] {}", describe(&c.method, &c.params));
            }
            if c.method == "Input.dispatchMouseEvent" {
                let ty = c.params["type"].as_str().unwrap_or("");
                let (x, y) = (c.params["x"].as_f64(), c.params["y"].as_f64());
                if let (Some(x), Some(y)) = (x, y) {
                    let down =
                        c.params["buttons"].as_u64().unwrap_or(0) != 0 && ty != "mouseReleased";
                    mouse = Some((x, y, down));
                }
            }
        }
        n += 1;
        let data = base64::engine::general_purpose::STANDARD.encode(render_page(n, mouse));
        server.emit_in_session(
            "S1",
            "Page.screencastFrame",
            json!({
                "data": data,
                "sessionId": n,
                "metadata": {
                    "deviceWidth": W, "deviceHeight": H, "offsetTop": 0,
                    "pageScaleFactor": 1, "scrollOffsetX": 0, "scrollOffsetY": 0,
                    "timestamp": 0
                }
            }),
        );
        acks += 1;
        // Chrome sends the next Frame only after the previous ack.
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            server.wait_for_command("Page.screencastFrameAck", acks),
        )
        .await;
        if max_frames.is_some_and(|m| n >= m) {
            println!("[fake page] sent {n} Frames, exiting");
            std::process::exit(0);
        }
        if let Some(rest) = period.checked_sub(started.elapsed()) {
            tokio::time::sleep(rest).await;
        }
    }
}

fn main() {
    let fps: f64 = std::env::var("RB_DEMO_FPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10.0);
    let max_frames: Option<u64> = std::env::var("RB_DEMO_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok());

    // tokio runtime for all CDP I/O, kept alive for the whole process.
    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap(),
    );
    let backend = rt.block_on(async {
        let server = FakeCdpServer::start().await;
        server.reply("Target.attachToTarget", json!({"sessionId": "S1"}));
        for m in [
            "Page.startScreencast",
            "Page.stopScreencast",
            "Page.screencastFrameAck",
            "Input.dispatchMouseEvent",
            "Input.dispatchKeyEvent",
            "Input.insertText",
        ] {
            server.reply(m, json!({}));
        }
        let client = Client::connect(&server.url(), None).await.unwrap();
        let session = client.attach_to_target("T1").await.unwrap();
        let backend = MirrorBackend::from_session(session, ScreencastOptions::default())
            .await
            .unwrap();
        tokio::spawn(async move {
            let _client = client; // keep the connection open
            run_page(server, fps, max_frames).await;
        });
        backend
    });

    Application::new().run(move |cx: &mut App| {
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        // 1.5 aspect window vs 1000x700 (1.43) page: slim bars left and right.
        // Use a clearly different shape so bars are obvious.
        let bounds = Bounds::centered(None, size(px(1100.), px(560.)), cx);
        let mut backend = Some(backend);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            move |window, cx| {
                let mirror = cx.new(|cx| Mirror::new(backend.take().unwrap(), cx));
                window.focus(&mirror.read(cx).focus_handle(cx));
                mirror
            },
        )
        .expect("open window");
        cx.activate(true);
    });
    // `run` blocks until quit; the runtime (and the fake page) live until here.
    drop(rt);
}
