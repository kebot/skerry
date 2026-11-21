use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use reqwest::{Client, Method, RequestBuilder, StatusCode, Url};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

const KEY_VARIABLE: &str = "SEALSKIN_APPLICATION_KEY";
const KEYCHAIN_SERVICE: &str = "dev.kebot.sealskin";
const KEYCHAIN_ACCOUNT: &str = "application-key";

/// The oldest SealSkin server version Skerry supports (ARCH-009). Older servers are refused.
pub const MINIMUM_SEALSKIN_VERSION: (u64, u64, u64) = (0, 5, 0);

pub struct Credential {
    raw: String,
    base: Url,
    authorization: HeaderValue,
}

impl Credential {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let raw = raw.trim();
        let (key, address) = raw.rsplit_once('@').ok_or_else(|| {
            "Paste the full application key, including the server address after @.".to_string()
        })?;
        let key_format = key.strip_prefix("ssk_").and_then(|key| key.split_once('_'));
        if !key_format.is_some_and(|(id, secret)| !id.is_empty() && !secret.is_empty())
            || !key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err("That doesn’t look like a SealSkin application key.".into());
        }
        let address = if address.contains("://") {
            address.to_string()
        } else {
            format!("https://{address}")
        };
        let base = Url::parse(&address).map_err(|_| "The key has an invalid server address.")?;
        if base.scheme() != "https"
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.path() != "/"
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err("The key must name a SealSkin server with an HTTPS address.".into());
        }
        let mut authorization = HeaderValue::from_str(&format!("Bearer {key}"))
            .map_err(|_| "The key contains invalid characters.")?;
        authorization.set_sensitive(true);
        Ok(Self {
            raw: raw.into(),
            base,
            authorization,
        })
    }

    pub fn startup() -> Result<Option<Self>, String> {
        if let Ok(value) = std::env::var(KEY_VARIABLE) {
            return Self::parse(&value).map(Some);
        }
        if let Ok(entries) = dotenvy::from_path_iter(".env") {
            for (name, value) in entries.flatten() {
                if name == KEY_VARIABLE {
                    return Self::parse(&value).map(Some);
                }
            }
        }
        match security_framework::passwords::get_generic_password(
            KEYCHAIN_SERVICE,
            KEYCHAIN_ACCOUNT,
        ) {
            Ok(bytes) => Self::parse(&String::from_utf8_lossy(&bytes)).map(Some),
            Err(error) if error.code() == -25300 => Ok(None), // errSecItemNotFound
            Err(_) => Err("Couldn’t read the saved application key from Keychain.".into()),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        security_framework::passwords::set_generic_password(
            KEYCHAIN_SERVICE,
            KEYCHAIN_ACCOUNT,
            self.raw.as_bytes(),
        )
        .map_err(|_| "Connected for this run. Keychain couldn’t save the key.".into())
    }

    pub fn forget() -> Result<(), String> {
        match security_framework::passwords::delete_generic_password(
            KEYCHAIN_SERVICE,
            KEYCHAIN_ACCOUNT,
        ) {
            Ok(()) => Ok(()),
            Err(error) if error.code() == -25300 => Ok(()),
            Err(_) => Err("Couldn’t remove the saved application key from Keychain.".into()),
        }
    }
}

#[derive(Clone, Deserialize)]
pub struct RemoteApp {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Deserialize)]
pub struct RemoteSession {
    pub session_id: String,
    pub app_name: String,
    pub session_url: String,
}

#[derive(Deserialize)]
struct Launched {
    session_id: String,
    session_url: String,
}

#[derive(Deserialize)]
struct ServerVersion {
    version: String,
}

#[derive(Deserialize)]
struct Failure {
    detail: String,
}

pub struct Library {
    pub apps: Vec<RemoteApp>,
    pub sessions: Vec<RemoteSession>,
}

#[derive(Clone)]
pub struct Api {
    client: Client,
    base: Url,
}

impl Api {
    pub fn new(credential: &Credential) -> Result<Self, String> {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, credential.authorization.clone());
        headers.insert("x-sealskin-app", HeaderValue::from_static("other"));
        let client = Client::builder()
            .default_headers(headers)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| "Couldn’t initialize the secure connection.")?;
        Ok(Self {
            client,
            base: credential.base.clone(),
        })
    }

    pub fn server(&self) -> &str {
        self.base
            .as_str()
            .trim_start_matches("https://")
            .trim_end_matches('/')
    }

    pub fn session_url(&self, path: &str) -> Result<Url, String> {
        let url = self
            .base
            .join(path)
            .map_err(|_| "The server returned an invalid session address.")?;
        if url.origin() != self.base.origin()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(
                "The server returned a session address outside this SealSkin server.".into(),
            );
        }
        Ok(url)
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        let mut url = self.base.clone();
        url.set_path(path);
        self.client.request(method, url)
    }

    /// Refuses any SealSkin server whose `GET /companion/api/ui/version` is older than
    /// [`MINIMUM_SEALSKIN_VERSION`] or unreadable. Servers older than 0.5.0 have no such route.
    pub async fn check_version(&self) -> Result<(), String> {
        let response = self
            .request(Method::GET, "/companion/api/ui/version")
            .send()
            .await
            .map_err(|_| {
                "Couldn’t reach the SealSkin server. Check the address, network, and HTTPS certificate."
                    .to_string()
            })?;
        if !response.status().is_success() {
            return Err(unsupported_version(None));
        }
        let server: ServerVersion = response
            .json()
            .await
            .map_err(|_| unsupported_version(None))?;
        supported_version(&server.version)
    }

    pub async fn library(&self) -> Result<Library, String> {
        self.check_version().await?;
        let apps = send(
            self.request(Method::POST, "/companion/api/applications")
                .json(&json!({})),
        );
        let sessions = send(self.request(Method::GET, "/companion/api/sessions"));
        let (apps, sessions) = tokio::try_join!(apps, sessions)?;
        Ok(Library { apps, sessions })
    }

    pub async fn launch(&self, app: RemoteApp) -> Result<RemoteSession, String> {
        let response: Launched = send(
            self.request(Method::POST, "/companion/api/launch/simple")
                .timeout(Duration::from_secs(180))
                .json(&json!({"application_id": app.id})),
        )
        .await?;
        Ok(RemoteSession {
            session_id: response.session_id,
            app_name: app.name,
            session_url: response.session_url,
        })
    }
}

/// Parses `MAJOR.MINOR.PATCH[-pre][+build]` and reports whether a pre-release tag is present.
fn parse_version(version: &str) -> Option<((u64, u64, u64), bool)> {
    let version = version.trim();
    let version = version.split_once('+').map_or(version, |(core, _)| core);
    let (core, pre) = match version.split_once('-') {
        Some((core, pre)) => (core, !pre.is_empty()),
        None => (version, false),
    };
    let mut parts = core.split('.').map(|part| {
        (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
            .then(|| part.parse::<u64>().ok())
            .flatten()
    });
    let parsed = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some((parsed, pre))
}

fn supported_version(version: &str) -> Result<(), String> {
    match parse_version(version) {
        // A pre-release of the minimum (e.g. `0.5.0-rc1`) precedes it, per SemVer.
        Some((core, pre))
            if core > MINIMUM_SEALSKIN_VERSION || (core == MINIMUM_SEALSKIN_VERSION && !pre) =>
        {
            Ok(())
        }
        _ => Err(unsupported_version(Some(version.trim()))),
    }
}

fn minimum_version() -> String {
    let (major, minor, patch) = MINIMUM_SEALSKIN_VERSION;
    format!("{major}.{minor}.{patch}")
}

fn unsupported_version(found: Option<&str>) -> String {
    let minimum = minimum_version();
    match found {
        Some(found) => format!(
            "Skerry requires SealSkin {minimum} or newer, but this server runs SealSkin {found}."
        ),
        None => format!(
            "Skerry requires SealSkin {minimum} or newer. This server is older or isn’t SealSkin."
        ),
    }
}

async fn send<T: DeserializeOwned>(request: RequestBuilder) -> Result<T, String> {
    let response = request.send().await.map_err(|error| {
        if error.is_timeout() {
            "The server took too long to respond. Refresh to check for running sessions."
        } else {
            "Couldn’t reach the SealSkin server. Check the address, network, and HTTPS certificate."
        }
    })?;
    let status = response.status();
    if status.is_success() {
        return response
            .json()
            .await
            .map_err(|_| "The server returned an unexpected response.".into());
    }
    let detail = response
        .json::<Failure>()
        .await
        .ok()
        .map(|failure| failure.detail);
    let error = match (status, detail.as_deref()) {
        (StatusCode::UNAUTHORIZED, Some("client")) => {
            "The account must allow the Other client type for application keys."
        }
        (StatusCode::UNAUTHORIZED, Some("expired")) => {
            "This application key has expired. Create a new key in SealSkin."
        }
        (StatusCode::UNAUTHORIZED, Some("disabled")) => {
            "Application keys are disabled for this account."
        }
        (StatusCode::UNAUTHORIZED, Some("dormant")) => {
            "This key is dormant. Sign in to the SealSkin web app before retrying."
        }
        (StatusCode::UNAUTHORIZED, _) => "The server didn’t accept this application key.",
        (StatusCode::FORBIDDEN, _) => "This account doesn’t have permission for that action.",
        _ => return Err(format!("SealSkin returned HTTP {}.", status.as_u16())),
    };
    Err(error.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_key_requires_a_trusted_https_origin() {
        let credential = Credential::parse("ssk_id_secret@seal.example:8443").unwrap();
        assert_eq!(credential.base.as_str(), "https://seal.example:8443/");
        assert!(credential.authorization.is_sensitive());
        for input in [
            "ssk_id_secret",
            "ssk_id_secret@http://seal.example",
            "ssk_id_secret@seal.example/path",
        ] {
            assert!(Credential::parse(input).is_err());
        }
    }

    #[test]
    fn sealskin_0_5_0_or_newer_is_supported() {
        assert_eq!(minimum_version(), "0.5.0");
        for version in [
            "0.5.0",
            " 0.5.0\n",
            "0.5.1",
            "0.6.0",
            "1.0.0",
            "0.10.0",
            "0.5.1-rc1",
            "0.5.0+build7",
        ] {
            assert!(supported_version(version).is_ok(), "{version}");
        }
        for version in [
            "0.4.2",
            "0.4.99",
            "0.5.0-rc1",
            "v0.5.0",
            "0.5",
            "0.5.0.1",
            "",
            "x.y.z",
        ] {
            let error = supported_version(version).unwrap_err();
            assert!(
                error.contains("requires SealSkin 0.5.0 or newer"),
                "{error}"
            );
        }
    }

    #[test]
    fn session_urls_stay_within_the_sealskin_origin() {
        let credential = Credential::parse("ssk_id_secret@seal.example:8443").unwrap();
        let api = Api::new(&credential).unwrap();
        let session = api.session_url("/session/?access_token=mock").unwrap();
        assert_eq!(
            session.as_str(),
            "https://seal.example:8443/session/?access_token=mock"
        );
        for path in [
            "//other.example/session/",
            "https://seal.example/session/",
            "http://seal.example:8443/session/",
        ] {
            assert!(api.session_url(path).is_err());
        }
    }
}
