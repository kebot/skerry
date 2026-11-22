use super::*;
use std::sync::Mutex;

fn address(raw: &str) -> SelkiesAddress {
    parse_address(raw).unwrap()
}

fn server(base: &str, auth: SelkiesAuth) -> SelkiesServer {
    SelkiesServer::new(
        Url::parse(base).unwrap(),
        auth,
        None,
        ServerSource::Typed,
        true,
    )
    .unwrap()
}

// ---- addresses ------------------------------------------------------------------------

#[test]
fn parses_a_plain_https_address_and_adds_the_trailing_slash() {
    let parsed = address("https://desktop.example.com");
    assert_eq!(parsed.base.as_str(), "https://desktop.example.com/");
    assert_eq!(parsed.token, None);
    assert_eq!(
        address("https://desktop.example.com:8443/").base.port(),
        Some(8443)
    );
}

#[test]
fn a_bare_host_gets_https() {
    assert_eq!(
        address("desktop.lab:8443").base.as_str(),
        "https://desktop.lab:8443/"
    );
}

#[test]
fn keeps_the_subfolder_as_the_base_path() {
    assert_eq!(
        address("https://gpu.example.com/kasm/desk").base.as_str(),
        "https://gpu.example.com/kasm/desk/"
    );
}

#[test]
fn takes_the_token_from_the_fragment_before_the_query() {
    let parsed = address("https://h.example/s/?token=query#token=frag");
    assert_eq!(parsed.token.as_deref(), Some("frag"));
    assert_eq!(parsed.base.as_str(), "https://h.example/s/");
}

#[test]
fn takes_the_token_from_the_query_and_strips_it() {
    let parsed = address("https://h.example/s/?token=abc&x=1");
    assert_eq!(parsed.token.as_deref(), Some("abc"));
    assert_eq!(parsed.base.query(), None);
}

#[test]
fn percent_decodes_a_fragment_token() {
    assert_eq!(
        address("https://h.example/#token=a%2Fb%3D")
            .token
            .as_deref(),
        Some("a/b=")
    );
}

#[test]
fn a_plus_in_a_fragment_token_is_a_space() {
    assert_eq!(
        address("https://h.example/#token=a+b").token.as_deref(),
        Some("a b")
    );
}

#[test]
fn moves_userinfo_into_the_login_and_clears_it() {
    let parsed = address("https://ubuntu:mypasswd@h.example/");
    assert_eq!(parsed.username.as_deref(), Some("ubuntu"));
    assert_eq!(parsed.password.as_deref(), Some("mypasswd"));
    assert_eq!(parsed.base.as_str(), "https://h.example/");
}

#[test]
fn userinfo_is_percent_decoded() {
    let parsed = address("https://us%40er:p%3As%20s@h.example/");
    assert_eq!(parsed.username.as_deref(), Some("us@er"));
    assert_eq!(parsed.password.as_deref(), Some("p:s s"));
}

#[test]
fn refuses_plain_http_off_loopback() {
    assert_eq!(
        parse_address("http://desktop.example.com/"),
        Err(AddressError::HttpRefused)
    );
    assert_eq!(
        parse_address("http://192.168.1.4:8080/"),
        Err(AddressError::HttpRefused)
    );
}

#[test]
fn allows_plain_http_to_localhost_127_and_ipv6_loopback() {
    for raw in [
        "http://localhost:8080",
        "http://LOCALHOST./",
        "http://127.0.0.1:8080/",
        "http://127.4.5.6/",
        "http://[::1]:8080/",
    ] {
        assert!(parse_address(raw).is_ok(), "{raw}");
    }
}

#[test]
fn refuses_subdomains_of_localhost_and_unspecified() {
    for raw in [
        "http://evil.localhost/",
        "http://0.0.0.0:8080/",
        "http://[::ffff:127.0.0.1]/",
        "http://[::]/",
    ] {
        assert_eq!(parse_address(raw), Err(AddressError::HttpRefused), "{raw}");
    }
}

#[test]
fn refuses_sharing_and_display_fragments() {
    for raw in [
        "https://h.example/#shared",
        "https://h.example/#player2",
        "https://h.example/#display2-1920x1080",
        "https://h.example/#token=a&shared",
    ] {
        assert_eq!(parse_address(raw), Err(AddressError::NotPrimary), "{raw}");
    }
}

#[test]
fn recognises_a_sealskin_key_pasted_as_an_address() {
    assert_eq!(
        parse_address("ssk_id_secret@seal.example"),
        Err(AddressError::LooksLikeKey)
    );
    assert_eq!(
        parse_address("ftp://h.example/"),
        Err(AddressError::Invalid)
    );
    assert_eq!(parse_address("  "), Err(AddressError::Empty));
}

// ---- logins ---------------------------------------------------------------------------

fn header(map: &HeaderMap, name: &str) -> Option<String> {
    map.get(name).map(|v| v.to_str().unwrap().to_string())
}

#[test]
fn basic_login_rides_authorization_on_http_and_websocket() {
    let auth = SelkiesAuth::new("ubuntu", "mypasswd", None);
    let expected = "Basic dWJ1bnR1Om15cGFzc3dk";
    assert_eq!(
        header(&auth.http_headers(), "authorization").as_deref(),
        Some(expected)
    );
    let ws = auth.websocket_headers();
    assert_eq!(header(&ws, "authorization").as_deref(), Some(expected));
    assert!(ws.get("sec-websocket-protocol").is_none());
}

#[test]
fn token_alone_rides_bearer_on_http_and_a_subprotocol_on_websocket() {
    let auth = SelkiesAuth::new("", "", Some("t?k>".into()));
    assert_eq!(
        header(&auth.http_headers(), "authorization").as_deref(),
        Some("Bearer t?k>")
    );
    let ws = auth.websocket_headers();
    assert!(ws.get("authorization").is_none());
    // base64url without padding: "t?k>" is dD9rPg== in standard base64.
    assert_eq!(
        header(&ws, "sec-websocket-protocol").as_deref(),
        Some("selkies, selkies.token.dD9rPg")
    );
}

#[test]
fn basic_and_token_together_put_the_token_in_the_selkies_token_cookie() {
    let auth = SelkiesAuth::new("ubuntu", "pw", Some("a b".into()));
    let http = auth.http_headers();
    let authorization = header(&http, "authorization").unwrap();
    assert!(authorization.starts_with("Basic "));
    assert!(!authorization.contains("Bearer"));
    assert_eq!(
        header(&http, "cookie").as_deref(),
        Some("selkies_token=a%20b")
    );
    let ws = auth.websocket_headers();
    assert!(header(&ws, "authorization").unwrap().starts_with("Basic "));
    assert!(header(&ws, "sec-websocket-protocol")
        .unwrap()
        .starts_with("selkies, selkies.token."));
}

#[test]
fn credential_headers_are_marked_sensitive() {
    let auth = SelkiesAuth::new("ubuntu", "pw", Some("tok".into()));
    for map in [
        auth.http_headers(),
        auth.websocket_headers(),
        auth.token_headers(),
    ] {
        for (name, value) in &map {
            assert!(value.is_sensitive(), "{name}");
        }
    }
}

#[test]
fn no_direct_request_carries_an_origin() {
    let server = server(
        "https://h.example/sub/",
        SelkiesAuth::new("u", "p", Some("t".into())),
    );
    let request = server.websocket_request().unwrap();
    assert!(request.headers().get("origin").is_none());
    assert!(server.auth.http_headers().get("origin").is_none());
}

#[test]
fn direct_websocket_request_targets_api_websockets_under_the_subfolder() {
    let tls = server("https://h.example/sub/", SelkiesAuth::default());
    assert_eq!(
        tls.websocket_request().unwrap().uri().to_string(),
        "wss://h.example/sub/api/websockets"
    );
    let plain = server("http://localhost:8080/", SelkiesAuth::default());
    assert_eq!(
        plain.websocket_request().unwrap().uri().to_string(),
        "ws://localhost:8080/api/websockets"
    );
}

#[test]
fn prefs_key_differs_from_a_sealskin_server_on_the_same_host() {
    let direct = server("https://seal.example/", SelkiesAuth::default());
    assert_eq!(direct.prefs_key(), "selkies-seal.example");
    assert_ne!(direct.prefs_key(), "seal.example");
    let port = server("https://seal.example:8443/", SelkiesAuth::default());
    assert_eq!(port.prefs_key(), "selkies-seal.example_8443");
}

#[test]
fn prefs_key_is_a_single_safe_file_name() {
    let long = format!("https://h.example/{}/../x y/", "a".repeat(200));
    for base in [
        "https://h.example/a/b/",
        "https://h.example/a_b/",
        "https://[::1]:8080/x/",
        long.as_str(),
    ] {
        let key = server(base, SelkiesAuth::default()).prefs_key();
        assert!(
            key.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')),
            "{key}"
        );
        assert!(key.len() <= 8 + 64 + 6 + 1 + 80 + 9, "{key}");
    }
    // Two subfolders that map to the same slug still get different files.
    let a = server("https://h.example/a/b/", SelkiesAuth::default()).prefs_key();
    let b = server("https://h.example/a_b/", SelkiesAuth::default()).prefs_key();
    assert_ne!(a, b);
}

#[test]
fn as_session_has_no_storage() {
    let session = server("https://h.example/kasm/", SelkiesAuth::default()).as_session();
    assert!(!crate::sealskin::has_storage(&session.home));
    assert!(session.session_id.starts_with("selkies|selkies-h.example"));
    assert_eq!(session.app_name, "h.example/kasm/");
}

// ---- the gate -------------------------------------------------------------------------

type Heads = Arc<Mutex<Vec<String>>>;

fn reply(status: &str, extra: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\n{extra}content-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// An in-process server answering each request from its head (request line and headers,
/// lowercased header names kept as sent).
async fn serve<R: Into<Vec<u8>>>(
    answer: impl Fn(&str) -> R + Send + Sync + 'static,
) -> (Url, Heads) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    let heads: Heads = Arc::default();
    let log = heads.clone();
    let answer = Arc::new(answer);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let (log, answer) = (log.clone(), answer.clone());
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut chunk = [0u8; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(read) => request.extend_from_slice(&chunk[..read]),
                    }
                }
                let head = String::from_utf8_lossy(&request).to_string();
                log.lock().unwrap().push(head.clone());
                let _ = stream.write_all(&answer(&head).into()).await;
            });
        }
    });
    (base, heads)
}

const STATUS_WS: &str = r#"{"current_mode":"websockets","available_modes":["websockets","webrtc"],"enable_dual_mode":true,"ui_layout":"x"}"#;
const SESSIONS: &str = r#"{"sessions":[{"id":"a1","transport":"websockets","role":"controller","slot":null,"display":"primary","connected_at":1700000000.5,"rtt_ms":3}]}"#;
const BASIC_CHALLENGE: &str =
    "www-authenticate: Basic realm=\"Selkies Restricted\", charset=\"UTF-8\"\r\n";
const BEARER_CHALLENGE: &str = "www-authenticate: Bearer realm=\"Selkies Restricted\"\r\n";

fn target(head: &str) -> &str {
    head.split_whitespace().nth(1).unwrap_or("")
}

fn authorization(head: &str) -> Option<String> {
    head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("authorization")
            .then(|| value.trim().to_string())
    })
}

fn cookie(head: &str) -> Option<String> {
    head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("cookie")
            .then(|| value.trim().to_string())
    })
}

/// Selkies in legacy mode with Basic `ubuntu`/`mypasswd`, in the order `authorize` checks.
fn legacy_basic(head: &str) -> String {
    match target(head) {
        "/api/status" => reply("200 OK", "", STATUS_WS),
        _ => match authorization(head).as_deref() {
            None => reply(
                "401 Unauthorized",
                BASIC_CHALLENGE,
                "Authorization Required",
            ),
            Some("Basic dWJ1bnR1Om15cGFzc3dk") => match target(head) {
                "/api/sessions" => reply("200 OK", "", SESSIONS),
                _ => reply("405 Method Not Allowed", "", ""),
            },
            Some(_) => reply("401 Unauthorized", BASIC_CHALLENGE, "Invalid Credentials"),
        },
    }
}

/// Selkies with `--master-token` and Basic on; the valid session token is `good`.
fn secure_with_basic(head: &str) -> String {
    let path = target(head);
    if path == "/api/status" {
        return reply("200 OK", "", STATUS_WS);
    }
    if path == "/api/tokens" {
        return reply("401 Unauthorized", BEARER_CHALLENGE, "Unauthorized");
    }
    let auth = authorization(head);
    let token_ok = auth.as_deref() == Some("Bearer good")
        || cookie(head).is_some_and(|c| c.contains("selkies_token=good"));
    if token_ok {
        return reply("200 OK", "", SESSIONS);
    }
    // Basic is on: a request without a valid token falls through to Basic (viewer).
    match auth.as_deref() {
        Some("Basic dWJ1bnR1Om15cGFzc3dk") => reply("200 OK", "", SESSIONS),
        Some(a) if a.starts_with("Basic ") => {
            reply("401 Unauthorized", BASIC_CHALLENGE, "Invalid Credentials")
        }
        _ => reply(
            "401 Unauthorized",
            BASIC_CHALLENGE,
            "Authorization Required",
        ),
    }
}

/// Secure mode with Basic off: tokens only.
fn secure_tokens_only(head: &str) -> String {
    match target(head) {
        "/api/status" => reply("200 OK", "", STATUS_WS),
        "/api/tokens" => reply("401 Unauthorized", BEARER_CHALLENGE, "Unauthorized"),
        _ if authorization(head).as_deref() == Some("Bearer good") => reply("200 OK", "", SESSIONS),
        _ => reply("401 Unauthorized", BEARER_CHALLENGE, "Unauthorized"),
    }
}

fn at(base: &Url, auth: SelkiesAuth) -> SelkiesServer {
    server(base.as_str(), auth)
}

#[tokio::test]
async fn gate_accepts_websockets_mode_with_valid_basic_login() {
    let (base, _) = serve(legacy_basic).await;
    let report = gate(&at(&base, SelkiesAuth::new("ubuntu", "mypasswd", None)))
        .await
        .unwrap();
    assert!(report.dual_mode);
    assert_eq!(report.pages.len(), 1);
    assert_eq!(report.pages[0].role, "controller");
}

#[tokio::test]
async fn gate_sends_status_without_credentials() {
    let (base, heads) = serve(legacy_basic).await;
    gate(&at(&base, SelkiesAuth::new("ubuntu", "mypasswd", None)))
        .await
        .unwrap();
    let heads = heads.lock().unwrap();
    let status = heads.iter().find(|h| target(h) == "/api/status").unwrap();
    assert!(authorization(status).is_none());
    assert!(heads
        .iter()
        .all(|h| !h.to_lowercase().contains("\norigin:")));
}

#[tokio::test]
async fn gate_refuses_a_server_without_api_status() {
    let (base, _) = serve(|_| reply("404 Not Found", "", "<html>nope</html>")).await;
    assert_eq!(
        gate(&at(&base, SelkiesAuth::default())).await,
        Err(GateError::NotSelkies)
    );
    let (base, _) = serve(|_| reply("200 OK", "", "{\"hello\":1}")).await;
    assert_eq!(
        gate(&at(&base, SelkiesAuth::default())).await,
        Err(GateError::NotSelkies)
    );
}

#[tokio::test]
async fn gate_refuses_webrtc_mode_and_reports_dual_mode() {
    let (base, _) = serve(|_| {
        reply(
            "200 OK",
            "",
            r#"{"current_mode":"webrtc","available_modes":["websockets","webrtc"],"enable_dual_mode":true}"#,
        )
    })
    .await;
    assert_eq!(
        gate(&at(&base, SelkiesAuth::default())).await,
        Err(GateError::WebRtc { dual_mode: true })
    );
    let (base, _) = serve(|_| {
        reply(
            "200 OK",
            "",
            r#"{"current_mode":"webrtc","available_modes":["webrtc"],"enable_dual_mode":false}"#,
        )
    })
    .await;
    assert_eq!(
        gate(&at(&base, SelkiesAuth::default())).await,
        Err(GateError::WebRtc { dual_mode: false })
    );
}

#[tokio::test]
async fn gate_treats_a_null_current_mode_as_starting() {
    let (base, _) = serve(|_| {
        reply(
            "200 OK",
            "",
            r#"{"current_mode":null,"available_modes":["websockets"]}"#,
        )
    })
    .await;
    let error = gate(&at(&base, SelkiesAuth::default())).await.unwrap_err();
    assert_eq!(error, GateError::Starting);
    assert!(error.keeps_library());
}

#[tokio::test]
async fn gate_reports_needs_login_on_a_basic_challenge_without_credentials() {
    let (base, _) = serve(legacy_basic).await;
    assert_eq!(
        gate(&at(&base, SelkiesAuth::default())).await,
        Err(GateError::NeedsLogin)
    );
}

#[tokio::test]
async fn gate_reports_bad_login_on_a_basic_challenge_with_credentials() {
    let (base, _) = serve(legacy_basic).await;
    assert_eq!(
        gate(&at(&base, SelkiesAuth::new("ubuntu", "wrong", None))).await,
        Err(GateError::BadLogin)
    );
    // A username with an empty password goes out as `ubuntu:` and is read by the body.
    assert_eq!(
        gate(&at(&base, SelkiesAuth::new("ubuntu", "", None))).await,
        Err(GateError::BadLogin)
    );
}

#[tokio::test]
async fn gate_reports_needs_token_and_bad_token_on_bearer_challenges() {
    let (base, _) = serve(secure_tokens_only).await;
    assert_eq!(
        gate(&at(&base, SelkiesAuth::default())).await,
        Err(GateError::NeedsToken)
    );
    assert_eq!(
        gate(&at(&base, SelkiesAuth::new("", "", Some("bad".into())))).await,
        Err(GateError::BadToken)
    );
    assert!(
        gate(&at(&base, SelkiesAuth::new("", "", Some("good".into()))))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn gate_reports_needs_token_on_a_secure_server_even_with_a_valid_basic_login() {
    let (base, _) = serve(secure_with_basic).await;
    assert_eq!(
        gate(&at(&base, SelkiesAuth::new("ubuntu", "mypasswd", None))).await,
        Err(GateError::NeedsToken)
    );
}

#[tokio::test]
async fn gate_reports_bad_token_even_when_basic_is_accepted() {
    let (base, heads) = serve(secure_with_basic).await;
    assert_eq!(
        gate(&at(
            &base,
            SelkiesAuth::new("ubuntu", "mypasswd", Some("bad".into()))
        ))
        .await,
        Err(GateError::BadToken)
    );
    // The token was checked alone, with Bearer and no Basic.
    let heads = heads.lock().unwrap();
    assert!(
        heads
            .iter()
            .any(|h| target(h) == "/api/sessions"
                && authorization(h).as_deref() == Some("Bearer bad"))
    );
    drop(heads);
    assert!(gate(&at(
        &base,
        SelkiesAuth::new("ubuntu", "mypasswd", Some("good".into()))
    ))
    .await
    .is_ok());
}

#[tokio::test]
async fn gate_recognises_a_sealskin_json_403() {
    let (base, _) = serve(|_| {
        reply(
            "403 Forbidden",
            "content-type: application/json\r\n",
            r#"{"detail":"Not authenticated"}"#,
        )
    })
    .await;
    assert_eq!(
        gate(&at(&base, SelkiesAuth::default())).await,
        Err(GateError::SealSkinAddress)
    );
}

#[tokio::test]
async fn gate_does_not_follow_redirects_or_send_credentials_to_location() {
    let (elsewhere, seen_elsewhere) = serve(|_| reply("200 OK", "", STATUS_WS)).await;
    let location = format!("location: {elsewhere}api/status\r\n");
    let (base, _) = serve(move |_| reply("302 Found", &location, "")).await;
    assert_eq!(
        gate(&at(&base, SelkiesAuth::new("ubuntu", "pw", None))).await,
        Err(GateError::NotSelkies)
    );
    assert!(seen_elsewhere.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_proxy_login_on_api_status_is_retried_with_the_login() {
    let (base, heads) = serve(|head| match authorization(head) {
        None => reply("401 Unauthorized", BASIC_CHALLENGE, "proxy"),
        Some(_) => legacy_basic(head),
    })
    .await;
    assert!(
        gate(&at(&base, SelkiesAuth::new("ubuntu", "mypasswd", None)))
            .await
            .is_ok()
    );
    assert!(heads.lock().unwrap().len() >= 3);
}

#[tokio::test]
async fn a_refused_connection_is_a_network_error_that_keeps_the_library() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    drop(listener);
    let error = gate(&at(&base, SelkiesAuth::default())).await.unwrap_err();
    assert_eq!(error, GateError::Network(NetworkReason::Refused));
    assert!(error.keeps_library());
    for refusal in [
        GateError::NotSelkies,
        GateError::BadLogin,
        GateError::WebRtc { dual_mode: false },
        GateError::PinMismatch {
            new: [0; 32],
            old: [1; 32],
        },
    ] {
        assert!(!refusal.keeps_library(), "{refusal:?}");
    }
}

#[tokio::test]
async fn an_untrusted_certificate_is_reported_with_its_fingerprint() {
    use crate::tls::test_server;
    let leaf = test_server::self_signed(&["localhost"]);
    let address =
        test_server::serve(vec![leaf.der.clone()], &leaf.key, |head| legacy_basic(head)).await;
    let base = Url::parse(&format!("https://localhost:{}/", address.port())).unwrap();
    let unpinned = at(&base, SelkiesAuth::new("ubuntu", "mypasswd", None));
    let expected = crate::tls::fingerprint(&leaf.der);
    assert_eq!(gate(&unpinned).await, Err(GateError::Untrusted(expected)));
    let pinned = unpinned.with_pin(expected).unwrap();
    assert!(gate(&pinned).await.is_ok());
    let stale = unpinned.with_pin([7; 32]).unwrap();
    assert_eq!(
        gate(&stale).await,
        Err(GateError::PinMismatch {
            new: expected,
            old: [7; 32]
        })
    );
}

#[tokio::test]
async fn screenshot_rejects_non_png_and_oversize_bodies() {
    let png = b"\x89PNG\r\n\x1a\nrest";
    let (base, _) = serve(move |head| match target(head) {
        "/api/screenshot" if authorization(head).is_some() => [
            format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                png.len()
            )
            .as_bytes(),
            png.as_slice(),
        ]
        .concat(),
        _ => reply(
            "401 Unauthorized",
            BASIC_CHALLENGE,
            "Authorization Required",
        )
        .into_bytes(),
    })
    .await;
    let login = SelkiesAuth::new("ubuntu", "mypasswd", None);
    assert_eq!(screenshot(&at(&base, login.clone())).await.unwrap(), png);
    let (base, _) = serve(|_| reply("200 OK", "", "GIF89a")).await;
    assert!(screenshot(&at(&base, login.clone())).await.is_err());
    let (base, _) = serve(|_| {
        format!(
            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            SCREENSHOT_MAX_BYTES + 1
        )
    })
    .await;
    assert!(screenshot(&at(&base, login.clone())).await.is_err());
    let (base, heads) =
        serve(|_| reply("503 Service Unavailable", "retry-after: 1\r\n", "busy")).await;
    assert!(screenshot(&at(&base, login)).await.is_err());
    assert_eq!(heads.lock().unwrap().len(), 1, "a 503 is not retried");
}

// ---- saved servers --------------------------------------------------------------------

fn saved(url: &str) -> SavedServer {
    SavedServer {
        url: url.into(),
        username: Some("ubuntu".into()),
        password: Some("pw".into()),
        token: Some("tok".into()),
        certificate_sha256: Some(crate::tls::fingerprint_text(&[0xAB; 32])),
    }
}

fn config_path() -> (tempfile::TempDir, std::path::PathBuf) {
    let home = tempfile::tempdir().unwrap();
    let path = config_store::config_file_in(home.path());
    (home, path)
}

#[test]
fn saved_server_round_trips_through_a_private_config_toml() {
    use std::os::unix::fs::PermissionsExt as _;
    let (_home, path) = config_path();
    assert!(saved_servers_at(&path).unwrap().is_empty());
    save_server_at(&path, &saved("https://h.example/"), true).unwrap();
    assert_eq!(
        saved_servers_at(&path).unwrap(),
        vec![saved("https://h.example/")]
    );
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(path.parent().unwrap()), 0o700);
    let server = SelkiesServer::from_saved(&saved("https://h.example/")).unwrap();
    assert_eq!(server.pin(), Some([0xAB; 32]));
    assert_eq!(server.source(), ServerSource::Saved);
    assert_eq!(server.saved(), saved("https://h.example/"));
}

#[test]
fn saving_a_server_keeps_the_application_key_and_comments() {
    let (_home, path) = config_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "# my notes\napplication_key = \"ssk_id_secret@seal.example\"\n",
    )
    .unwrap();
    save_server_at(&path, &saved("https://h.example/"), true).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("# my notes"), "{text}");
    assert!(text.contains("application_key = \"ssk_id_secret@seal.example\""));
    assert!(text.contains("[[selkies]]"));
    // Forgetting the SealSkin key leaves the servers alone.
    crate::sealskin::forget_key_at(&path, "ssk_id_secret@seal.example").unwrap();
    assert_eq!(saved_servers_at(&path).unwrap().len(), 1);
}

#[test]
fn saving_moves_the_server_to_the_front_and_keeps_five() {
    let (_home, path) = config_path();
    for n in 0..7 {
        save_server_at(&path, &saved(&format!("https://h{n}.example/")), true).unwrap();
    }
    save_server_at(&path, &saved("https://h3.example"), true).unwrap();
    let urls: Vec<String> = saved_servers_at(&path)
        .unwrap()
        .into_iter()
        .map(|s| s.url)
        .collect();
    assert_eq!(
        urls,
        [
            "https://h3.example/",
            "https://h6.example/",
            "https://h5.example/",
            "https://h4.example/",
            "https://h2.example/",
        ]
    );
}

#[test]
fn remember_off_saves_no_password_or_token() {
    let (_home, path) = config_path();
    save_server_at(&path, &saved("https://h.example/"), false).unwrap();
    let back = &saved_servers_at(&path).unwrap()[0];
    assert_eq!(back.username.as_deref(), Some("ubuntu"));
    assert_eq!(back.password, None);
    assert_eq!(back.token, None);
    // The pin is not a secret: it is kept, so the sheet doesn't come back every launch.
    assert!(back.certificate_sha256.is_some());
}

#[test]
fn remember_off_on_an_existing_entry_removes_its_password_and_token() {
    let (_home, path) = config_path();
    save_server_at(&path, &saved("https://h.example/"), true).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        text.replace("[[selkies]]\n", "[[selkies]]\nnote = \"mine\"\n"),
    )
    .unwrap();
    save_server_at(&path, &saved("https://h.example/"), false).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        !text.contains("password") && !text.contains("token ="),
        "{text}"
    );
    assert!(text.contains("note = \"mine\""), "{text}");
}

#[test]
fn forgetting_a_server_removes_only_that_entry() {
    let (_home, path) = config_path();
    save_server_at(&path, &saved("https://a.example/"), true).unwrap();
    save_server_at(&path, &saved("https://b.example/"), true).unwrap();
    forget_server_at(&path, "https://b.example").unwrap();
    let left = saved_servers_at(&path).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].url, "https://a.example/");
    assert_eq!(config_store::start_with(&path).unwrap(), None);
}

#[test]
fn start_with_follows_the_last_successful_connect() {
    let (_home, path) = config_path();
    save_server_at(&path, &saved("https://a.example/"), true).unwrap();
    assert_eq!(
        config_store::start_with(&path).unwrap(),
        Some(StartWith::Selkies)
    );
    crate::sealskin::save_key_at(&path, "ssk_id_secret@seal.example").unwrap();
    assert_eq!(
        config_store::start_with(&path).unwrap(),
        Some(StartWith::SealSkin)
    );
    save_server_at(&path, &saved("https://a.example/"), true).unwrap();
    clear_start_with_at(&path).unwrap();
    assert_eq!(config_store::start_with(&path).unwrap(), None);
    assert_eq!(saved_servers_at(&path).unwrap().len(), 1);
}

#[test]
fn a_malformed_selkies_table_is_reported_not_overwritten() {
    let (_home, path) = config_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "selkies = \"oops\"\n").unwrap();
    assert!(saved_servers_at(&path).is_err());
    assert!(save_server_at(&path, &saved("https://h.example/"), true).is_err());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "selkies = \"oops\"\n"
    );
    // An entry without a url is skipped, not an error.
    std::fs::write(
        &path,
        "[[selkies]]\nusername = \"x\"\n[[selkies]]\nurl = \"https://h.example/\"\n",
    )
    .unwrap();
    assert_eq!(saved_servers_at(&path).unwrap().len(), 1);
}

#[test]
fn environment_selkies_server_is_never_saved() {
    let server = environment_server("https://u:p@h.example/#token=t", "", "", "").unwrap();
    assert_eq!(server.source(), ServerSource::Environment);
    assert!(!server.remember());
    assert_eq!(server.auth().token.as_deref(), Some("t"));
    assert_eq!(server.auth().basic.as_ref().unwrap().username, "u");
}

#[test]
fn environment_pin_is_used_and_never_saved() {
    let text = crate::tls::fingerprint_text(&[0x12; 32]);
    let server = environment_server("https://h.example/", "ubuntu", "pw", &text).unwrap();
    assert_eq!(server.pin(), Some([0x12; 32]));
    assert_eq!(server.source(), ServerSource::Environment);
    let hex = "12".repeat(32);
    assert_eq!(
        environment_server("https://h.example/", "", "", &hex)
            .unwrap()
            .pin(),
        Some([0x12; 32])
    );
    assert!(environment_server("https://h.example/", "", "", "nope").is_err());
}

#[test]
fn startup_with_start_with_selkies_beats_a_config_key_but_not_an_env_key() {
    use crate::sealskin::KeySource;
    let s = Some(StartWith::Selkies);
    assert_eq!(
        startup_choice(Some(KeySource::ConfigFile), false, s, true),
        Startup::SelkiesSaved
    );
    assert_eq!(
        startup_choice(Some(KeySource::Environment), true, s, true),
        Startup::SealSkin
    );
    assert_eq!(
        startup_choice(Some(KeySource::DotEnv), false, s, true),
        Startup::SealSkin
    );
    assert_eq!(
        startup_choice(
            Some(KeySource::ConfigFile),
            false,
            Some(StartWith::SealSkin),
            true
        ),
        Startup::SealSkin
    );
    assert_eq!(startup_choice(None, false, s, false), Startup::Nothing);
    assert_eq!(
        startup_choice(None, true, None, false),
        Startup::SelkiesEnvironment
    );
    assert_eq!(
        startup_choice(Some(KeySource::ConfigFile), true, None, false),
        Startup::SelkiesEnvironment
    );
}

#[test]
fn startup_prefers_the_sealskin_environment_key_over_saved_selkies() {
    use crate::sealskin::KeySource;
    assert_eq!(
        startup_choice(
            Some(KeySource::Environment),
            false,
            Some(StartWith::Selkies),
            true
        ),
        Startup::SealSkin
    );
}
