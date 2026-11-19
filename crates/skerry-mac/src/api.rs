use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use reqwest::{Client, Method, RequestBuilder, StatusCode, Url};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

const KEY_VARIABLE: &str = "SEALSKIN_APPLICATION_KEY";
const KEYCHAIN_SERVICE: &str = "dev.kebot.sealskin";
const KEYCHAIN_ACCOUNT: &str = "application-key";

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

    pub async fn library(&self) -> Result<Library, String> {
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
