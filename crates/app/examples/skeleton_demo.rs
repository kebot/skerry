//! Walking-skeleton demo without a real browser.
//!
//! Starts the fake CDP server with several fake page Tabs, writes a temporary
//! `config.toml` with a raw `cdp_url` Connection pointing at it, and runs the
//! real app against it. Every Tab streams a synthetic JPEG test pattern that
//! shows the Tab's title, so switching Tabs is visible. Input, navigation and
//! Tab commands received by the fake page are printed to stdout.
//!
//! Run: `cargo run -p rb-app --example skeleton_demo`
//! Env: RB_DEMO_FPS (default 10); RB_DEMO_OUTAGE=<start_s>:<len_s> drops the
//! connection at start_s and refuses reconnects for len_s seconds (to see the
//! "Reconnecting..." overlay and the recovery).

use std::io::Cursor;
use std::time::{Duration, Instant};

use base64::Engine;
use image::{codecs::jpeg::JpegEncoder, ExtendedColorType, ImageEncoder, RgbImage};
use rb_cdp::testing::FakeCdpServer;
use serde_json::{json, Value};

const W: u32 = 1000;
const H: u32 = 700;

/// 3x5 glyphs, one row per entry (bit 2 = left pixel).
const FONT: &[(char, [u8; 5])] = &[
    ('A', [2, 5, 7, 5, 5]),
    ('B', [6, 5, 6, 5, 6]),
    ('C', [3, 4, 4, 4, 3]),
    ('D', [6, 5, 5, 5, 6]),
    ('E', [7, 4, 7, 4, 7]),
    ('F', [7, 4, 7, 4, 4]),
    ('G', [3, 4, 5, 5, 3]),
    ('H', [5, 5, 7, 5, 5]),
    ('I', [7, 2, 2, 2, 7]),
    ('J', [1, 1, 1, 5, 2]),
    ('K', [5, 5, 6, 5, 5]),
    ('L', [4, 4, 4, 4, 7]),
    ('M', [5, 7, 7, 5, 5]),
    ('N', [6, 5, 5, 5, 5]),
    ('O', [2, 5, 5, 5, 2]),
    ('P', [6, 5, 6, 4, 4]),
    ('Q', [2, 5, 5, 6, 3]),
    ('R', [6, 5, 6, 5, 5]),
    ('S', [3, 4, 2, 1, 6]),
    ('T', [7, 2, 2, 2, 2]),
    ('U', [5, 5, 5, 5, 7]),
    ('V', [5, 5, 5, 5, 2]),
    ('W', [5, 5, 7, 7, 5]),
    ('X', [5, 5, 2, 5, 5]),
    ('Y', [5, 5, 2, 2, 2]),
    ('Z', [7, 1, 2, 4, 7]),
    ('0', [7, 5, 5, 5, 7]),
    ('1', [2, 6, 2, 2, 7]),
    ('2', [7, 1, 7, 4, 7]),
    ('3', [7, 1, 7, 1, 7]),
    ('4', [5, 5, 7, 1, 1]),
    ('5', [7, 4, 7, 1, 7]),
    ('6', [7, 4, 7, 5, 7]),
    ('7', [7, 1, 1, 1, 1]),
    ('8', [7, 5, 7, 5, 7]),
    ('9', [7, 5, 7, 1, 7]),
    ('.', [0, 0, 0, 0, 2]),
    (':', [0, 2, 0, 2, 0]),
    ('/', [1, 1, 2, 4, 4]),
    ('-', [0, 0, 7, 0, 0]),
    ('_', [0, 0, 0, 0, 7]),
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

fn text(img: &mut RgbImage, mut x: i64, y: i64, s: &str, scale: i64, c: [u8; 3]) {
    for ch in s.chars() {
        let up = ch.to_ascii_uppercase();
        if let Some((_, g)) = FONT.iter().find(|(k, _)| *k == up) {
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
        }
        x += 4 * scale;
    }
}

#[derive(Clone)]
struct Tab {
    id: String,
    title: String,
    url: String,
    color: [u8; 3],
}

impl Tab {
    fn info(&self) -> Value {
        json!({"targetId": self.id, "type": "page", "title": self.title,
               "url": self.url, "attached": false})
    }
}

const PALETTE: [[u8; 3]; 6] = [
    [30, 60, 110],
    [20, 90, 60],
    [110, 50, 40],
    [80, 40, 110],
    [100, 80, 20],
    [20, 90, 100],
];

fn render_page(tab: &Tab, n: u64, mouse: Option<(f64, f64, bool)>) -> Vec<u8> {
    let mut img = RgbImage::from_pixel(W, H, image::Rgb(tab.color));
    let grid = [
        tab.color[0].saturating_add(40),
        tab.color[1].saturating_add(40),
        tab.color[2].saturating_add(40),
    ];
    for gx in (0..W as i64).step_by(100) {
        rect(&mut img, gx, 0, 1, H as i64, grid);
    }
    for gy in (0..H as i64).step_by(100) {
        rect(&mut img, 0, gy, W as i64, 1, grid);
    }
    for (x, y, w, h) in [
        (0, 0, W as i64, 6),
        (0, H as i64 - 6, W as i64, 6),
        (0, 0, 6, H as i64),
        (W as i64 - 6, 0, 6, H as i64),
    ] {
        rect(&mut img, x, y, w, h, [255, 200, 0]);
    }
    text(&mut img, 40, 60, &tab.title, 9, [255, 255, 255]);
    text(&mut img, 40, 160, &tab.url, 4, [220, 230, 255]);
    text(&mut img, 40, 640, &format!("FRAME {n}"), 3, [200, 200, 200]);
    let t = n as i64;
    rect(
        &mut img,
        100 + (t * 7) % 600,
        300 + (t * 3) % 150,
        60,
        60,
        [230, 80, 80],
    );
    if let Some((mx, my, down)) = mouse {
        let c = if down { [255, 60, 255] } else { [60, 255, 120] };
        let (mx, my) = (mx as i64, my as i64);
        rect(&mut img, mx - 20, my - 1, 41, 3, c);
        rect(&mut img, mx - 1, my - 20, 3, 41, c);
        text(&mut img, mx + 14, my + 14, &format!("{mx},{my}"), 2, c);
    }
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(Cursor::new(&mut out), 70)
        .write_image(img.as_raw(), W, H, ExtendedColorType::Rgb8)
        .unwrap();
    out
}

fn host_of(url: &str) -> String {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url)
        .to_string()
}

fn publish_targets(server: &FakeCdpServer, tabs: &[Tab]) {
    let infos: Vec<Value> = tabs.iter().map(Tab::info).collect();
    server.reply("Target.getTargets", json!({ "targetInfos": infos }));
}

/// Fake browser: answers commands (statically, see `main`), reacts to Tab
/// commands and streams a Frame of the attached Tab, waiting for the ack.
async fn run_browser(server: FakeCdpServer, fps: f64, outage: Option<(f64, f64)>) {
    let mut tabs: Vec<Tab> = [
        ("Example Domain", "https://example.com/"),
        ("Rust Programming Language", "https://www.rust-lang.org/"),
        ("GitHub rb", "https://github.com/kebot/rb"),
        ("Hacker News", "https://news.ycombinator.com/"),
    ]
    .iter()
    .enumerate()
    .map(|(i, (t, u))| Tab {
        id: format!("T{}", i + 1),
        title: t.to_string(),
        url: u.to_string(),
        color: PALETTE[i % PALETTE.len()],
    })
    .collect();
    let mut next_id = tabs.len() + 1;
    publish_targets(&server, &tabs);

    let mut current = String::new();
    let mut seen = 0usize;
    let mut mouse: Option<(f64, f64, bool)> = None;
    let (mut n, mut acks) = (0u64, 0usize);
    let start = Instant::now();
    let mut outage_stage = 0;
    let period = Duration::from_secs_f64(1.0 / fps);
    loop {
        let began = Instant::now();
        if let Some((at, len)) = outage {
            let t = start.elapsed().as_secs_f64();
            if outage_stage == 0 && t >= at {
                println!("[fake browser] OUTAGE: dropping connections, refusing for {len}s");
                server.refuse_connections(true);
                server.drop_connections();
                outage_stage = 1;
            } else if outage_stage == 1 && t >= at + len {
                println!("[fake browser] OUTAGE over: accepting connections again");
                server.refuse_connections(false);
                outage_stage = 2;
            }
        }
        let cmds = server.commands();
        for c in &cmds[seen.min(cmds.len())..] {
            let p = &c.params;
            match c.method.as_str() {
                "Target.attachToTarget" => {
                    current = p["targetId"].as_str().unwrap_or("").to_string();
                    println!("[fake browser] attach {current}");
                }
                "Target.createTarget" => {
                    let id = format!("T{next_id}");
                    let tab = Tab {
                        id: id.clone(),
                        title: format!("New Tab {}", next_id),
                        url: "about:blank".into(),
                        color: PALETTE[next_id % PALETTE.len()],
                    };
                    next_id += 1;
                    println!("[fake browser] createTarget -> {id}");
                    tabs.push(tab.clone());
                    publish_targets(&server, &tabs);
                    server.emit("Target.targetCreated", json!({"targetInfo": tab.info()}));
                }
                "Target.closeTarget" => {
                    let id = p["targetId"].as_str().unwrap_or("");
                    println!("[fake browser] closeTarget {id}");
                    tabs.retain(|t| t.id != id);
                    publish_targets(&server, &tabs);
                    server.emit("Target.targetDestroyed", json!({"targetId": id}));
                }
                "Page.navigate" => {
                    let url = p["url"].as_str().unwrap_or("").to_string();
                    println!("[fake browser] Page.navigate {url}");
                    if let Some(t) = tabs.iter_mut().find(|t| t.id == current) {
                        t.title = host_of(&url);
                        t.url = url;
                        let info = t.info();
                        publish_targets(&server, &tabs);
                        server.emit("Target.targetInfoChanged", json!({"targetInfo": info}));
                    }
                }
                "Page.reload" | "Page.getNavigationHistory" | "Page.navigateToHistoryEntry" => {
                    println!("[fake browser] {}", c.method)
                }
                "Input.dispatchMouseEvent" => {
                    let ty = p["type"].as_str().unwrap_or("");
                    if let (Some(x), Some(y)) = (p["x"].as_f64(), p["y"].as_f64()) {
                        let down = p["buttons"].as_u64().unwrap_or(0) != 0 && ty != "mouseReleased";
                        mouse = Some((x, y, down));
                        if ty != "mouseMoved" {
                            println!(
                                "[fake browser] mouse {ty} x={x:.1} y={y:.1} button={} clicks={}",
                                p["button"], p["clickCount"]
                            );
                        }
                    }
                }
                "Input.dispatchKeyEvent" | "Input.insertText" => {
                    println!("[fake browser] {} {}", c.method, p)
                }
                _ => {}
            }
        }
        seen = cmds.len();

        if let Some(tab) = tabs.iter().find(|t| t.id == current) {
            n += 1;
            let data = base64::engine::general_purpose::STANDARD.encode(render_page(tab, n, mouse));
            server.emit_in_session(
                "S1",
                "Page.screencastFrame",
                json!({"data": data, "sessionId": n,
                       "metadata": {"deviceWidth": W, "deviceHeight": H, "offsetTop": 0,
                                    "pageScaleFactor": 1, "scrollOffsetX": 0, "scrollOffsetY": 0,
                                    "timestamp": 0}}),
            );
            acks += 1;
            let _ = tokio::time::timeout(
                Duration::from_millis(300),
                server.wait_for_command("Page.screencastFrameAck", acks),
            )
            .await;
            // Acks lost to an outage must not wedge the counter.
            acks = server
                .commands()
                .iter()
                .filter(|c| c.method == "Page.screencastFrameAck")
                .count();
        }
        if let Some(rest) = period.checked_sub(began.elapsed()) {
            tokio::time::sleep(rest).await;
        }
    }
}

fn main() {
    let fps: f64 = std::env::var("RB_DEMO_FPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10.0);
    let outage = std::env::var("RB_DEMO_OUTAGE").ok().and_then(|v| {
        let (a, b) = v.split_once(':')?;
        Some((a.parse().ok()?, b.parse().ok()?))
    });

    let rt = rb_app::app::build_runtime();
    let url = rt.block_on(async {
        let server = FakeCdpServer::start().await;
        let url = server.url();
        server.reply("Target.setDiscoverTargets", json!({}));
        server.reply("Target.attachToTarget", json!({"sessionId": "S1"}));
        server.reply("Target.createTarget", json!({"targetId": "new"}));
        for m in [
            "Target.closeTarget",
            "Page.startScreencast",
            "Page.stopScreencast",
            "Page.screencastFrameAck",
            "Page.reload",
            "Page.navigateToHistoryEntry",
            "Input.dispatchMouseEvent",
            "Input.dispatchKeyEvent",
            "Input.insertText",
        ] {
            server.reply(m, json!({}));
        }
        server.reply("Page.navigate", json!({"frameId": "F"}));
        server.reply(
            "Page.getNavigationHistory",
            json!({"currentIndex": 1, "entries": [{"id": 10, "url": "about:blank"}, {"id": 11, "url": "x"}]}),
        );
        tokio::spawn(run_browser(server, fps, outage));
        url
    });

    let dir = std::env::temp_dir().join(format!("rb-skeleton-demo-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    rb_store::write_private_file(
        &path,
        &format!("[[connection]]\nname = \"demo\"\ncdp_url = \"{url}\"\n"),
    )
    .unwrap();
    println!("[demo] config: {}", path.display());

    let startup = rb_app::startup::load(Some(path), None);
    rb_app::app::run(startup, rt.handle().clone());
    drop(rt);
}
