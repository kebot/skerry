use std::time::Duration;

use rb_cdp::testing::FakeCdpServer;
use rb_cdp::{
    ActiveTab, BackoffConfig, Connection, ConnectionConfig, ConnectionEvent, ConnectionState,
    FailureReason, ScreencastOptions, Token,
};
use serde_json::json;
use tokio::sync::watch;

const T: Duration = Duration::from_secs(5);
const INITIAL: Duration = Duration::from_millis(20);

fn page(id: &str) -> serde_json::Value {
    json!({"targetInfo": {"targetId": id, "type": "page", "title": id, "url": "https://x/"}})
}

async fn server() -> FakeCdpServer {
    let s = FakeCdpServer::start().await;
    s.reply("Target.setDiscoverTargets", json!({}));
    s.reply(
        "Target.getTargets",
        json!({"targetInfos": [page("A")["targetInfo"].clone(), page("B")["targetInfo"].clone()]}),
    );
    s.reply("Target.attachToTarget", json!({"sessionId": "S1"}));
    s.reply("Page.startScreencast", json!({}));
    s.reply("Page.stopScreencast", json!({}));
    s
}

fn config(s: &FakeCdpServer) -> ConnectionConfig {
    let mut c = ConnectionConfig::new(s.url(), Some(Token::new("tok")));
    c.backoff = BackoffConfig {
        initial: INITIAL,
        max: Duration::from_millis(80),
        jitter: 0.0,
        stable_after: Duration::from_millis(250),
    };
    c
}

async fn until(
    rx: &mut watch::Receiver<ConnectionState>,
    f: impl Fn(&ConnectionState) -> bool,
) -> ConnectionState {
    tokio::time::timeout(T, rx.wait_for(|s| f(s)))
        .await
        .expect("timed out waiting for state")
        .unwrap()
        .clone()
}

fn is_connected(s: &ConnectionState) -> bool {
    *s == ConnectionState::Connected
}

fn count(s: &FakeCdpServer, m: &str) -> usize {
    s.commands().iter().filter(|c| c.method == m).count()
}

#[tokio::test]
async fn connects_and_publishes_pages() {
    let s = server().await;
    let conn = Connection::start(config(&s));
    let mut st = conn.state();
    until(&mut st, is_connected).await;
    assert_eq!(conn.pages().borrow().len(), 2);
    assert!(conn.client().is_some());
    assert_eq!(
        s.authorizations()[0].as_deref(),
        Some("Bearer tok"),
        "token sent"
    );
    conn.close().await;
    assert_eq!(*conn.state().borrow(), ConnectionState::Closed);
}

#[tokio::test]
async fn recovers_after_drop_and_rebuilds_state() {
    let s = server().await;
    let conn = Connection::start(config(&s));
    let mut events = conn.events();
    let mut frames = conn.take_frames().unwrap();
    let mut st = conn.state();
    until(&mut st, is_connected).await;
    conn.set_active_tab(Some(ActiveTab {
        target_id: "B".into(),
        screencast: ScreencastOptions::default(),
    }));
    s.wait_for_command("Page.startScreencast", 1).await.unwrap();
    let old = conn.client().unwrap();

    // Pending command during the drop fails fast.
    s.never_reply("Browser.getVersion");
    let pending = tokio::spawn({
        let old = old.clone();
        async move { old.send("Browser.getVersion", None, None).await }
    });
    s.wait_for_command("Browser.getVersion", 1).await.unwrap();

    s.drop_connections();
    assert!(pending.await.unwrap().is_err());
    until(&mut st, |s| {
        matches!(s, ConnectionState::Reconnecting { attempt: 1, .. })
    })
    .await;
    until(&mut st, is_connected).await;

    // Rebuilt: discover re-enabled, screencast restarted on a fresh session.
    assert_eq!(count(&s, "Target.setDiscoverTargets"), 2);
    assert_eq!(count(&s, "Target.getTargets"), 2);
    s.wait_for_command("Page.startScreencast", 2).await.unwrap();
    assert_eq!(s.accepted_connections(), 2);
    let attaches: Vec<_> = s
        .commands()
        .into_iter()
        .filter(|c| c.method == "Target.attachToTarget")
        .collect();
    assert_eq!(attaches.len(), 2);
    assert_eq!(attaches[1].params["targetId"], "B");
    assert!(old.is_closed());
    let mut sess = conn.active_session();
    let fresh = tokio::time::timeout(T, sess.wait_for(|s| s.is_some()))
        .await
        .unwrap()
        .unwrap()
        .clone()
        .unwrap();
    assert!(!fresh.client().is_closed());

    // The single frame stream continues across the reconnect.
    s.emit_in_session(
        "S1",
        "Page.screencastFrame",
        json!({"data": "AAEC", "sessionId": 7, "metadata": {}}),
    );
    let f = tokio::time::timeout(T, frames.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(f.data, vec![0, 1, 2]);

    let mut saw = vec![];
    while let Ok(e) = events.try_recv() {
        saw.push(e);
    }
    assert!(saw.contains(&ConnectionEvent::Disconnected { close_code: None }));
    assert!(saw.contains(&ConnectionEvent::Connected));
}

#[tokio::test]
async fn backoff_schedule_and_reset() {
    let s = server().await;
    let conn = Connection::start(config(&s));
    let mut st = conn.state();
    until(&mut st, is_connected).await;

    s.refuse_connections(true);
    s.drop_connections();
    let ConnectionState::Reconnecting {
        attempt,
        next_retry_in,
    } = until(&mut st, |s| {
        matches!(s, ConnectionState::Reconnecting { .. })
    })
    .await
    else {
        unreachable!()
    };
    assert_eq!((attempt, next_retry_in), (1, INITIAL));
    for (n, ms) in [(2, 40), (3, 80), (4, 80)] {
        let got = until(
            &mut st,
            |s| matches!(s, ConnectionState::Reconnecting { attempt, .. } if *attempt == n),
        )
        .await;
        assert_eq!(
            got,
            ConnectionState::Reconnecting {
                attempt: n,
                next_retry_in: Duration::from_millis(ms)
            }
        );
    }

    // Recover, stay up past stable_after: schedule restarts at attempt 1.
    s.refuse_connections(false);
    until(&mut st, is_connected).await;
    tokio::time::sleep(Duration::from_millis(350)).await;
    s.refuse_connections(true);
    s.drop_connections();
    let got = until(&mut st, |s| {
        matches!(s, ConnectionState::Reconnecting { .. })
    })
    .await;
    assert_eq!(
        got,
        ConnectionState::Reconnecting {
            attempt: 1,
            next_retry_in: INITIAL
        }
    );

    // A connection that does NOT stay up keeps the schedule climbing.
    s.refuse_connections(false);
    until(&mut st, is_connected).await;
    s.refuse_connections(true);
    s.drop_connections();
    let got = until(&mut st, |s| {
        matches!(s, ConnectionState::Reconnecting { .. })
    })
    .await;
    assert_eq!(
        got,
        ConnectionState::Reconnecting {
            attempt: 2,
            next_retry_in: Duration::from_millis(40)
        }
    );
}

#[tokio::test]
async fn no_retry_after_401_handshake_then_retry_on_demand() {
    let s = server().await;
    s.require_bearer("right");
    let mut c = config(&s);
    c.token = Some(Token::new("wrong"));
    let conn = Connection::start(c);
    let mut st = conn.state();
    let got = until(&mut st, |s| matches!(s, ConnectionState::Failed { .. })).await;
    assert_eq!(
        got,
        ConnectionState::Failed {
            reason: FailureReason::Unauthorized
        }
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(s.authorizations().len(), 1, "must not retry");

    s.require_bearer("wrong");
    conn.retry();
    until(&mut st, is_connected).await;
    assert_eq!(s.authorizations().len(), 2);
}

#[tokio::test]
async fn close_4401_and_4403_are_fatal() {
    for (code, reason) in [
        (4401, FailureReason::Unauthorized),
        (4403, FailureReason::OriginRejected),
    ] {
        let s = server().await;
        let conn = Connection::start(config(&s));
        let mut st = conn.state();
        until(&mut st, is_connected).await;
        s.close_connections_with(code);
        let got = until(&mut st, |s| matches!(s, ConnectionState::Failed { .. })).await;
        assert_eq!(got, ConnectionState::Failed { reason });
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(s.accepted_connections(), 1, "must not retry after {code}");
    }
}

#[tokio::test]
async fn not_running_on_404_and_4004_is_not_retried_until_asked() {
    let s = server().await;
    s.reject_all_handshakes(Some(404));
    let conn = Connection::start(config(&s));
    let mut st = conn.state();
    until(&mut st, |s| *s == ConnectionState::NotRunning).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(s.authorizations().len(), 1);

    // Profile started: retry connects.
    s.reject_all_handshakes(None);
    conn.retry();
    until(&mut st, is_connected).await;

    // Close code 4004 maps to NotRunning too.
    s.close_connections_with(4004);
    until(&mut st, |s| *s == ConnectionState::NotRunning).await;
}

#[tokio::test]
async fn transient_handshake_errors_are_retried() {
    let s = server().await;
    s.reject_next_handshakes(2, 503);
    let conn = Connection::start(config(&s));
    let mut st = conn.state();
    until(&mut st, is_connected).await;
    assert_eq!(s.authorizations().len(), 3);
    let _ = conn;
}

#[tokio::test]
async fn active_tab_gone_after_restart_with_new_target_ids() {
    let s = server().await;
    let conn = Connection::start(config(&s));
    let mut events = conn.events();
    let mut st = conn.state();
    until(&mut st, is_connected).await;
    conn.set_active_tab(Some(ActiveTab {
        target_id: "B".into(),
        screencast: ScreencastOptions::default(),
    }));
    s.wait_for_command("Page.startScreencast", 1).await.unwrap();

    // Browser restarts with different target ids.
    s.reply(
        "Target.getTargets",
        json!({"targetInfos": [page("C")["targetInfo"].clone()]}),
    );
    s.refuse_connections(true);
    s.drop_connections();
    until(&mut st, |s| {
        matches!(s, ConnectionState::Reconnecting { .. })
    })
    .await;
    s.refuse_connections(false);
    until(&mut st, is_connected).await;

    let ev = loop {
        let e = tokio::time::timeout(T, events.recv())
            .await
            .unwrap()
            .unwrap();
        if matches!(e, ConnectionEvent::ActiveTabGone { .. }) {
            break e;
        }
    };
    assert_eq!(
        ev,
        ConnectionEvent::ActiveTabGone {
            target_id: "B".into()
        }
    );
    assert_eq!(count(&s, "Page.startScreencast"), 1, "no guessing");
    assert!(conn.active_session().borrow().is_none());
    let pages = conn.pages().borrow().clone();
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].target_id, "C");
}

#[tokio::test]
async fn target_events_update_pages_and_survive_drop() {
    let s = server().await;
    let conn = Connection::start(config(&s));
    let mut st = conn.state();
    until(&mut st, is_connected).await;
    let mut pages = conn.pages();
    s.emit("Target.targetCreated", page("Z"));
    tokio::time::timeout(T, pages.wait_for(|p| p.len() == 3))
        .await
        .unwrap()
        .unwrap();
    s.refuse_connections(true);
    s.drop_connections();
    until(&mut st, |s| {
        matches!(s, ConnectionState::Reconnecting { .. })
    })
    .await;
    assert_eq!(conn.pages().borrow().len(), 3, "stale list kept while down");
    assert!(conn.client().is_none());
}
