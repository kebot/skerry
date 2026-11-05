use std::time::{Duration, Instant};

use rb_cdp::testing::FakeCdpServer;
use rb_cdp::{AgentEvent, AgentProxy, AgentProxyConfig, Client, Token, ACTING_WINDOW};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::broadcast;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

const T: Duration = Duration::from_secs(5);

async fn setup(secret: Option<&str>) -> (FakeCdpServer, AgentProxy) {
    let server = FakeCdpServer::start().await;
    server.require_bearer("s3cret-token");
    let mut cfg = AgentProxyConfig::new(server.url(), Some(Token::new("s3cret-token")));
    cfg.path_secret = secret.map(str::to_string);
    let proxy = AgentProxy::start(cfg).await.unwrap();
    (server, proxy)
}

/// Raw HTTP/1.1 GET; returns (status, body).
async fn http(
    addr: std::net::SocketAddr,
    method: &str,
    path: &str,
    extra: &[&str],
) -> (u16, String) {
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\n");
    if !extra
        .iter()
        .any(|h| h.to_ascii_lowercase().starts_with("host:"))
    {
        req.push_str(&format!("Host: {addr}\r\n"));
    }
    for h in extra {
        req.push_str(h);
        req.push_str("\r\n");
    }
    req.push_str("Connection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = String::new();
    timeout(T, s.read_to_string(&mut out))
        .await
        .unwrap()
        .unwrap();
    let status = out[9..12].parse().unwrap();
    let body = out.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (status, body)
}

async fn next_event(rx: &mut broadcast::Receiver<AgentEvent>) -> AgentEvent {
    timeout(T, rx.recv()).await.expect("event").unwrap()
}

fn target(id: &str) -> Value {
    json!({"targetId": id, "type": "page", "title": "T", "url": "https://e.com/", "attached": false})
}

#[tokio::test]
async fn json_version_is_rewritten_and_token_injected() {
    let (server, proxy) = setup(None).await;
    server.reply(
        "Browser.getVersion",
        json!({"product": "Chrome/130", "protocolVersion": "1.3", "userAgent": "UA", "jsVersion": "13", "revision": "r1"}),
    );
    let (st, body) = http(proxy.local_addr(), "GET", "/json/version", &[]).await;
    assert_eq!(st, 200);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["Browser"], "Chrome/130");
    assert_eq!(
        v["webSocketDebuggerUrl"],
        format!("ws://{}/devtools/browser/rb", proxy.local_addr())
    );
    assert_eq!(
        server.authorizations(),
        vec![Some("Bearer s3cret-token".to_string())]
    );
    assert!(!body.contains("s3cret-token"));
}

#[tokio::test]
async fn json_list_new_activate_close() {
    let (server, proxy) = setup(None).await;
    server.reply(
        "Target.getTargets",
        json!({"targetInfos": [target("A"), {"targetId": "B", "type": "browser", "title": "", "url": ""}]}),
    );
    server.reply("Target.createTarget", json!({"targetId": "N"}));
    server.reply("Target.getTargetInfo", json!({"targetInfo": target("N")}));
    server.reply("Target.activateTarget", json!({}));
    server.reply("Target.closeTarget", json!({"success": true}));
    let mut ev = proxy.events();

    let (st, body) = http(proxy.local_addr(), "GET", "/json/list", &[]).await;
    assert_eq!(st, 200);
    let l: Vec<Value> = serde_json::from_str(&body).unwrap();
    assert_eq!(l.len(), 1, "browser target filtered");
    assert_eq!(l[0]["id"], "A");
    assert_eq!(
        l[0]["webSocketDebuggerUrl"],
        format!("ws://{}/devtools/page/A", proxy.local_addr())
    );
    let (_, body) = http(proxy.local_addr(), "GET", "/json", &[]).await;
    assert_eq!(serde_json::from_str::<Vec<Value>>(&body).unwrap().len(), 1);

    let (st, body) = http(proxy.local_addr(), "PUT", "/json/new?https://e.com/", &[]).await;
    assert_eq!(st, 200);
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["id"], "N");
    let cmd = server
        .wait_for_command("Target.createTarget", 1)
        .await
        .unwrap();
    assert_eq!(cmd.params["url"], "https://e.com/");
    assert!(
        matches!(next_event(&mut ev).await, AgentEvent::Acted { target_id, .. } if target_id == "N")
    );

    assert_eq!(
        http(proxy.local_addr(), "GET", "/json/activate/A", &[])
            .await
            .0,
        200
    );
    assert_eq!(
        http(proxy.local_addr(), "GET", "/json/close/A", &[])
            .await
            .0,
        200
    );
    assert_eq!(http(proxy.local_addr(), "GET", "/nope", &[]).await.0, 404);
}

#[tokio::test]
async fn upstream_down_is_502() {
    let (server, proxy) = setup(None).await;
    server.reject_all_handshakes(Some(503));
    assert_eq!(
        http(proxy.local_addr(), "GET", "/json/version", &[])
            .await
            .0,
        502
    );
}

#[tokio::test]
async fn browser_ws_relays_with_ids_preserved_and_no_token_needed() {
    let (server, proxy) = setup(None).await;
    server.reply("Browser.getVersion", json!({"product": "P"}));
    // The Agent sends no token at all.
    let agent = Client::connect(&proxy.ws_endpoint(), None).await.unwrap();
    let mut events = agent.events();
    let r = agent.send("Browser.getVersion", None, None).await.unwrap();
    assert_eq!(r["product"], "P");
    let got = server.commands();
    assert_eq!(got[0].id, 1, "id is relayed untouched");
    assert_eq!(
        server.authorizations(),
        vec![Some("Bearer s3cret-token".to_string())]
    );
    server.emit("Target.targetCreated", json!({"x": 1}));
    assert_eq!(events.recv().await.unwrap().method, "Target.targetCreated");
}

#[tokio::test]
async fn page_ws_attaches_and_translates_session() {
    let (server, proxy) = setup(None).await;
    server.reply("Target.attachToTarget", json!({"sessionId": "PS"}));
    server.reply("Page.navigate", json!({"frameId": "F"}));
    let mut ev = proxy.events();
    let url = format!("ws://{}/devtools/page/T1", proxy.local_addr());
    let agent = Client::connect(&url, None).await.unwrap();
    let mut events = agent.events();
    let r = agent
        .send("Page.navigate", Some(json!({"url": "https://e.com"})), None)
        .await
        .unwrap();
    assert_eq!(r["frameId"], "F");
    let nav = server.wait_for_command("Page.navigate", 1).await.unwrap();
    assert_eq!(nav.session_id.as_deref(), Some("PS"));
    let att = server
        .wait_for_command("Target.attachToTarget", 1)
        .await
        .unwrap();
    assert_eq!(att.params["targetId"], "T1");
    assert_eq!(att.params["flatten"], true);

    server.emit_in_session("PS", "Page.loadEventFired", json!({"timestamp": 1.0}));
    server.emit_in_session("OTHER", "Page.frameNavigated", json!({}));
    let e = events.recv().await.unwrap();
    assert_eq!(e.method, "Page.loadEventFired");
    assert_eq!(e.session_id, None, "session id stripped for the Agent");

    assert!(matches!(
        next_event(&mut ev).await,
        AgentEvent::Attached { .. }
    ));
    match next_event(&mut ev).await {
        AgentEvent::Acted {
            target_id, method, ..
        } => {
            assert_eq!(target_id, "T1");
            assert_eq!(method, "Page.navigate");
        }
        e => panic!("{e:?}"),
    }
}

#[tokio::test]
async fn page_ws_unknown_target_is_404() {
    let (server, proxy) = setup(None).await;
    server.reply_error("Target.attachToTarget", -32602, "No target");
    let url = format!("ws://{}/devtools/page/NOPE", proxy.local_addr());
    assert!(Client::connect(&url, None).await.is_err());
}

#[tokio::test]
async fn origin_is_rejected_http_and_ws() {
    let (server, proxy) = setup(None).await;
    server.reply("Browser.getVersion", json!({}));
    let (st, _) = http(
        proxy.local_addr(),
        "GET",
        "/json/version",
        &["Origin: https://evil.example"],
    )
    .await;
    assert_eq!(st, 403);

    let mut req = proxy.ws_endpoint().into_client_request().unwrap();
    req.headers_mut()
        .insert("origin", "https://evil.example".parse().unwrap());
    let err = tokio_tungstenite::connect_async(req).await.unwrap_err();
    match err {
        tokio_tungstenite::tungstenite::Error::Http(r) => assert_eq!(r.status().as_u16(), 403),
        e => panic!("{e:?}"),
    }
    assert_eq!(server.accepted_connections(), 0, "upstream never touched");
}

#[tokio::test]
async fn host_header_is_validated() {
    let (server, proxy) = setup(None).await;
    server.reply("Browser.getVersion", json!({}));
    let a = proxy.local_addr();
    assert_eq!(
        http(a, "GET", "/json/version", &["Host: evil.example"])
            .await
            .0,
        403
    );
    assert_eq!(
        http(a, "GET", "/json/version", &["Host: evil.example:80"])
            .await
            .0,
        403
    );
    assert_eq!(
        http(a, "GET", "/json/version", &["Host: 127.0.0.1.evil.com"])
            .await
            .0,
        403
    );
    let ok = format!("Host: localhost:{}", a.port());
    assert_eq!(http(a, "GET", "/json/version", &[&ok]).await.0, 200);
    assert_eq!(server.accepted_connections(), 1);
}

#[tokio::test]
async fn path_secret_is_required() {
    let (server, proxy) = setup(Some("abc123")).await;
    server.reply("Browser.getVersion", json!({"product": "P"}));
    let a = proxy.local_addr();
    assert_eq!(proxy.endpoint(), format!("http://{a}/abc123"));
    assert_eq!(http(a, "GET", "/json/version", &[]).await.0, 404);
    assert_eq!(http(a, "GET", "/wrong/json/version", &[]).await.0, 404);
    let (st, body) = http(a, "GET", "/abc123/json/version", &[]).await;
    assert_eq!(st, 200);
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["webSocketDebuggerUrl"],
        format!("ws://{a}/abc123/devtools/browser/rb")
    );
    assert!(
        Client::connect(&format!("ws://{a}/devtools/browser/rb"), None)
            .await
            .is_err()
    );
    let c = Client::connect(&proxy.ws_endpoint(), None).await.unwrap();
    assert_eq!(
        c.send("Browser.getVersion", None, None).await.unwrap()["product"],
        "P"
    );
}

#[tokio::test]
async fn binds_loopback_only() {
    let (_server, proxy) = setup(None).await;
    assert!(proxy.local_addr().ip().is_loopback());
    assert_eq!(proxy.local_addr().ip().to_string(), "127.0.0.1");
    assert!(proxy.endpoint().starts_with("http://127.0.0.1:"));
}

#[tokio::test]
async fn activity_for_playwright_like_sequence() {
    let (server, proxy) = setup(None).await;
    server.reply("Target.setAutoAttach", json!({}));
    server.emit_after(
        "Target.setAutoAttach",
        "Target.attachedToTarget",
        json!({"sessionId": "S9", "targetInfo": target("T9"), "waitingForDebugger": false}),
    );
    server.reply("Target.attachToTarget", json!({"sessionId": "S1"}));
    server.reply("Page.navigate", json!({"frameId": "F"}));
    server.reply("Emulation.setDeviceMetricsOverride", json!({}));
    server.reply("Target.activateTarget", json!({}));
    server.reply("Target.createTarget", json!({"targetId": "NEW"}));
    let mut ev = proxy.events();
    let mut state = proxy.state();
    assert_eq!(state.borrow().clients, 0);

    let agent = Client::connect(&proxy.ws_endpoint(), None).await.unwrap();
    let AgentEvent::Attached { client_id } = next_event(&mut ev).await else {
        panic!()
    };
    assert_eq!(state.wait_for(|s| s.clients == 1).await.unwrap().clients, 1);

    // auto-attach announces T9 with session S9
    agent
        .send(
            "Target.setAutoAttach",
            Some(json!({"autoAttach": true, "flatten": true, "waitForDebuggerOnStart": false})),
            None,
        )
        .await
        .unwrap();
    // explicit attach T1 -> S1
    let s = agent.attach_to_target("T1").await.unwrap();
    assert_eq!(s.id(), "S1");
    s.send("Page.navigate", Some(json!({"url": "https://e.com"})))
        .await
        .unwrap();
    match next_event(&mut ev).await {
        AgentEvent::Acted {
            target_id,
            method,
            at,
        } => {
            assert_eq!(
                (target_id.as_str(), method.as_str()),
                ("T1", "Page.navigate")
            );
            assert!(at.elapsed() < T);
        }
        e => panic!("{e:?}"),
    }
    // session command via auto-attached session, sent raw
    agent
        .send(
            "Emulation.setDeviceMetricsOverride",
            Some(json!({"width": 1})),
            Some("S9"),
        )
        .await
        .unwrap();
    match next_event(&mut ev).await {
        AgentEvent::Acted {
            target_id, method, ..
        } => {
            assert_eq!(
                (target_id.as_str(), method.as_str()),
                ("T9", "Emulation.setDeviceMetricsOverride")
            );
        }
        e => panic!("{e:?}"),
    }
    agent
        .send(
            "Target.activateTarget",
            Some(json!({"targetId": "T2"})),
            None,
        )
        .await
        .unwrap();
    assert!(
        matches!(next_event(&mut ev).await, AgentEvent::Acted { target_id, .. } if target_id == "T2")
    );
    agent
        .send(
            "Target.createTarget",
            Some(json!({"url": "about:blank"})),
            None,
        )
        .await
        .unwrap();
    assert!(
        matches!(next_event(&mut ev).await, AgentEvent::Acted { target_id, method, .. } if target_id == "NEW" && method == "Target.createTarget")
    );
    // unmapped session: no activity
    let _ = agent.send("Page.enable", None, Some("UNKNOWN")).await;

    let snap = state.borrow().clone();
    assert!(snap.is_acting("T1", ACTING_WINDOW, Instant::now()));
    assert!(!snap.is_acting(
        "T1",
        ACTING_WINDOW,
        Instant::now() + Duration::from_secs(10)
    ));
    assert!(snap.touched("T1") && !snap.touched("UNKNOWN") && !snap.touched("T3"));
    assert!(snap.targets["T9"].emulation_override);
    assert!(!snap.targets["T1"].emulation_override);

    agent.close().await;
    assert!(
        matches!(next_event(&mut ev).await, AgentEvent::Detached { client_id: c } if c == client_id)
    );
    assert_eq!(state.wait_for(|s| s.clients == 0).await.unwrap().clients, 0);
    assert!(state.borrow().touched("T1"), "touched history is kept");
}

#[tokio::test]
async fn multiple_agents_have_own_upstreams_and_cleanup() {
    let (server, proxy) = setup(None).await;
    server.reply("Browser.getVersion", json!({"product": "P"}));
    let mut ev = proxy.events();
    let a = Client::connect(&proxy.ws_endpoint(), None).await.unwrap();
    let b = Client::connect(&proxy.ws_endpoint(), None).await.unwrap();
    assert!(server.wait_for_connections(2).await);
    let AgentEvent::Attached { client_id: ia } = next_event(&mut ev).await else {
        panic!()
    };
    let AgentEvent::Attached { client_id: ib } = next_event(&mut ev).await else {
        panic!()
    };
    assert_ne!(ia, ib);
    assert_eq!(proxy.state().borrow().clients, 2);
    // Responses are not crossed between agents even with identical ids.
    let (ra, rb) = tokio::join!(
        a.send("Browser.getVersion", None, None),
        b.send("Browser.getVersion", None, None)
    );
    assert_eq!(ra.unwrap()["product"], "P");
    assert_eq!(rb.unwrap()["product"], "P");
    a.close().await;
    assert!(matches!(
        next_event(&mut ev).await,
        AgentEvent::Detached { .. }
    ));
    // b still works; a's upstream connection is gone.
    assert_eq!(
        b.send("Browser.getVersion", None, None).await.unwrap()["product"],
        "P"
    );
    let mut st = proxy.state();
    st.wait_for(|s| s.clients == 1).await.unwrap();
}

#[tokio::test]
async fn upstream_drop_closes_agent_connection() {
    let (server, proxy) = setup(None).await;
    let mut ev = proxy.events();
    let agent = Client::connect(&proxy.ws_endpoint(), None).await.unwrap();
    assert!(server.wait_for_connections(1).await);
    assert!(matches!(
        next_event(&mut ev).await,
        AgentEvent::Attached { .. }
    ));
    server.drop_connections();
    timeout(T, agent.closed())
        .await
        .expect("agent connection closed");
    assert!(matches!(
        next_event(&mut ev).await,
        AgentEvent::Detached { .. }
    ));
}

#[tokio::test]
async fn stop_closes_agents_and_listener() {
    let (server, proxy) = setup(None).await;
    let addr = proxy.local_addr();
    let agent = Client::connect(&proxy.ws_endpoint(), None).await.unwrap();
    assert!(server.wait_for_connections(1).await);
    proxy.stop();
    timeout(T, agent.closed())
        .await
        .expect("agent closed on stop");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(tokio::net::TcpStream::connect(addr).await.is_err());
}

#[tokio::test]
async fn debug_never_prints_secrets() {
    let mut cfg = AgentProxyConfig::new("ws://x/", Some(Token::new("tok-123")));
    cfg.path_secret = Some("sec-456".into());
    let d = format!("{cfg:?}");
    assert!(!d.contains("tok-123") && !d.contains("sec-456"));
    let s = rb_cdp::random_path_secret();
    assert_eq!(s.len(), 32);
    assert_ne!(s, rb_cdp::random_path_secret());
}
