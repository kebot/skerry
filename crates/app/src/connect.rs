//! Turning a config Connection into something `rb_cdp::Connection` can run,
//! and the Manager pre-flight (is the Profile running? start it).

use rb_manager::{Error as ManagerError, ManagerClient};
use rb_store::{ConnectionConfig, ConnectionKind};

/// A Manager plus Profile.
#[derive(Clone, Debug)]
pub struct ManagerTarget {
    pub client: ManagerClient,
    pub profile_id: String,
}

/// A config Connection ready to connect.
#[derive(Debug)]
pub struct Resolved {
    pub cdp: rb_cdp::ConnectionConfig,
    /// Set for Manager Connections.
    pub manager: Option<ManagerTarget>,
}

/// CDP URL and token for a Connection; builds the Manager client if needed.
pub fn resolve(c: &ConnectionConfig) -> Result<Resolved, String> {
    match &c.kind {
        ConnectionKind::Cdp { cdp_url, token } => Ok(Resolved {
            cdp: rb_cdp::ConnectionConfig::new(
                cdp_url.clone(),
                token.as_ref().map(|t| rb_cdp::Token::new(t.expose())),
            ),
            manager: None,
        }),
        ConnectionKind::Manager {
            manager_url,
            profile_id,
            token,
        } => {
            let client =
                ManagerClient::new(manager_url, Some(rb_manager::Token::new(token.expose())))
                    .map_err(|e| e.to_string())?;
            let url = client.cdp_ws_url(profile_id);
            Ok(Resolved {
                cdp: rb_cdp::ConnectionConfig::new(url, Some(rb_cdp::Token::new(token.expose()))),
                manager: Some(ManagerTarget {
                    client,
                    profile_id: profile_id.clone(),
                }),
            })
        }
    }
}

/// Result of the Manager pre-flight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prepared {
    /// Connect now.
    Ready,
    /// The Profile is stopped: offer Start.
    NeedsStart,
    /// Cannot continue; `detail` says why. The user can retry.
    Failed { title: String, detail: String },
}

/// Check the Profile status on the Manager. A raw CDP Connection has no
/// pre-flight: pass `None`.
pub async fn prepare(manager: Option<&ManagerTarget>) -> Prepared {
    let Some(m) = manager else {
        return Prepared::Ready;
    };
    match m.client.status(&m.profile_id).await {
        Ok(s) if s.is_running() => Prepared::Ready,
        Ok(_) => Prepared::NeedsStart,
        // Unknown or stopped Profile: launching tells which.
        Err(ManagerError::NotFound { .. }) => Prepared::NeedsStart,
        Err(ManagerError::Unauthorized) => Prepared::Failed {
            title: "Authentication failed".into(),
            detail: "The Manager rejected the token (401). Check `token` in config.toml.".into(),
        },
        Err(e) => Prepared::Failed {
            title: "Cannot reach the Manager".into(),
            detail: e.to_string(),
        },
    }
}

/// Launch the Profile. 409 (already running) counts as success.
pub async fn launch(m: &ManagerTarget) -> Result<(), String> {
    match m.client.launch(&m.profile_id).await {
        Ok(_) => Ok(()),
        Err(e) if e.is_already_running() => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rb_store::load_config_str;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn raw_cdp_connection_resolves_to_its_url() {
        let c = load_config_str(
            "[[connection]]\nname=\"a\"\ncdp_url=\"ws://127.0.0.1:9222/\"\ntoken=\"t\"\n",
        )
        .unwrap();
        let r = resolve(&c.connections[0]).unwrap();
        assert_eq!(r.cdp.url, "ws://127.0.0.1:9222/");
        assert!(r.cdp.token.is_some() && r.manager.is_none());
    }

    #[test]
    fn manager_connection_resolves_to_the_profile_cdp_url() {
        let c = load_config_str(
            "[[connection]]\nname=\"m\"\nmanager_url=\"https://mgr.example.com\"\nprofile_id=\"p1\"\ntoken=\"SECRET\"\n",
        )
        .unwrap();
        let r = resolve(&c.connections[0]).unwrap();
        assert_eq!(r.cdp.url, "wss://mgr.example.com/api/profiles/p1/cdp");
        assert_eq!(r.manager.as_ref().unwrap().profile_id, "p1");
        assert!(!format!("{r:?}").contains("SECRET"));
    }

    /// One-shot HTTP server answering every request with `status` and `body`.
    async fn fake_http(status: u16, body: &'static str) -> String {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let _ = s.read(&mut buf).await;
                    let resp = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = s.write_all(resp.as_bytes()).await;
                });
            }
        });
        format!("http://{addr}")
    }

    fn target(base: &str) -> ManagerTarget {
        ManagerTarget {
            client: ManagerClient::new(base, None).unwrap(),
            profile_id: "p".into(),
        }
    }

    #[tokio::test]
    async fn prepare_maps_profile_status() {
        let running = fake_http(200, r#"{"status":"running"}"#).await;
        assert_eq!(prepare(Some(&target(&running))).await, Prepared::Ready);
        let stopped = fake_http(200, r#"{"status":"stopped"}"#).await;
        assert_eq!(prepare(Some(&target(&stopped))).await, Prepared::NeedsStart);
        let unauth = fake_http(401, r#"{"detail":"no"}"#).await;
        assert!(matches!(
            prepare(Some(&target(&unauth))).await,
            Prepared::Failed { .. }
        ));
        assert_eq!(prepare(None).await, Prepared::Ready);
    }

    #[tokio::test]
    async fn launch_treats_409_as_success_and_surfaces_license_errors() {
        let conflict = fake_http(409, r#"{"detail":"already running"}"#).await;
        assert!(launch(&target(&conflict)).await.is_ok());
        let license = fake_http(402, r#"{"detail":"no seats"}"#).await;
        assert!(launch(&target(&license)).await.is_err());
    }
}
