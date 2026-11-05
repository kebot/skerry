//! MirrorBackend against the fake CDP server: Frames flow in, ack works,
//! input flows out in order.

use std::time::Duration;

use base64::Engine;
use rb_app::mirror::{MirrorBackend, MirrorInput};
use rb_cdp::testing::FakeCdpServer;
use rb_cdp::{Client, KeyEvent, KeyEventKind, MouseEvent, ScreencastOptions};
use serde_json::json;
use tokio::time::timeout;

const T: Duration = Duration::from_secs(5);

async fn setup() -> (FakeCdpServer, Client, MirrorBackend) {
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
    (server, client, backend)
}

fn frame_event(id: i64) -> serde_json::Value {
    json!({
        "data": base64::engine::general_purpose::STANDARD.encode(b"jpeg"),
        "sessionId": id,
        "metadata": {"deviceWidth": 800.0, "deviceHeight": 600.0}
    })
}

#[tokio::test]
async fn frames_arrive_and_ack_reaches_the_server() {
    let (server, _client, mut b) = setup().await;
    server.emit_in_session("S1", "Page.screencastFrame", frame_event(7));
    let f = timeout(T, b.frames.recv()).await.unwrap().unwrap();
    assert_eq!(f.frame_id, 7);
    assert_eq!(f.metadata.device_width, 800.0);
    // No ack yet: the Mirror decides when.
    assert!(server
        .commands()
        .iter()
        .all(|c| c.method != "Page.screencastFrameAck"));
    f.ack().await.unwrap();
    let ack = server
        .wait_for_command("Page.screencastFrameAck", 1)
        .await
        .unwrap();
    assert_eq!(ack.params, json!({"sessionId": 7}));
}

#[tokio::test]
async fn input_is_dispatched_in_order() {
    let (server, _client, b) = setup().await;
    b.input
        .send(MirrorInput::Mouse(MouseEvent::moved(10.0, 20.0)))
        .unwrap();
    b.input
        .send(MirrorInput::Key(
            KeyEvent::new(KeyEventKind::KeyDown, "a").with_text("a"),
        ))
        .unwrap();
    b.input.send(MirrorInput::InsertText("é".into())).unwrap();
    server
        .wait_for_command("Input.insertText", 1)
        .await
        .unwrap();
    let methods: Vec<_> = server
        .commands()
        .into_iter()
        .map(|c| c.method)
        .filter(|m| m.starts_with("Input."))
        .collect();
    assert_eq!(
        methods,
        [
            "Input.dispatchMouseEvent",
            "Input.dispatchKeyEvent",
            "Input.insertText"
        ]
    );
    let m = &server
        .commands()
        .into_iter()
        .find(|c| c.method == "Input.dispatchMouseEvent")
        .unwrap();
    assert_eq!(
        (m.params["x"].clone(), m.params["y"].clone()),
        (json!(10.0), json!(20.0))
    );
}

#[tokio::test]
async fn dropping_the_receiver_stops_the_screencast() {
    let (server, _client, b) = setup().await;
    drop(b.frames);
    server.emit_in_session("S1", "Page.screencastFrame", frame_event(1));
    assert!(
        timeout(T, server.wait_for_command("Page.stopScreencast", 1))
            .await
            .unwrap()
            .is_some()
    );
}
