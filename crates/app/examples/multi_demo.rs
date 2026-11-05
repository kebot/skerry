//! Multi-Connection demo without a real browser.
//!
//! Starts THREE fake CDP servers ("alpha", "beta", "lab") with different Tab
//! sets and test-pattern colours, writes a temp `config.toml` (alpha and beta
//! auto-connect; lab connects lazily) and runs the real app against them.
//! `lab` has stored Placeholder Tabs (seeded state) so the Sidebar shows
//! dimmed entries until its chip is clicked.
//!
//! The demo prints, per server, every screencast start/stop and how many
//! screencasts are running ("exactly one app-wide" is visible there), dialog
//! answers and Tab commands.
//!
//! Control while it runs: append lines to the command file (path printed at
//! start):
//!   kill <server> [secs]                    drop connections, refuse for secs (default 6)
//!   dialog <server> <target> <kind> <text>  kind: alert|confirm|prompt|beforeunload
//!   popup <server> <opener> <title>         open a popup Tab (openerId)
//!
//! `alpha` has `agent_proxy = true` on a fixed port and secret, so the demo
//! can play the Agent: it connects `rb_cdp::Client` to the proxy endpoint
//! (printed at start). Commands (same command file):
//!   agent attach                  connect an Agent client to alpha's proxy
//!   agent nav <target> <url>      attach (once) to the target, Page.navigate
//!   agent detach                  close the Agent client
//! Env RB_DEMO_AGENT_PORT (default 39333).
//!
//! Run: `cargo run -p rb-app --example multi_demo`
//! Env: RB_DEMO_FPS (default 8). RB_STATE is a temp file unless set.

use std::collections::HashMap;
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

fn render_page(label: &str, tab: &Tab, n: u64, mouse: Option<(f64, f64, bool)>) -> Vec<u8> {
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
    text(&mut img, 40, 30, label, 6, [255, 220, 120]);
    text(&mut img, 40, 100, &tab.title, 7, [255, 255, 255]);
    text(&mut img, 40, 190, &tab.url, 4, [220, 230, 255]);
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

struct Spec {
    name: &'static str,
    prefix: &'static str,
    colors: [[u8; 3]; 3],
    tabs: &'static [(&'static str, &'static str)],
    auto_connect: bool,
    agent: bool,
}

const SPECS: [Spec; 3] = [
    Spec {
        name: "alpha",
        prefix: "A",
        colors: [[30, 60, 130], [20, 50, 100], [50, 80, 150]],
        tabs: &[
            ("Example Domain", "https://example.com/"),
            ("Rust Language", "https://www.rust-lang.org/"),
            ("Hacker News", "https://news.ycombinator.com/"),
        ],
        auto_connect: true,
        agent: true,
    },
    Spec {
        name: "beta",
        prefix: "B",
        colors: [[20, 110, 60], [15, 85, 45], [40, 130, 80]],
        tabs: &[
            ("Crates io", "https://crates.io/"),
            ("Docs rs", "https://docs.rs/"),
        ],
        auto_connect: true,
        agent: false,
    },
    Spec {
        name: "lab",
        prefix: "L",
        colors: [[130, 70, 20], [100, 55, 15], [150, 90, 40]],
        tabs: &[
            ("Lab Notebook", "https://lab.example/notebook"),
            ("Lab Metrics", "https://lab.example/metrics"),
            ("Lab Queue", "https://lab.example/queue"),
        ],
        auto_connect: false,
        agent: false,
    },
];

const AGENT_SECRET: &str = "demo-secret-12345";

fn agent_port() -> u16 {
    std::env::var("RB_DEMO_AGENT_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(39333)
}

/// The demo plays the Agent through the proxy.
#[derive(Default)]
struct Player {
    client: Option<rb_cdp::Client>,
    sessions: HashMap<String, rb_cdp::Session>,
}

impl Player {
    async fn run(&mut self, w: &[&str]) {
        let ws = format!(
            "ws://127.0.0.1:{}/{AGENT_SECRET}/devtools/browser/rb",
            agent_port()
        );
        match w {
            ["attach"] => match rb_cdp::Client::connect(&ws, None).await {
                Ok(c) => {
                    self.client = Some(c);
                    println!("[agent] connected to {ws}");
                }
                Err(e) => println!("[agent] connect failed: {e}"),
            },
            ["nav", target, url] => {
                let Some(c) = &self.client else {
                    return println!("[agent] not attached");
                };
                if !self.sessions.contains_key(*target) {
                    match c.attach_to_target(target).await {
                        Ok(s) => {
                            self.sessions.insert(target.to_string(), s);
                        }
                        Err(e) => return println!("[agent] attach failed: {e}"),
                    }
                }
                let r = self.sessions[*target]
                    .send("Page.navigate", Some(json!({"url": url})))
                    .await;
                println!("[agent] navigate {target} {url}: {r:?}");
            }
            ["detach"] => {
                self.sessions.clear();
                if let Some(c) = self.client.take() {
                    c.close().await;
                }
                println!("[agent] detached");
            }
            _ => println!("[agent] bad command"),
        }
    }
}

/// Control commands from the command file, per server.
enum Ctl {
    Kill(f64),
    Dialog {
        target: String,
        kind: String,
        text: String,
    },
    Popup {
        opener: String,
        title: String,
    },
}

fn target_of_session(sid: &str) -> &str {
    sid.split_once('-').map(|x| x.1).unwrap_or(sid)
}

async fn run_server(
    spec: &'static Spec,
    server: FakeCdpServer,
    fps: f64,
    mut ctl: tokio::sync::mpsc::UnboundedReceiver<Ctl>,
) {
    let mut tabs: Vec<Tab> = spec
        .tabs
        .iter()
        .enumerate()
        .map(|(i, (t, u))| Tab {
            id: format!("{}{}", spec.prefix, i + 1),
            title: t.to_string(),
            url: u.to_string(),
            color: spec.colors[i % 3],
        })
        .collect();
    let mut next_id = tabs.len() + 1;
    publish_targets(&server, &tabs);

    let mut seen = 0usize;
    // Sessions with a running screencast, by session id -> frame counter.
    let mut casting: HashMap<String, u64> = HashMap::new();
    let mut acks: HashMap<String, usize> = HashMap::new();
    let mut mouse: Option<(f64, f64, bool)> = None;
    let mut refuse_until: Option<Instant> = None;
    let period = Duration::from_secs_f64(1.0 / fps);
    loop {
        let began = Instant::now();
        if refuse_until.is_some_and(|t| Instant::now() >= t) {
            println!("[{}] accepting connections again", spec.name);
            server.refuse_connections(false);
            refuse_until = None;
        }
        while let Ok(c) = ctl.try_recv() {
            match c {
                Ctl::Kill(secs) => {
                    println!("[{}] KILL: dropping connections for {secs}s", spec.name);
                    server.refuse_connections(true);
                    server.drop_connections();
                    refuse_until = Some(Instant::now() + Duration::from_secs_f64(secs));
                    casting.clear();
                }
                Ctl::Dialog { target, kind, text } => {
                    let sid = server
                        .commands()
                        .iter()
                        .find(|c| {
                            c.method == "Page.enable"
                                && c.session_id
                                    .as_deref()
                                    .is_some_and(|s| s.ends_with(&format!("-{target}")))
                        })
                        .and_then(|c| c.session_id.clone());
                    let url = tabs
                        .iter()
                        .find(|t| t.id == target)
                        .map(|t| t.url.clone())
                        .unwrap_or_default();
                    match sid {
                        Some(sid) => {
                            println!("[{}] dialog {kind} on {target}: {text}", spec.name);
                            server.emit_in_session(
                                &sid,
                                "Page.javascriptDialogOpening",
                                json!({"url": url, "message": text, "type": kind,
                                       "hasBrowserHandler": true, "defaultPrompt": "Bob"}),
                            );
                        }
                        None => println!("[{}] no observer session for {target}", spec.name),
                    }
                }
                Ctl::Popup { opener, title } => {
                    let id = format!("{}{}", spec.prefix, next_id);
                    let tab = Tab {
                        id: id.clone(),
                        title,
                        url: "https://popup.example/".into(),
                        color: spec.colors[next_id % 3],
                    };
                    next_id += 1;
                    println!("[{}] popup {id} opened by {opener}", spec.name);
                    tabs.push(tab.clone());
                    publish_targets(&server, &tabs);
                    let mut info = tab.info();
                    info["openerId"] = json!(opener);
                    server.emit("Target.targetCreated", json!({"targetInfo": info}));
                }
            }
        }
        let cmds = server.commands();
        // A reconnect forgets all sessions: restart bookkeeping when the
        // recorded command list is shorter than what we have consumed.
        for c in &cmds[seen.min(cmds.len())..] {
            let p = &c.params;
            let sid = c.session_id.clone().unwrap_or_default();
            match c.method.as_str() {
                "Page.startScreencast" => {
                    casting.insert(sid.clone(), 0);
                    println!(
                        "[{}] screencast START {} (running here: {})",
                        spec.name,
                        target_of_session(&sid),
                        casting.len()
                    );
                }
                "Page.stopScreencast" => {
                    casting.remove(&sid);
                    println!(
                        "[{}] screencast STOP  {} (running here: {})",
                        spec.name,
                        target_of_session(&sid),
                        casting.len()
                    );
                }
                "Page.handleJavaScriptDialog" => {
                    println!(
                        "[{}] Page.handleJavaScriptDialog accept={} promptText={}",
                        spec.name, p["accept"], p["promptText"]
                    );
                    server.emit_in_session(
                        &sid,
                        "Page.javascriptDialogClosed",
                        json!({"result": p["accept"], "userInput": p["promptText"].as_str().unwrap_or("")}),
                    );
                }
                "Target.createTarget" => {
                    let id = format!("{}{}", spec.prefix, next_id);
                    let tab = Tab {
                        id: id.clone(),
                        title: format!("New Tab {next_id}"),
                        url: "about:blank".into(),
                        color: spec.colors[next_id % 3],
                    };
                    next_id += 1;
                    println!("[{}] createTarget -> {id}", spec.name);
                    tabs.push(tab.clone());
                    publish_targets(&server, &tabs);
                    server.emit("Target.targetCreated", json!({"targetInfo": tab.info()}));
                }
                "Target.closeTarget" => {
                    let id = p["targetId"].as_str().unwrap_or("");
                    println!("[{}] closeTarget {id}", spec.name);
                    tabs.retain(|t| t.id != id);
                    publish_targets(&server, &tabs);
                    server.emit("Target.targetDestroyed", json!({"targetId": id}));
                }
                "Page.navigate" => {
                    let url = p["url"].as_str().unwrap_or("").to_string();
                    println!("[{}] Page.navigate {url}", spec.name);
                    if let Some(t) = tabs.iter_mut().find(|t| t.id == target_of_session(&sid)) {
                        t.title = host_of(&url);
                        t.url = url;
                        let info = t.info();
                        publish_targets(&server, &tabs);
                        server.emit("Target.targetInfoChanged", json!({"targetInfo": info}));
                    }
                }
                "Input.dispatchMouseEvent" => {
                    let ty = p["type"].as_str().unwrap_or("");
                    if let (Some(x), Some(y)) = (p["x"].as_f64(), p["y"].as_f64()) {
                        let down = p["buttons"].as_u64().unwrap_or(0) != 0 && ty != "mouseReleased";
                        mouse = Some((x, y, down));
                        if ty != "mouseMoved" {
                            println!("[{}] mouse {ty} x={x:.0} y={y:.0}", spec.name);
                        }
                    }
                }
                "Input.dispatchKeyEvent" | "Input.insertText" => {
                    println!("[{}] {} {}", spec.name, c.method, p)
                }
                _ => {}
            }
        }
        seen = cmds.len();

        let sessions: Vec<String> = casting.keys().cloned().collect();
        for sid in sessions {
            let Some(tab) = tabs.iter().find(|t| t.id == target_of_session(&sid)) else {
                continue;
            };
            let n = casting.entry(sid.clone()).or_insert(0);
            *n += 1;
            let data = base64::engine::general_purpose::STANDARD.encode(render_page(
                &spec.name.to_uppercase(),
                tab,
                *n,
                mouse,
            ));
            server.emit_in_session(
                &sid,
                "Page.screencastFrame",
                json!({"data": data, "sessionId": *n,
                       "metadata": {"deviceWidth": W, "deviceHeight": H, "offsetTop": 0,
                                    "pageScaleFactor": 1, "scrollOffsetX": 0, "scrollOffsetY": 0,
                                    "timestamp": 0}}),
            );
            let _ = acks.entry(sid.clone()).or_insert(0);
        }
        // Wait briefly for acks (the client acks every Frame it shows).
        tokio::time::sleep(Duration::from_millis(60)).await;
        if let Some(rest) = period.checked_sub(began.elapsed()) {
            tokio::time::sleep(rest).await;
        }
    }
}

fn seed_state() -> String {
    r#"
[[group]]
id = "g1"
name = "Reading"
color = "violet"

[[tab]]
connection = "lab"
title = "Lab Notebook"
url = "https://lab.example/notebook"
pinned = true

[[tab]]
connection = "lab"
title = "Lab Metrics"
url = "https://lab.example/metrics"
group = "g1"

[[tab]]
connection = "alpha"
title = "Hacker News"
url = "https://news.ycombinator.com/"
pinned = true

[[tab]]
connection = "alpha"
title = "Rust Language"
url = "https://www.rust-lang.org/"
group = "g1"

[[tab]]
connection = "beta"
title = "Crates io"
url = "https://crates.io/"
group = "g1"
"#
    .to_string()
}

fn main() {
    let fps: f64 = std::env::var("RB_DEMO_FPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8.0);
    let dir = std::env::temp_dir().join(format!("rb-multi-demo-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let rt = rb_app::app::build_runtime();
    let mut config = String::new();
    let mut senders: HashMap<&'static str, tokio::sync::mpsc::UnboundedSender<Ctl>> =
        HashMap::new();
    for spec in SPECS.iter() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        senders.insert(spec.name, tx);
        let url = rt.block_on(async {
            let server = FakeCdpServer::start().await;
            let url = server.url();
            server.reply("Target.setDiscoverTargets", json!({}));
            let n = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            server.reply_with("Target.attachToTarget", move |c| {
                let i = n.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                Ok(json!({"sessionId": format!("S{i}-{}", c.params["targetId"].as_str().unwrap_or(""))}))
            });
            server.reply("Target.createTarget", json!({"targetId": "new"}));
            for m in [
                "Target.closeTarget",
                "Page.enable",
                "Page.startScreencast",
                "Page.stopScreencast",
                "Page.screencastFrameAck",
                "Page.reload",
                "Page.navigateToHistoryEntry",
                "Page.handleJavaScriptDialog",
                "Browser.setPermission",
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
            tokio::spawn(run_server(spec, server, fps, rx));
            url
        });
        config.push_str(&format!(
            "[[connection]]\nname = \"{}\"\ncdp_url = \"{url}\"\nauto_connect = {}\n",
            spec.name, spec.auto_connect
        ));
        if spec.agent {
            config.push_str(&format!(
                "agent_proxy = true\nagent_proxy_port = {}\nagent_proxy_secret = \"{AGENT_SECRET}\"\n",
                agent_port()
            ));
            println!(
                "[demo] Agent endpoint: http://127.0.0.1:{}/{AGENT_SECRET}",
                agent_port()
            );
        }
        config.push('\n');
    }
    let path = dir.join("config.toml");
    rb_store::write_private_file(&path, &config).unwrap();
    println!("[demo] config: {}", path.display());

    if std::env::var_os("RB_STATE").is_none_or(|v| v.is_empty()) {
        std::env::set_var("RB_STATE", dir.join("state.toml"));
    }
    if std::env::var("RB_DEMO_SEED").map_or(true, |v| v != "0") {
        std::fs::write(std::env::var("RB_STATE").unwrap(), seed_state()).unwrap();
    }
    println!(
        "[demo] state: {}",
        std::env::var("RB_STATE").unwrap_or_default()
    );

    let cmd_file = dir.join("cmd");
    std::fs::write(&cmd_file, "").unwrap();
    println!("[demo] command file: {}", cmd_file.display());
    rt.spawn(async move {
        let mut player = Player::default();
        let mut done = 0usize;
        loop {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let text = std::fs::read_to_string(&cmd_file).unwrap_or_default();
            let lines: Vec<&str> = text.lines().collect();
            for line in lines.iter().skip(done) {
                let w: Vec<&str> = line.split_whitespace().collect();
                if let ["agent", rest @ ..] = w.as_slice() {
                    player.run(rest).await;
                    continue;
                }
                let ctl = match w.as_slice() {
                    ["kill", s, rest @ ..] => Some((
                        *s,
                        Ctl::Kill(rest.first().and_then(|v| v.parse().ok()).unwrap_or(6.0)),
                    )),
                    ["dialog", s, t, k, rest @ ..] => Some((
                        *s,
                        Ctl::Dialog {
                            target: t.to_string(),
                            kind: k.to_string(),
                            text: rest.join(" "),
                        },
                    )),
                    ["popup", s, o, rest @ ..] => Some((
                        *s,
                        Ctl::Popup {
                            opener: o.to_string(),
                            title: rest.join(" "),
                        },
                    )),
                    _ => None,
                };
                match ctl.and_then(|(s, c)| senders.get(s).map(|tx| tx.send(c))) {
                    Some(_) => {}
                    None => println!("[demo] bad command: {line}"),
                }
            }
            done = lines.len();
        }
    });

    let startup = rb_app::startup::load(Some(path), None);
    rb_app::app::run(startup, rt.handle().clone());
    drop(rt);
}
