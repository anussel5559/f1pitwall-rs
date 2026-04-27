pub mod models;

use anyhow::Result;
use reqwest::Client;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use models::*;

use crate::auth::{AuthManager, Credentials};

const BASE_URL: &str = "https://api.openf1.org/v1";

pub struct OpenF1Client {
    client: Client,
    rate_limiter: Arc<Mutex<RateLimiter>>,
    auth: Option<AuthManager>,
}

struct RateLimiter {
    timestamps: Vec<Instant>,
    last_request: Option<Instant>,
    max_requests: usize,
    window: Duration,
    min_interval: Duration,
}

impl RateLimiter {
    fn new(max_requests: usize, window: Duration, min_interval: Duration) -> Self {
        Self {
            timestamps: Vec::new(),
            last_request: None,
            max_requests,
            window,
            min_interval,
        }
    }

    async fn wait_if_needed(&mut self) {
        // Enforce minimum interval between any two requests (burst protection)
        if let Some(last) = self.last_request {
            let elapsed = last.elapsed();
            if elapsed < self.min_interval {
                tokio::time::sleep(self.min_interval - elapsed).await;
            }
        }

        // Enforce sliding window limit
        let now = Instant::now();
        self.timestamps
            .retain(|t| now.duration_since(*t) < self.window);

        if self.timestamps.len() >= self.max_requests {
            if let Some(oldest) = self.timestamps.first() {
                let wait = self.window - now.duration_since(*oldest);
                tokio::time::sleep(wait).await;
            }
            let now = Instant::now();
            self.timestamps
                .retain(|t| now.duration_since(*t) < self.window);
        }

        let now = Instant::now();
        self.timestamps.push(now);
        self.last_request = Some(now);
    }
}

impl OpenF1Client {
    pub async fn new(credentials: Option<Credentials>) -> Result<Self> {
        let auth = match credentials {
            Some(creds) => Some(AuthManager::new(creds).await?),
            None => None,
        };

        let (max_req, min_interval) = if auth.is_some() {
            (55, Duration::from_millis(200)) // authenticated: 55 req/min with headroom
        } else {
            (28, Duration::from_millis(400)) // public: 28 req/min
        };

        Ok(Self {
            client: Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("failed to build HTTP client"),
            rate_limiter: Arc::new(Mutex::new(RateLimiter::new(
                max_req,
                Duration::from_secs(60),
                min_interval,
            ))),
            auth,
        })
    }

    pub async fn is_authenticated(&self) -> bool {
        match &self.auth {
            Some(auth) => auth.is_valid().await,
            None => false,
        }
    }

    /// Borrow the AuthManager, if authenticated. Used by MQTT module for token access.
    pub fn auth_manager(&self) -> Option<&crate::auth::AuthManager> {
        self.auth.as_ref()
    }

    /// Build a URL with params that may contain operators like >= in the key.
    /// reqwest's .query() encodes these, but the OpenF1 API needs them raw.
    fn build_url(endpoint: &str, params: &[(&str, &str)]) -> String {
        let mut url = format!("{}{}", BASE_URL, endpoint);
        if !params.is_empty() {
            url.push('?');
            for (i, (k, v)) in params.iter().enumerate() {
                if i > 0 {
                    url.push('&');
                }
                // URL-encode the value but keep the key raw (preserves >=, <= etc.)
                // Keys like "date>=" already contain '=', so don't add another.
                // Keys like "date>" need the value appended directly (no extra '=').
                url.push_str(k);
                if !k.ends_with('=') && !k.ends_with('>') && !k.ends_with('<') {
                    url.push('=');
                }
                url.push_str(&urlencoding_value(v));
            }
        }
        url
    }

    async fn get<T: serde::de::DeserializeOwned>(
        &self,
        endpoint: &str,
        params: &[(&str, &str)],
    ) -> Result<Vec<T>> {
        let url = Self::build_url(endpoint, params);

        for attempt in 0..4 {
            self.rate_limiter.lock().await.wait_if_needed().await;

            let mut req = self.client.get(&url);
            if let Some(ref auth) = self.auth
                && let Some(token) = auth.get_valid_token().await
            {
                req = req.bearer_auth(token);
            }

            let resp = req.send().await?;
            let status = resp.status();
            // 404 means "no results found" — not a real error, just empty data
            if status == reqwest::StatusCode::NOT_FOUND {
                return Ok(Vec::new());
            }
            // 401: force re-auth and retry once
            if status == reqwest::StatusCode::UNAUTHORIZED
                && let Some(ref auth) = self.auth
            {
                auth.force_refresh().await;
                continue;
            }
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                // Back off and retry — the API rate-limited us
                let wait = Duration::from_secs(2u64.pow(attempt + 1)); // 2s, 4s, 8s
                tracing::warn!(endpoint, ?wait, "429 — backing off");
                tokio::time::sleep(wait).await;
                continue;
            }
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                anyhow::bail!("API error {status} for {url}: {body}");
            }
            let data = resp.json::<Vec<T>>().await?;
            return Ok(data);
        }

        anyhow::bail!("API request failed after retries for {url}")
    }

    pub async fn get_sessions(&self, params: &[(&str, &str)]) -> Result<Vec<Session>> {
        self.get("/sessions", params).await
    }

    pub async fn get_drivers(&self, session_key: i64) -> Result<Vec<Driver>> {
        let sk = session_key.to_string();
        self.get("/drivers", &[("session_key", &sk)]).await
    }

    pub async fn get_laps(
        &self,
        session_key: i64,
        date_gte: Option<&str>,
        date_lte: Option<&str>,
    ) -> Result<Vec<Lap>> {
        let sk = session_key.to_string();
        let mut params = vec![("session_key", sk.as_str())];
        if let Some(d) = date_gte {
            params.push(("date_start>=", d));
        }
        if let Some(d) = date_lte {
            params.push(("date_start<=", d));
        }
        self.get("/laps", &params).await
    }

    pub async fn get_positions(
        &self,
        session_key: i64,
        date_gte: Option<&str>,
        date_lte: Option<&str>,
    ) -> Result<Vec<Position>> {
        let sk = session_key.to_string();
        let mut params = vec![("session_key", sk.as_str())];
        if let Some(d) = date_gte {
            params.push(("date>=", d));
        }
        if let Some(d) = date_lte {
            params.push(("date<=", d));
        }
        self.get("/position", &params).await
    }

    pub async fn get_intervals(
        &self,
        session_key: i64,
        date_gte: Option<&str>,
        date_lte: Option<&str>,
    ) -> Result<Vec<Interval>> {
        let sk = session_key.to_string();
        let mut params = vec![("session_key", sk.as_str())];
        if let Some(d) = date_gte {
            params.push(("date>=", d));
        }
        if let Some(d) = date_lte {
            params.push(("date<=", d));
        }
        self.get("/intervals", &params).await
    }

    pub async fn get_stints(&self, session_key: i64) -> Result<Vec<Stint>> {
        let sk = session_key.to_string();
        self.get("/stints", &[("session_key", &sk)]).await
    }

    pub async fn get_pit_stops(&self, session_key: i64) -> Result<Vec<PitStop>> {
        let sk = session_key.to_string();
        self.get("/pit", &[("session_key", &sk)]).await
    }

    pub async fn get_race_control(
        &self,
        session_key: i64,
        date_gte: Option<&str>,
        date_lte: Option<&str>,
    ) -> Result<Vec<RaceControl>> {
        let sk = session_key.to_string();
        let mut params = vec![("session_key", sk.as_str())];
        if let Some(d) = date_gte {
            params.push(("date>=", d));
        }
        if let Some(d) = date_lte {
            params.push(("date<=", d));
        }
        self.get("/race_control", &params).await
    }

    pub async fn get_weather(
        &self,
        session_key: i64,
        date_lte: Option<&str>,
    ) -> Result<Vec<Weather>> {
        let sk = session_key.to_string();
        let mut params = vec![("session_key", sk.as_str())];
        if let Some(d) = date_lte {
            params.push(("date<=", d));
        }
        self.get("/weather", &params).await
    }

    pub async fn get_car_data(
        &self,
        session_key: i64,
        driver_number: i64,
        date_gte: Option<&str>,
        date_gt: Option<&str>,
        date_lte: Option<&str>,
    ) -> Result<Vec<CarData>> {
        let sk = session_key.to_string();
        let dn = driver_number.to_string();
        let mut params = vec![("session_key", sk.as_str()), ("driver_number", dn.as_str())];
        if let Some(d) = date_gte {
            params.push(("date>=", d));
        }
        if let Some(d) = date_gt {
            params.push(("date>", d));
        }
        if let Some(d) = date_lte {
            params.push(("date<=", d));
        }
        self.get("/car_data", &params).await
    }

    pub async fn get_location(
        &self,
        session_key: i64,
        driver_numbers: &[i64],
        date_gte: Option<&str>,
        date_gt: Option<&str>,
        date_lte: Option<&str>,
    ) -> Result<Vec<Location>> {
        let sk = session_key.to_string();
        let dn_strings: Vec<String> = driver_numbers.iter().map(|n| n.to_string()).collect();
        let mut params = vec![("session_key", sk.as_str())];
        for dn in &dn_strings {
            params.push(("driver_number", dn.as_str()));
        }
        if let Some(d) = date_gte {
            params.push(("date>=", d));
        }
        if let Some(d) = date_gt {
            params.push(("date>", d));
        }
        if let Some(d) = date_lte {
            params.push(("date<=", d));
        }
        self.get("/location", &params).await
    }

    pub async fn get_starting_grid(&self, meeting_key: i64) -> Result<Vec<StartingGrid>> {
        let mk = meeting_key.to_string();
        self.get("/starting_grid", &[("meeting_key", &mk)]).await
    }
}

/// Minimal URL encoding for values. Keeps chars safe for ISO timestamps
/// (colons, plus signs, hyphens, dots) unencoded.
fn urlencoding_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b':' => {
                out.push(b as char)
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}
