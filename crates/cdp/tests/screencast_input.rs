use std::time::Duration;

use base64::Engine;
use rb_cdp::testing::FakeCdpServer;
use rb_cdp::{
    CdpError, Client, KeyEvent, KeyEventKind, Modifiers, MouseButton, MouseEvent,
    ScreencastOptions, Session,
};
use serde_json::json;
use tokio::time::timeout;

const T: Duration = Duration::from_secs(5);

async fn setup() -> (FakeCdpServer, Client, Session) {
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
    (server, client, session)
}

fn frame_event(data: &[u8], id: i64) -> serde_json::Value {
    json!({
        "data": base64::engine::general_purpose::STANDARD.encode(data),
        "sessionId": id,
        "metadata": {
            "deviceWidth": 800.0, "deviceHeight": 600.0, "offsetTop": 12.0,
            "pageScaleFactor": 1.5, "scrollOffsetX": 3.0, "scrollOffsetY": 4.0,
            "timestamp": 1700000000.5
        }
    })
}

fn count(server: &FakeCdpServer, method: &str) -> usize {
    server
        .commands()
        .iter()
        .filter(|c| c.method == method)
        .count()
}

#[tokio::test]
async fn start_params_defaults_and_options() {
    let (server, _c, s) = setup().await;
    let sc = s
        .start_screencast(ScreencastOptions::default())
        .await
        .unwrap();
    let cmd = server
        .wait_for_command("Page.startScreencast", 1)
        .await
        .unwrap();
    assert_eq!(cmd.session_id.as_deref(), Some("S1"));
    assert_eq!(
        cmd.params,
        json!({"format": "jpeg", "quality": 60, "everyNthFrame": 1})
    );
    sc.stop().await.unwrap();

    let opts = ScreencastOptions {
        quality: 80,
        max_width: Some(1024),
        max_height: Some(768),
        every_nth_frame: 2,
    };
    let _sc = s.start_screencast(opts).await.unwrap();
    let cmd = server
        .wait_for_command("Page.startScreencast", 2)
        .await
        .unwrap();
    assert_eq!(
        cmd.params,
        json!({"format": "jpeg", "quality": 80, "maxWidth": 1024, "maxHeight": 768, "everyNthFrame": 2})
    );
}

#[tokio::test]
async fn frames_are_decoded_and_acked_only_on_request() {
    let (server, _c, s) = setup().await;
    let mut sc = s
        .start_screencast(ScreencastOptions::default())
        .await
        .unwrap();
    let jpeg: Vec<u8> = (0..=255u8).chain([0xFF, 0xD8, 0xFF]).collect();
    server.emit_in_session("S1", "Page.screencastFrame", frame_event(&jpeg, 7));
    // A frame for another session is ignored.
    server.emit_in_session("S2", "Page.screencastFrame", frame_event(b"x", 99));
    server.emit_in_session("S1", "Page.screencastFrame", frame_event(b"second", 8));

    let f = timeout(T, sc.next()).await.unwrap().unwrap();
    assert_eq!(f.data, jpeg);
    assert_eq!(f.frame_id, 7);
    assert_eq!(f.session_id(), "S1");
    assert_eq!(f.metadata.device_width, 800.0);
    assert_eq!(f.metadata.device_height, 600.0);
    assert_eq!(f.metadata.offset_top, 12.0);
    assert_eq!(f.metadata.page_scale_factor, 1.5);
    assert_eq!(f.metadata.scroll_offset_x, 3.0);
    assert_eq!(f.metadata.scroll_offset_y, 4.0);
    assert_eq!(f.metadata.timestamp, Some(1700000000.5));

    // Received but not acked: nothing sent yet.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(count(&server, "Page.screencastFrameAck"), 0);

    f.ack().await.unwrap();
    let ack = server
        .wait_for_command("Page.screencastFrameAck", 1)
        .await
        .unwrap();
    assert_eq!(ack.params, json!({"sessionId": 7}));
    assert_eq!(ack.session_id.as_deref(), Some("S1"));

    let f2 = timeout(T, sc.next()).await.unwrap().unwrap();
    assert_eq!(f2.data, b"second");
    assert_eq!(count(&server, "Page.screencastFrameAck"), 1);
    f2.ack().await.unwrap();
    assert_eq!(count(&server, "Page.screencastFrameAck"), 2);
}

#[tokio::test]
async fn only_one_screencast_per_session_and_stop() {
    let (server, _c, s) = setup().await;
    let mut sc = s
        .start_screencast(ScreencastOptions::default())
        .await
        .unwrap();
    let err = s
        .start_screencast(ScreencastOptions::default())
        .await
        .err()
        .unwrap();
    assert_eq!(err, CdpError::ScreencastActive);
    assert_eq!(count(&server, "Page.startScreencast"), 1);

    s.stop_screencast().await.unwrap();
    let stop = server
        .wait_for_command("Page.stopScreencast", 1)
        .await
        .unwrap();
    assert_eq!(stop.session_id.as_deref(), Some("S1"));
    assert!(
        timeout(T, sc.next()).await.unwrap().is_none(),
        "stream ends"
    );

    // Can restart after stop.
    let _sc2 = s
        .start_screencast(ScreencastOptions::default())
        .await
        .unwrap();
    assert_eq!(count(&server, "Page.startScreencast"), 2);
}

#[tokio::test]
async fn dropping_stream_frees_the_slot() {
    let (_server, _c, s) = setup().await;
    let sc = s
        .start_screencast(ScreencastOptions::default())
        .await
        .unwrap();
    drop(sc);
    s.start_screencast(ScreencastOptions::default())
        .await
        .unwrap();
}

#[tokio::test]
async fn failed_start_frees_the_slot() {
    let (server, _c, s) = setup().await;
    server.reply_error("Page.startScreencast", -32000, "nope");
    assert!(s
        .start_screencast(ScreencastOptions::default())
        .await
        .is_err());
    server.reply("Page.startScreencast", json!({}));
    s.start_screencast(ScreencastOptions::default())
        .await
        .unwrap();
}

#[tokio::test]
async fn mouse_events() {
    let (server, _c, s) = setup().await;
    s.dispatch_mouse(MouseEvent::moved(10.5, 20.0))
        .await
        .unwrap();
    s.dispatch_mouse(
        MouseEvent::pressed(1.0, 2.0, MouseButton::Left, 2)
            .with_modifiers(Modifiers::SHIFT | Modifiers::CTRL),
    )
    .await
    .unwrap();
    s.dispatch_mouse(MouseEvent::released(1.0, 2.0, MouseButton::Left, 2))
        .await
        .unwrap();
    s.dispatch_mouse(MouseEvent::wheel(5.0, 6.0, 0.0, -120.0))
        .await
        .unwrap();

    let cmds: Vec<_> = server
        .commands()
        .into_iter()
        .filter(|c| c.method == "Input.dispatchMouseEvent")
        .collect();
    assert_eq!(cmds.len(), 4);
    assert!(cmds.iter().all(|c| c.session_id.as_deref() == Some("S1")));
    assert_eq!(
        cmds[0].params,
        json!({"type": "mouseMoved", "x": 10.5, "y": 20.0, "button": "none", "buttons": 0, "clickCount": 0, "modifiers": 0})
    );
    assert_eq!(
        cmds[1].params,
        json!({"type": "mousePressed", "x": 1.0, "y": 2.0, "button": "left", "buttons": 1, "clickCount": 2, "modifiers": 10})
    );
    assert_eq!(cmds[2].params["type"], "mouseReleased");
    assert_eq!(cmds[2].params["buttons"], 0);
    assert_eq!(
        cmds[3].params,
        json!({"type": "mouseWheel", "x": 5.0, "y": 6.0, "button": "none", "buttons": 0, "clickCount": 0, "modifiers": 0, "deltaX": 0.0, "deltaY": -120.0})
    );
}

#[tokio::test]
async fn key_events_and_insert_text() {
    let (server, _c, s) = setup().await;
    s.dispatch_key(
        KeyEvent::new(KeyEventKind::KeyDown, "a")
            .with_code("KeyA")
            .with_text("a")
            .with_virtual_key_code(65)
            .with_modifiers(Modifiers::ALT),
    )
    .await
    .unwrap();
    s.dispatch_key(KeyEvent::new(KeyEventKind::KeyUp, "Enter"))
        .await
        .unwrap();
    s.dispatch_key(KeyEvent::new(KeyEventKind::RawKeyDown, "Tab"))
        .await
        .unwrap();
    s.dispatch_key(KeyEvent::new(KeyEventKind::Char, "é").with_text("é"))
        .await
        .unwrap();
    s.insert_text("héllo 世界").await.unwrap();

    let keys: Vec<_> = server
        .commands()
        .into_iter()
        .filter(|c| c.method == "Input.dispatchKeyEvent")
        .collect();
    assert_eq!(keys.len(), 4);
    assert_eq!(
        keys[0].params,
        json!({"type": "keyDown", "key": "a", "code": "KeyA", "text": "a", "windowsVirtualKeyCode": 65, "modifiers": 1})
    );
    assert_eq!(
        keys[1].params,
        json!({"type": "keyUp", "key": "Enter", "modifiers": 0})
    );
    assert_eq!(keys[2].params["type"], "rawKeyDown");
    assert_eq!(keys[3].params["type"], "char");
    assert!(keys.iter().all(|c| c.session_id.as_deref() == Some("S1")));

    let ins = server
        .wait_for_command("Input.insertText", 1)
        .await
        .unwrap();
    assert_eq!(ins.params, json!({"text": "héllo 世界"}));
    assert_eq!(ins.session_id.as_deref(), Some("S1"));
}

#[test]
fn modifier_bits() {
    assert_eq!(Modifiers::ALT.bits(), 1);
    assert_eq!(Modifiers::CTRL.bits(), 2);
    assert_eq!(Modifiers::META.bits(), 4);
    assert_eq!(Modifiers::SHIFT.bits(), 8);
    let m = Modifiers::META | Modifiers::SHIFT;
    assert!(m.contains(Modifiers::META) && !m.contains(Modifiers::ALT));
}

#[tokio::test]
async fn never_overrides_device_metrics() {
    let (server, _c, s) = setup().await;
    let sc = s
        .start_screencast(ScreencastOptions::default())
        .await
        .unwrap();
    s.dispatch_mouse(MouseEvent::moved(1.0, 1.0)).await.unwrap();
    sc.stop().await.unwrap();
    assert!(server
        .commands()
        .iter()
        .all(|c| !c.method.starts_with("Emulation.")));
}
