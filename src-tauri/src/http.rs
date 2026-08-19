//! The one HTTP client, plus the `Fetch` trait every provider is written
//! against. Providers never touch reqwest directly, so their resolution logic
//! can be driven from recorded fixtures in tests without a network.

use std::future::Future;
use std::time::Duration;

use crate::error::{AppError, AppResult};

pub const USER_AGENT: &str = concat!(
    "mc-server-manager/",
    env!("CARGO_PKG_VERSION"),
    " (desktop server manager)"
);

/// Text fetching, the only network shape provider resolution needs.
pub trait Fetch: Send + Sync {
    fn get_text(&self, url: &str) -> impl Future<Output = AppResult<String>> + Send;

    fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
    ) -> impl Future<Output = AppResult<T>> + Send {
        async move {
            let body = self.get_text(url).await?;
            serde_json::from_str(&body).map_err(|e| {
                AppError::Other(format!("{url} returned JSON this build cannot read: {e}"))
            })
        }
    }
}

#[derive(Clone)]
pub struct Http {
    client: reqwest::Client,
}

impl Http {
    pub fn new() -> AppResult<Self> {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(15))
            .pool_idle_timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| AppError::Network(format!("could not start the HTTP client: {e}")))?;
        Ok(Self { client })
    }

    /// Last resort when the configured client cannot be built: reqwest's own
    /// defaults still work for plain requests.
    pub fn default_client() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }
}

impl Fetch for Http {
    async fn get_text(&self, url: &str) -> AppResult<String> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| crate::error::from_reqwest(url, &e))?;

        let status = response.status();
        if !status.is_success() {
            return Err(AppError::Network(format!(
                "{url} answered {} {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("")
            )));
        }

        response
            .text()
            .await
            .map_err(|e| AppError::Network(format!("{url} returned an unreadable body: {e}")))
    }
}

/// Fixture-backed `Fetch` for tests: maps exact URLs to recorded payloads and
/// fails loudly on any URL a test did not record.
#[cfg(test)]
pub struct FixtureFetch {
    routes: std::collections::HashMap<String, String>,
}

#[cfg(test)]
impl Default for FixtureFetch {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl FixtureFetch {
    pub fn new() -> Self {
        Self {
            routes: std::collections::HashMap::new(),
        }
    }

    /// Loads `src-tauri/tests/fixtures/<file>` for the given URL.
    pub fn route(mut self, url: &str, file: &str) -> Self {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(file);
        let body = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("fixture {} is missing: {e}", path.display()));
        self.routes.insert(url.to_string(), body);
        self
    }
}

#[cfg(test)]
impl Fetch for FixtureFetch {
    async fn get_text(&self, url: &str) -> AppResult<String> {
        self.routes
            .get(url)
            .cloned()
            .ok_or_else(|| AppError::Network(format!("no fixture recorded for {url}")))
    }
}
