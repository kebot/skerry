//! JS dialogs, popups and the permission policy on a `Connection`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rb_cdp::testing::FakeCdpServer;
use rb_cdp::{
    ActiveTab, BackoffConfig, Connection, ConnectionConfig, ConnectionEvent, ConnectionState,
    DialogKind, DialogResponse, PermissionPolicy, ScreencastOptions, POLICY_PERMISSIONS,
};
use serde_json::json;
use tokio::sync::broadcast;

const T: Duration = Duration::from_secs(5);

fn info(id: &str, url: &str) -> serde_json::Value {
    json!({"targetId": id, "type": "page", "title": id, "url": url})
}

/// Pages A and B. Every attach gets a fresh session id `S<n>-<target>`.
async fn server() -> FakeCdpServer {
    let s = FakeCdpServer::start().await;
    s.reply("Target.setDiscoverTargets", json!({}));
    s.reply(
        "Target.getTargets",
        json!({"targetInfos": [info("A", "https://a.test/x"), info("B", "about:blank")]}),
    );
    let n = Arc::new(AtomicUsize::new(0));
    s.reply_with("Target.attachToTarget", move |c| {
        let i = n.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(json!({"sessionId": format!("S{i}-{}", c.params["targetId"].as_str().unwrap())}))
    });
    s.reply("Page.enable", json!({}));
    s.reply("Page.startScreencast", json!({}));
    s.reply("Page.stopScreencast", json!({}));
    s.reply("Page.handleJavaScriptDialog", json!({}));
    s
}

fn config(s: &FakeCdpServer, policy: PermissionPolicy) -> ConnectionConfig {
    let mut c = ConnectionConfig::new(s.url(), None);
    c.permissions = policy;
    c.backoff = BackoffConfig {
        initial: Duration::from_millis(20),
        max: Duration::from_millis(80),
        jitter: 0.0,
        stable_after: Duration::from_millis(250),
    };
    c
}

async fn connected(s: &FakeCdpServer, policy: PermissionPolicy) -> Connection {
    let conn = Connection::start(config(s, policy));
    let mut st = conn.state();
    tokio::time::timeout(T, st.wait_for(|s| *s == ConnectionState::Connected))
        .await
        .unwrap()
        .unwrap();
    conn
}

fn session_for(s: &FakeCdpServer, target: &str) -> String {
    s.commands()
        .iter()
        .find(|c| c.method == "Page.enable" && c.session_id.as_deref().unwrap().ends_with(target))
        .unwrap_or_else(|| panic!("no Page.enable for {target}"))
        .session_id
        .clone()
        .unwrap()
}

async fn next_dialog_event(rx: &mut broadcast::Receiver<ConnectionEvent>) -> ConnectionEvent {
    loop {
        let e = tokio::time::timeout(T, rx.recv()).await.unwrap().unwrap();
        if matches!(
            e,
            ConnectionEvent::DialogOpened(_) | ConnectionEvent::DialogClosed { .. }
        ) {
            return e;
        }
    }
}

fn opening(kind: &str, msg: &str) -> serde_json::Value {
    json!({"url": "https://a.test/x", "message": msg, "type": kind,
           "hasBrowserHandler": false, "defaultPrompt": "dflt"})
}

#[tokio::test]
async fn observes_active_and_background_tabs_without_extra_screencast() {
    let s = server().await;
    let conn = connected(&s, PermissionPolicy::Ask).await;
    conn.set_active_tab(Some(ActiveTab {
        target_id: "B".into(),
        screencast: ScreencastOptions::default(),
    }));
    s.wait_for_command("Page.startScreencast", 1).await.unwrap();
    s.wait_for_command("Page.enable", 2).await.unwrap();
    let mut events = conn.events();
    let mut pending = conn.pending_dialogs();

    // Background Tab A.
    let sa = session_for(&s, "A");
    s.emit_in_session(
        &sa,
        "Page.javascriptDialogOpening",
        opening("prompt", "name?"),
    );
    let ConnectionEvent::DialogOpened(d) = next_dialog_event(&mut events).await else {
        panic!()
    };
    assert_eq!(d.target_id, "A");
    assert_eq!(d.session_id, sa);
    assert_eq!(d.kind, DialogKind::Prompt);
    assert_eq!(d.message, "name?");
    assert_eq!(d.default_prompt.as_deref(), Some("dflt"));
    assert_eq!(conn.pending_dialogs().borrow().len(), 1);

    // Active Tab B (observer session, not the screencast session).
    let sb = session_for(&s, "B");
    s.emit_in_session(
        &sb,
        "Page.javascriptDialogOpening",
        opening("beforeunload", ""),
    );
    let ConnectionEvent::DialogOpened(d) = next_dialog_event(&mut events).await else {
        panic!()
    };
    assert_eq!(d.target_id, "B");
    assert_eq!(d.kind, DialogKind::BeforeUnload);
    pending.changed().await.ok();
    assert_eq!(conn.pending_dialogs().borrow().len(), 2);

    // Closed by the page/agent.
    s.emit_in_session(
        &sa,
        "Page.javascriptDialogClosed",
        json!({"result": true, "userInput": "bob"}),
    );
    assert_eq!(
        next_dialog_event(&mut events).await,
        ConnectionEvent::DialogClosed {
            target_id: "A".into(),
            accepted: true,
            user_input: "bob".into()
        }
    );
    assert_eq!(conn.pending_dialogs().borrow().len(), 1);

    let count = |m: &str| s.commands().iter().filter(|c| c.method == m).count();
    assert_eq!(count("Page.startScreencast"), 1, "no duplicate screencast");
    assert_eq!(
        count("Target.attachToTarget"),
        3,
        "2 observers + 1 screencast"
    );
}

#[tokio::test]
async fn handle_dialog_sends_command_on_the_observing_session() {
    let s = server().await;
    let conn = connected(&s, PermissionPolicy::Ask).await;
    s.wait_for_command("Page.enable", 2).await.unwrap();
    let mut events = conn.events();
    let sa = session_for(&s, "A");
    s.emit_in_session(&sa, "Page.javascriptDialogOpening", opening("prompt", "q"));
    next_dialog_event(&mut events).await;

    conn.handle_dialog(
        "A",
        DialogResponse {
            accept: true,
            prompt_text: Some("hi".into()),
        },
    )
    .await
    .unwrap();
    let cmd = s
        .wait_for_command("Page.handleJavaScriptDialog", 1)
        .await
        .unwrap();
    assert_eq!(cmd.session_id.as_deref(), Some(sa.as_str()));
    assert_eq!(cmd.params, json!({"accept": true, "promptText": "hi"}));

    // The list changes when the browser reports the close.
    assert_eq!(conn.pending_dialogs().borrow().len(), 1);
    s.emit_in_session(
        &sa,
        "Page.javascriptDialogClosed",
        json!({"result": true, "userInput": "hi"}),
    );
    assert!(matches!(
        next_dialog_event(&mut events).await,
        ConnectionEvent::DialogClosed { .. }
    ));
    assert!(conn.pending_dialogs().borrow().is_empty());

    // Dismiss without text: no promptText.
    s.emit_in_session(
        &sa,
        "Page.javascriptDialogOpening",
        opening("confirm", "ok?"),
    );
    next_dialog_event(&mut events).await;
    conn.handle_dialog(
        "A",
        DialogResponse {
            accept: false,
            prompt_text: None,
        },
    )
    .await
    .unwrap();
    let cmd = s
        .wait_for_command("Page.handleJavaScriptDialog", 2)
        .await
        .unwrap();
    assert_eq!(cmd.params, json!({"accept": false}));
}

#[tokio::test]
async fn already_closed_dialog_is_not_an_error() {
    let s = server().await;
    s.reply_error(
        "Page.handleJavaScriptDialog",
        -32000,
        "No dialog is showing",
    );
    let conn = connected(&s, PermissionPolicy::Ask).await;
    s.wait_for_command("Page.enable", 2).await.unwrap();
    let mut events = conn.events();
    s.emit_in_session(
        &session_for(&s, "A"),
        "Page.javascriptDialogOpening",
        opening("alert", "hey"),
    );
    next_dialog_event(&mut events).await;

    let r = DialogResponse {
        accept: true,
        prompt_text: None,
    };
    conn.handle_dialog("A", r.clone()).await.unwrap();
    assert!(matches!(
        next_dialog_event(&mut events).await,
        ConnectionEvent::DialogClosed { target_id, .. } if target_id == "A"
    ));
    assert!(conn.pending_dialogs().borrow().is_empty());
    // Nothing pending any more: still fine, and no command is sent.
    conn.handle_dialog("A", r).await.unwrap();
    assert_eq!(
        s.commands()
            .iter()
            .filter(|c| c.method == "Page.handleJavaScriptDialog")
            .count(),
        1
    );
}

#[tokio::test]
async fn targets_created_after_connect_are_observed_and_popups_are_pages() {
    let s = server().await;
    let conn = connected(&s, PermissionPolicy::Ask).await;
    s.wait_for_command("Page.enable", 2).await.unwrap();
    let mut pages = conn.pages();

    // window.open(): a new page target whose opener is A.
    s.emit(
        "Target.targetCreated",
        json!({"targetInfo": {"targetId": "P", "type": "page", "title": "", "url": "",
                              "openerId": "A", "canAccessOpener": true}}),
    );
    s.wait_for_command("Page.enable", 3).await.unwrap();
    tokio::time::timeout(T, pages.wait_for(|p| p.iter().any(|t| t.target_id == "P")))
        .await
        .unwrap()
        .unwrap();
    let popup = conn
        .pages()
        .borrow()
        .iter()
        .find(|t| t.target_id == "P")
        .cloned()
        .unwrap();
    assert_eq!(popup.opener_id.as_deref(), Some("A"));
    assert!(popup.is_page());

    // Non-page targets are neither listed nor observed.
    s.emit(
        "Target.targetCreated",
        json!({"targetInfo": {"targetId": "W", "type": "service_worker", "url": "https://a.test/sw.js"}}),
    );
    // A dialog in the popup surfaces.
    let mut events = conn.events();
    s.emit_in_session(
        &session_for(&s, "P"),
        "Page.javascriptDialogOpening",
        opening("alert", "popup"),
    );
    let ConnectionEvent::DialogOpened(d) = next_dialog_event(&mut events).await else {
        panic!()
    };
    assert_eq!(d.target_id, "P");
    assert_eq!(
        s.commands()
            .iter()
            .filter(|c| c.method == "Target.attachToTarget")
            .count(),
        3
    );

    // Destroying the target drops its dialog.
    s.emit("Target.targetDestroyed", json!({"targetId": "P"}));
    assert!(matches!(
        next_dialog_event(&mut events).await,
        ConnectionEvent::DialogClosed { target_id, .. } if target_id == "P"
    ));
}

#[tokio::test]
async fn reconnect_reestablishes_observation_and_clears_pending() {
    let s = server().await;
    let conn = connected(&s, PermissionPolicy::Ask).await;
    s.wait_for_command("Page.enable", 2).await.unwrap();
    let mut events = conn.events();
    s.emit_in_session(
        &session_for(&s, "A"),
        "Page.javascriptDialogOpening",
        opening("alert", "x"),
    );
    next_dialog_event(&mut events).await;

    s.drop_connections();
    s.wait_for_command("Page.enable", 4).await.unwrap();
    let mut st = conn.state();
    tokio::time::timeout(T, st.wait_for(|s| *s == ConnectionState::Connected))
        .await
        .unwrap()
        .unwrap();
    assert!(conn.pending_dialogs().borrow().is_empty());

    // The new sessions work.
    let last_a = s
        .commands()
        .iter()
        .rfind(|c| c.method == "Page.enable" && c.session_id.as_deref().unwrap().ends_with('A'))
        .unwrap()
        .session_id
        .clone()
        .unwrap();
    s.emit_in_session(
        &last_a,
        "Page.javascriptDialogOpening",
        opening("alert", "again"),
    );
    let ConnectionEvent::DialogOpened(d) = next_dialog_event(&mut events).await else {
        panic!()
    };
    assert_eq!(d.session_id, last_a);
}

#[tokio::test]
async fn permission_policy_commands() {
    // Deny (default): once per http(s) origin, for every listed permission.
    let s = server().await;
    let conn = connected(&s, PermissionPolicy::default()).await;
    s.wait_for_command("Browser.setPermission", POLICY_PERMISSIONS.len())
        .await
        .unwrap();
    let sets: Vec<_> = s
        .commands()
        .into_iter()
        .filter(|c| c.method == "Browser.setPermission")
        .collect();
    assert!(sets.iter().all(|c| c.params["origin"] == "https://a.test"));
    assert!(sets.iter().all(|c| c.params["setting"] == "denied"));
    assert_eq!(sets[0].params["permission"]["name"], "geolocation");
    assert!(sets
        .iter()
        .any(|c| c.params["permission"]["name"] == "notifications"));

    // Navigating B (was about:blank) to a new origin adds it; same origin
    // again does nothing.
    let n = POLICY_PERMISSIONS.len();
    s.emit(
        "Target.targetInfoChanged",
        json!({"targetInfo": info("B", "http://b.test:8080/p")}),
    );
    s.wait_for_command("Browser.setPermission", 2 * n)
        .await
        .unwrap();
    s.emit(
        "Target.targetInfoChanged",
        json!({"targetInfo": info("B", "http://b.test:8080/q")}),
    );
    // Use a later command as a barrier.
    s.emit(
        "Target.targetInfoChanged",
        json!({"targetInfo": info("B", "https://c.test/")}),
    );
    s.wait_for_command("Browser.setPermission", 3 * n)
        .await
        .unwrap();
    let origins: Vec<_> = s
        .commands()
        .iter()
        .filter(|c| c.method == "Browser.setPermission")
        .map(|c| c.params["origin"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(origins.len(), 3 * n);
    assert_eq!(origins[n], "http://b.test:8080");
    assert_eq!(origins[2 * n], "https://c.test");
    drop(conn);

    // Allow.
    let s = server().await;
    let _c = connected(&s, PermissionPolicy::Allow).await;
    let c = s
        .wait_for_command("Browser.setPermission", 1)
        .await
        .unwrap();
    assert_eq!(c.params["setting"], "granted");

    // Ask: reserved, sends nothing.
    let s = server().await;
    let _c = connected(&s, PermissionPolicy::Ask).await;
    s.wait_for_command("Page.enable", 2).await.unwrap();
    assert!(s
        .commands()
        .iter()
        .all(|c| c.method != "Browser.setPermission"));
}
