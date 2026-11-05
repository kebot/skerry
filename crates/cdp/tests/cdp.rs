use std::time::Duration;

use rb_cdp::testing::FakeCdpServer;
use rb_cdp::{CdpError, Client, TargetEvent, TargetTracker, Token};
use serde_json::json;
use tokio::time::timeout;

const T: Duration = Duration::from_secs(5);

fn target(id: &str, title: &str, url: &str) -> serde_json::Value {
    json!({"targetId": id, "type": "page", "title": title, "url": url, "attached": false})
}

#[tokio::test]
async fn concurrent_commands_match_responses() {
    let server = FakeCdpServer::start().await;
    server.reply("A.one", json!({"v": 1}));
    server.reply("A.two", json!({"v": 2}));
    server.reply("A.three", json!({"v": 3}));
    let client = Client::connect(&server.url(), None).await.unwrap();
    let (a, b, c) = tokio::join!(
        client.send("A.one", None, None),
        client.send("A.two", Some(json!({"x": 1})), None),
        client.send("A.three", None, None),
    );
    assert_eq!(a.unwrap()["v"], 1);
    assert_eq!(b.unwrap()["v"], 2);
    assert_eq!(c.unwrap()["v"], 3);
    let cmds = server.commands();
    assert_eq!(cmds.len(), 3);
    let ids: std::collections::HashSet<_> = cmds.iter().map(|c| c.id).collect();
    assert_eq!(ids.len(), 3, "ids are unique");
    assert_eq!(
        cmds.iter().find(|c| c.method == "A.two").unwrap().params,
        json!({"x": 1})
    );
}

#[tokio::test]
async fn protocol_errors_are_mapped() {
    let server = FakeCdpServer::start().await;
    server.reply_error("Page.navigate", -32000, "Cannot navigate");
    let client = Client::connect(&server.url(), None).await.unwrap();
    let err = client.send("Page.navigate", None, None).await.unwrap_err();
    assert_eq!(
        err,
        CdpError::Protocol {
            code: -32000,
            message: "Cannot navigate".into(),
            data: None
        }
    );
    assert!(err.is_protocol());
    // Unknown method: -32601, connection still usable.
    let err = client.send("Nope.nope", None, None).await.unwrap_err();
    assert!(matches!(err, CdpError::Protocol { code: -32601, .. }));
    assert!(!client.is_closed());
}

#[tokio::test]
async fn events_are_delivered() {
    let server = FakeCdpServer::start().await;
    let client = Client::connect(&server.url(), None).await.unwrap();
    let mut events = client.events();
    server.wait_for_connections(1).await;
    server.emit("Page.loadEventFired", json!({"timestamp": 1.5}));
    let ev = timeout(T, events.recv()).await.unwrap().unwrap();
    assert_eq!(ev.method, "Page.loadEventFired");
    assert_eq!(ev.params["timestamp"], 1.5);
    assert_eq!(ev.session_id, None);
}

#[tokio::test]
async fn session_id_routing() {
    let server = FakeCdpServer::start().await;
    server.reply("Target.attachToTarget", json!({"sessionId": "S1"}));
    server.reply("Page.enable", json!({}));
    let client = Client::connect(&server.url(), None).await.unwrap();
    let session = client.attach_to_target("T1").await.unwrap();
    assert_eq!(session.id(), "S1");
    let attach = &server.commands()[0];
    assert_eq!(attach.params, json!({"targetId": "T1", "flatten": true}));
    assert_eq!(attach.session_id, None);

    session.send("Page.enable", None).await.unwrap();
    let cmds = server.commands();
    assert_eq!(cmds[1].session_id.as_deref(), Some("S1"));

    // Session events only see their own session.
    let mut sev = session.events();
    server.emit_in_session("S2", "Page.other", json!({}));
    server.emit("Browser.noise", json!({}));
    server.emit_in_session("S1", "Page.mine", json!({"a": 1}));
    let ev = timeout(T, sev.recv()).await.unwrap().unwrap();
    assert_eq!(ev.method, "Page.mine");
    assert_eq!(ev.session_id.as_deref(), Some("S1"));
}

#[tokio::test]
async fn get_targets_and_typed_events() {
    let server = FakeCdpServer::start().await;
    server.reply(
        "Target.getTargets",
        json!({"targetInfos": [target("A", "t", "u"), {"targetId": "W", "type": "service_worker"}]}),
    );
    let client = Client::connect(&server.url(), None).await.unwrap();
    let infos = client.get_targets().await.unwrap();
    assert_eq!(infos.len(), 2);
    assert!(infos[0].is_page() && !infos[1].is_page());

    let mut events = client.events();
    server.wait_for_connections(1).await;
    server.emit(
        "Target.attachedToTarget",
        json!({"sessionId": "S", "targetInfo": target("A", "t", "u"), "waitingForDebugger": false}),
    );
    server.emit(
        "Target.detachedFromTarget",
        json!({"sessionId": "S", "targetId": "A"}),
    );
    let a = TargetEvent::from_event(&events.recv().await.unwrap()).unwrap();
    assert!(matches!(a, TargetEvent::Attached { ref session_id, .. } if session_id == "S"));
    let d = TargetEvent::from_event(&events.recv().await.unwrap()).unwrap();
    assert_eq!(
        d,
        TargetEvent::Detached {
            session_id: "S".into(),
            target_id: Some("A".into())
        }
    );
}

async fn wait_pages(tracker: &TargetTracker, pred: impl Fn(&[rb_cdp::TargetInfo]) -> bool) {
    let mut w = tracker.watch();
    timeout(T, async {
        loop {
            if pred(&w.borrow_and_update()) {
                return;
            }
            w.changed().await.unwrap();
        }
    })
    .await
    .expect("tracker condition");
}

#[tokio::test]
async fn target_tracker_follows_events() {
    let server = FakeCdpServer::start().await;
    server.reply("Target.setDiscoverTargets", json!({}));
    // Existing targets are replayed after enabling discovery.
    server.emit_after(
        "Target.setDiscoverTargets",
        "Target.targetCreated",
        json!({"targetInfo": target("A", "Old", "https://a/")}),
    );
    server.emit_after(
        "Target.setDiscoverTargets",
        "Target.targetCreated",
        json!({"targetInfo": {"targetId": "SW", "type": "service_worker", "title": "", "url": ""}}),
    );
    let client = Client::connect(&server.url(), None).await.unwrap();
    let tracker = TargetTracker::start(&client).await.unwrap();
    wait_pages(&tracker, |p| p.len() == 1).await;
    assert_eq!(tracker.pages()[0].title, "Old");

    server.emit(
        "Target.targetCreated",
        json!({"targetInfo": target("B", "B", "https://b/")}),
    );
    wait_pages(&tracker, |p| p.len() == 2).await;

    server.emit(
        "Target.targetInfoChanged",
        json!({"targetInfo": target("A", "New title", "https://a/2")}),
    );
    wait_pages(&tracker, |p| p[0].title == "New title").await;
    assert_eq!(tracker.pages()[0].url, "https://a/2");

    server.emit("Target.targetDestroyed", json!({"targetId": "A"}));
    wait_pages(&tracker, |p| p.len() == 1).await;
    let pages = tracker.pages();
    assert_eq!(pages[0].target_id, "B");
    assert_eq!(server.commands()[0].params, json!({"discover": true}));
}

#[tokio::test]
async fn bearer_header_is_sent_and_enforced() {
    let server = FakeCdpServer::start().await;
    server.require_bearer("s3cret");
    // Missing token is rejected.
    assert!(Client::connect(&server.url(), None).await.is_err());
    // Wrong token is rejected.
    assert!(Client::connect(&server.url(), Some(Token::new("bad")))
        .await
        .is_err());
    let c = Client::connect(&server.url(), Some(Token::new("s3cret")))
        .await
        .unwrap();
    assert!(!c.is_closed());
    let auths = server.authorizations();
    assert_eq!(
        auths,
        vec![
            None,
            Some("Bearer bad".into()),
            Some("Bearer s3cret".into())
        ]
    );
}

#[test]
fn token_debug_is_redacted() {
    let t = Token::new("s3cret");
    assert!(!format!("{t:?}").contains("s3cret"));
}

#[tokio::test]
async fn connection_drop_fails_pending_commands() {
    let server = FakeCdpServer::start().await;
    server.never_reply("Slow.call");
    let client = Client::connect(&server.url(), None).await.unwrap();
    let mut events = client.events();
    let c2 = client.clone();
    let pending = tokio::spawn(async move { c2.send("Slow.call", None, None).await });
    server.wait_for_command("Slow.call", 1).await.unwrap();
    server.drop_connections();
    let res = timeout(T, pending).await.unwrap().unwrap();
    assert!(
        matches!(res, Err(CdpError::Closed) | Err(CdpError::Transport(_))),
        "{res:?}"
    );
    timeout(T, client.closed()).await.unwrap();
    assert!(client.is_closed());
    assert!(events.recv().await.is_none());
    // New commands fail immediately.
    assert!(client.send("X.y", None, None).await.is_err());
}

#[tokio::test]
async fn clean_close() {
    let server = FakeCdpServer::start().await;
    let client = Client::connect(&server.url(), None).await.unwrap();
    server.wait_for_connections(1).await;
    client.close().await;
    assert!(client.is_closed());
    assert_eq!(client.send("A.b", None, None).await, Err(CdpError::Closed));
}
