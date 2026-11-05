//! MirrorBackend::from_connection against the fake CDP server: Frames come
//! from the supervisor's stream, input follows the current Session across a
//! reconnect.

use std::time::Duration;

use base64::Engine;
use rb_app::mirror::{MirrorBackend, MirrorInput};
use rb_cdp::testing::FakeCdpServer;
use rb_cdp::{
    ActiveTab, Connection, ConnectionConfig, ConnectionState, MouseEvent, ScreencastOptions,
};
use serde_json::json;
use tokio::time::timeout;

const T: Duration = Duration::from_secs(10);

fn server_setup(s: &FakeCdpServer) {
    s.reply("Target.setDiscoverTargets", json!({}));
    s.reply(
        "Target.getTargets",
        json!({"targetInfos": [{"targetId": "T1", "type": "page", "title": "a", "url": "u"}]}),
    );
    s.reply("Target.attachToTarget", json!({"sessionId": "S1"}));
    for m in [
        "Page.startScreencast",
        "Page.stopScreencast",
        "Page.screencastFrameAck",
        "Input.dispatchMouseEvent",
    ] {
        s.reply(m, json!({}));
    }
}

#[tokio::test]
async fn frames_and_input_survive_a_reconnect() {
    let server = FakeCdpServer::start().await;
    server_setup(&server);
    let conn = Connection::start(ConnectionConfig::new(server.url(), None));
    let mut backend = MirrorBackend::from_connection(&conn, tokio::runtime::Handle::current())
        .expect("frames available");
    assert!(
        MirrorBackend::from_connection(&conn, tokio::runtime::Handle::current()).is_none(),
        "the Frame stream can be taken once"
    );
    let mut state = conn.state();
    timeout(T, state.wait_for(|s| *s == ConnectionState::Connected))
        .await
        .unwrap()
        .unwrap();
    conn.set_active_tab(Some(ActiveTab {
        target_id: "T1".into(),
        screencast: ScreencastOptions::default(),
    }));
    server
        .wait_for_command("Page.startScreencast", 1)
        .await
        .unwrap();

    let frame = json!({
        "data": base64::engine::general_purpose::STANDARD.encode(b"jpeg"),
        "sessionId": 3,
        "metadata": {"deviceWidth": 800.0, "deviceHeight": 600.0}
    });
    server.emit_in_session("S1", "Page.screencastFrame", frame.clone());
    let f = timeout(T, backend.frames.recv()).await.unwrap().unwrap();
    assert_eq!(f.frame_id, 3);
    f.ack().await.unwrap();
    server
        .wait_for_command("Page.screencastFrameAck", 1)
        .await
        .unwrap();

    backend
        .input
        .send(MirrorInput::Mouse(MouseEvent::moved(1.0, 2.0)))
        .unwrap();
    server
        .wait_for_command("Input.dispatchMouseEvent", 1)
        .await
        .unwrap();

    // Drop the WebSocket; the supervisor reconnects and re-attaches.
    server.drop_connections();
    timeout(T, state.wait_for(|s| *s != ConnectionState::Connected))
        .await
        .unwrap()
        .unwrap();
    timeout(T, state.wait_for(|s| *s == ConnectionState::Connected))
        .await
        .unwrap()
        .unwrap();
    server
        .wait_for_command("Page.startScreencast", 2)
        .await
        .unwrap();

    // Input goes to the new Session (retry briefly: the Session is published
    // just after the state).
    let mut got = 1;
    for _ in 0..50 {
        backend
            .input
            .send(MirrorInput::Mouse(MouseEvent::moved(5.0, 6.0)))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        got = server
            .commands()
            .iter()
            .filter(|c| c.method == "Input.dispatchMouseEvent")
            .count();
        if got >= 2 {
            break;
        }
    }
    assert!(got >= 2, "input did not reach the new Session");

    // Frames continue on the same receiver.
    server.emit_in_session("S1", "Page.screencastFrame", frame);
    let f = timeout(T, backend.frames.recv()).await.unwrap().unwrap();
    assert_eq!(f.frame_id, 3);
}
